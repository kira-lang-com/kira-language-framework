//! `<driver> lsp` and its analysis worker (`KLF-EDITOR.md` sections 8.6
//! and 10).
//!
//! Where `klf build` gives definition code unwind tables (ELF and Mach-O
//! hosts), the server analyzes in its own process: the main thread, which
//! owns the definition's globals, heap and stack limit, runs one analysis
//! at a time, and a cancellation or trap unwinds the definition's frames
//! back to the phase boundary. Elsewhere the server starts `<driver> lsp-worker` for each
//! analysis, so a trap in definition code ends a worker, not the server.

use crate::arguments::parse_arguments;
use crate::{FrontendMetadata, NativePhases, add_inputs, new_session};
use klf_defsvc::{DriveError, drive};
use klf_ide::{
    Analysis, AnalysisRequest, Analyzer, Cancellation, FileInfo, IdeDiagnostic, Interruption,
    KirFunction, Languages, ProcessAnalyzer, ProcessFormatter,
};
use klf_lsp::ServerConfig;
use klf_native_rt::interrupt::{self, Interrupt, Pending};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};

/// Serves the language server on standard input and output.
pub(crate) fn serve(metadata: &FrontendMetadata) -> i32 {
    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(error) => {
            klf_diag::err!(
                "{}: error: cannot find this executable: {error}",
                metadata.driver
            );
            return 1;
        }
    };
    let formatter = metadata.declares(klf_defsvc::Hook::Format).then(|| {
        Arc::new(ProcessFormatter {
            program: program.clone(),
        }) as Arc<dyn klf_ide::Formatter>
    });
    let config = |analyzer: Arc<dyn Analyzer>| ServerConfig {
        name: metadata.driver.clone(),
        version: metadata.version.clone(),
        languages: Languages::single(metadata.editor_spec()),
        analyzer,
        formatter,
    };
    let served = if UNWINDS {
        let (jobs, queue) = channel();
        let config = config(Arc::new(InProcess {
            jobs: Mutex::new(jobs),
        }));
        let server = std::thread::spawn(move || klf_lsp::serve(config).map_err(|e| e.to_string()));
        run_jobs(metadata, &queue);
        server
            .join()
            .unwrap_or_else(|_| Err("the server thread panicked".to_owned()))
    } else {
        klf_lsp::serve(config(Arc::new(ProcessAnalyzer {
            program,
            arguments: vec!["lsp-worker".to_owned()],
        })))
        .map_err(|e| e.to_string())
    };
    match served {
        Ok(()) => 0,
        Err(error) => {
            klf_diag::err!("{}: error: {error}", metadata.driver);
            1
        }
    }
}

/// Whether this host's definition code has the unwind tables an in-process
/// analysis needs (`klf build` writes them for ELF and Mach-O objects).
const UNWINDS: bool = cfg!(unix);

/// An analysis for the main thread, and where its result goes.
struct Job {
    request: AnalysisRequest,
    cancellation: Arc<AtomicCancellation>,
    reply: Sender<Analysis>,
}

/// The cancellation of a queued job, shared with the main thread.
type AtomicCancellation = std::sync::atomic::AtomicBool;

/// Analyzes on the main thread, one request at a time.
struct InProcess {
    jobs: Mutex<Sender<Job>>,
}

impl Analyzer for InProcess {
    fn analyze(&self, request: &AnalysisRequest, cancellation: &Cancellation) -> Analysis {
        let (reply, result) = channel();
        let job = Job {
            request: request.clone(),
            cancellation: cancellation.flag(),
            reply,
        };
        let pending = Pending::default();
        cancellation.on_cancel(pending.signal());
        let sent = self
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(job);
        if sent.is_err() {
            return Analysis::failed("the analysis thread has stopped");
        }
        let analysis = result
            .recv()
            .unwrap_or_else(|_| Analysis::failed("the analysis thread has stopped"));
        // The pending count drops once the analysis no longer runs.
        drop(pending);
        analysis
    }
}

/// Runs the server's analyses on this thread until the server ends.
fn run_jobs(metadata: &FrontendMetadata, queue: &Receiver<Job>) {
    for job in queue {
        let cancelled = || job.cancellation.load(std::sync::atomic::Ordering::Acquire);
        if cancelled() {
            let _ = job
                .reply
                .send(Analysis::interrupted(Interruption::Cancelled));
            continue;
        }
        let live = klf_native_rt::live_blocks();
        let flag = Arc::clone(&job.cancellation);
        let mut analysis = interrupt::serve(Arc::clone(&flag), || {
            klf_vm::cancellable(flag, || analyze(metadata, &job.request))
        });
        if let Some((phase, stopped)) = crate::take_interrupted() {
            analysis.interrupted = Some(match stopped {
                Interrupt::Cancelled => Interruption::Cancelled,
                Interrupt::Trapped(code) => Interruption::Trapped {
                    code: format!("KLF-RT-{code:04}"),
                    query: phase.symbol().to_owned(),
                },
                Interrupt::CheckFailed(message) => Interruption::Trapped {
                    code: message.lines().next().unwrap_or_default().to_owned(),
                    query: phase.symbol().to_owned(),
                },
                Interrupt::Failed(value) => Interruption::Trapped {
                    code: format!("uncaught failure {value}"),
                    query: phase.symbol().to_owned(),
                },
                Interrupt::Panicked(message) => Interruption::Trapped {
                    code: format!("kernel panic: {message}"),
                    query: phase.symbol().to_owned(),
                },
            });
        } else if cancelled() {
            analysis.interrupted = Some(Interruption::Cancelled);
        }
        if analysis.interrupted.is_some() {
            let leaked = klf_native_rt::live_blocks().saturating_sub(live);
            let balance = if leaked == 0 {
                "the heap is balanced".to_owned()
            } else {
                format!("{leaked} blocks leaked")
            };
            analysis.failure = Some(match analysis.failure.take() {
                Some(failure) => format!("{failure}; {balance}"),
                None => balance,
            });
        }
        let _ = job.reply.send(analysis);
    }
}

/// Reads an analysis request on standard input and writes the analysis on
/// standard output.
pub(crate) fn worker(metadata: &FrontendMetadata) -> i32 {
    let mut input = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut input) {
        klf_diag::err!(
            "{}: error: cannot read the analysis request: {error}",
            metadata.driver
        );
        return 1;
    }
    let analysis = match AnalysisRequest::decode(&input) {
        Ok(request) => analyze(metadata, &request),
        Err(error) => Analysis::failed(error),
    };
    let mut output = std::io::stdout().lock();
    if output
        .write_all(analysis.encode().as_bytes())
        .and_then(|()| output.flush())
        .is_err()
    {
        return 1;
    }
    0
}

/// Runs the phases over the request's project in `Editor` mode.
pub(crate) fn analyze(metadata: &FrontendMetadata, request: &AnalysisRequest) -> Analysis {
    if !request.directory.as_os_str().is_empty()
        && std::env::set_current_dir(&request.directory).is_err()
    {
        return Analysis::failed(format!("cannot enter {}", request.directory.display()));
    }
    let invocation = match parse_arguments(&request.arguments, metadata) {
        Ok(invocation) => invocation,
        Err(error) => return Analysis::failed(error),
    };
    let triple = invocation
        .target
        .clone()
        .unwrap_or_else(|| metadata.target.clone());
    let mut session = match new_session(metadata, &invocation, &triple) {
        Ok(session) => session,
        Err(error) => return Analysis::failed(error),
    };
    session.set_mode(klf_facts::AnalysisMode::Editor);
    session.set_overlay(
        request
            .documents
            .iter()
            .map(|(path, text)| (path.clone(), Arc::from(text.as_str()))),
    );
    let shader = metadata.shader.is_some();
    let mut failure = None;
    let mut pipeline = crate::shader_editor::Found::default();
    let mut kir = Vec::new();
    match add_inputs(&mut session, metadata, &invocation) {
        Ok(files) if shader => match crate::shader_editor::run(metadata, &mut session, &files) {
            Ok(compiled) => pipeline = compiled,
            Err(error) => failure = Some(error),
        },
        Ok(files) => {
            let mut make_runner = || Ok(NativePhases::new());
            match drive(&mut session, &files, &mut make_runner) {
                Ok(module) if request.kir => kir = kir_functions(&session, &module),
                Ok(_) | Err(DriveError::Phase { .. }) => {}
                Err(error) => failure = Some(error.to_string()),
            }
        }
        Err(error) => failure = Some(error),
    }
    let mut diagnostics: Vec<IdeDiagnostic> = session
        .runtime_diagnostics()
        .iter()
        .map(|diagnostic| IdeDiagnostic {
            file: diagnostic.span.file,
            start: diagnostic.span.start,
            end: diagnostic.span.end,
            severity: diagnostic.severity,
            code: diagnostic.code.to_string(),
            message: format!("{}: {}", diagnostic.title, diagnostic.explanation),
            source: None,
        })
        .collect();
    diagnostics.extend(session.diagnostics().iter().filter_map(|diagnostic| {
        let span = diagnostic.primary_span?;
        Some(IdeDiagnostic {
            file: span.file,
            start: span.start,
            end: span.end,
            severity: diagnostic.severity,
            code: diagnostic.code.to_owned(),
            message: diagnostic.message.clone(),
            source: None,
        })
    }));
    diagnostics.extend(pipeline.diagnostics);
    let mut files = session
        .files()
        .iter()
        .map(|file| {
            let name = PathBuf::from(file.name().as_str());
            (file.id(), std::path::absolute(&name).unwrap_or(name))
        })
        .collect();
    let mut facts = session.take_facts();
    crate::ridl_declarations::redirect(&session, &mut facts, &mut files);
    let file_info = session
        .files()
        .iter()
        .map(|file| {
            let id = file.id();
            FileInfo {
                file: id,
                mode: session
                    .file_mode(id)
                    .and_then(|index| metadata.file_modes.get(index))
                    .map(|mode| mode.name.clone())
                    .unwrap_or_default(),
                memory: session
                    .file_memory(id)
                    .map(|memory| format!("{memory:?}"))
                    .unwrap_or_default(),
                convention: session
                    .file_convention(id)
                    .map(|convention| format!("{convention:?}"))
                    .unwrap_or_default(),
                unit: session.unit_holding(id).map(|unit| unit.as_u32()),
            }
        })
        .collect();
    let expansions = session
        .expansions()
        .iter()
        .filter(|expansion| session.has_file(expansion.file))
        .map(|expansion| klf_ide::Expansion {
            origin: expansion.origin.file,
            start: expansion.origin.start,
            end: expansion.origin.end,
            name: expansion.name.clone(),
            file: expansion.file,
            text: session.file(expansion.file).text().to_string(),
        })
        .collect();
    Analysis {
        files,
        diagnostics,
        facts,
        shader_outputs: pipeline.outputs,
        file_info,
        kir,
        expansions,
        failure,
        interrupted: None,
    }
}

/// The KIR of each function a source file of the session named, raw and
/// after the mandatory passes (`KLF-EDITOR.md` section 13.7).
fn kir_functions(session: &klf_services::Session, module: &klf_kir::Module) -> Vec<KirFunction> {
    let (canonical, _) = klf_kir_passes::canonicalize(module);
    let sources: Vec<_> = session.files().iter().map(|file| file.id()).collect();
    module
        .functions
        .iter()
        .zip(&canonical.functions)
        .filter(|(function, _)| !function.foreign && sources.contains(&function.span.file))
        .filter(|(function, _)| function.span.start < function.span.end)
        .map(|(raw, canonical)| KirFunction {
            file: raw.span.file,
            start: raw.span.start,
            end: raw.span.end,
            name: raw.name.as_str().to_owned(),
            raw: klf_kir::function_text(raw),
            canonical: klf_kir::function_text(canonical),
        })
        .collect()
}
