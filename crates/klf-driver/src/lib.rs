//! The driver loop (section 9.1, M4 slice): given a compiled KIR module, run
//! one of its functions on a chosen backend. Compilation itself is done by a
//! [`Definition`]; the driver picks the backend and executes.

mod corpus;
mod format;
mod frontend;
mod kernel;
mod language;
mod lock;
mod native;
mod phase_runner;
mod shader_sysroot;
mod sysroot;
mod unit_tests;

pub use corpus::{
    CaseResult, Compared, CorpusCase, CorpusManifest, EditorCase, EditorCheck, EditorFile,
    EditorManifest, Engine, has_corpus, read_corpora, read_editor_corpora, run_corpus,
    run_editor_corpus, run_shader_corpus, test_package,
};
pub use frontend::{BuiltFrontend, Profile, build_frontend};
pub use kernel::{
    KernelArchive, build_distribution, distribution_home, find_clang, find_llvm_tool, host_triple,
    kernel_archive,
};
pub use language::{LanguageDefinition, PackageError, load_package};
pub use native::{NativeError, build_native, toolchain_available};
pub use sysroot::{BuiltSysroot, build_sysroot};
pub use unit_tests::run_unit_tests;

use klf_core::FileId;
use klf_defsvc::PhaseRunner;
use klf_diag::progress;
use klf_kir::Module;
use klf_services::{
    Definition, DefinitionHandleError, DefinitionImage, DefinitionPackage, DefinitionPhase,
    DefinitionPhaseArguments, DefinitionPhaseExecutor, DefinitionPhaseResult, DefinitionRegistry,
    DefinitionRegistryError, ExecutableDefinition, Session,
};
use klf_target::Target;
use klf_vm::{
    Outcome, RuntimeDiagnostic, kernel_services_with_source_and_diagnostics,
    run_module_with_capabilities,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    Vm,
    Native,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackendDefinitionExecutor {
    backend: Backend,
}

impl BackendDefinitionExecutor {
    #[must_use]
    pub const fn new(backend: Backend) -> Self {
        Self { backend }
    }
}

impl DefinitionPhaseExecutor for BackendDefinitionExecutor {
    fn execute(
        &self,
        image: &DefinitionImage,
        phase: DefinitionPhase,
        session: &mut Session,
        arguments: &DefinitionPhaseArguments,
    ) -> Result<DefinitionPhaseResult, String> {
        if !phase_runner::hosts_artifacts(self.backend) {
            return run_definition_phase_result(image, phase, session, arguments, self.backend);
        }
        let program = phase_runner::PhaseProgram::new(image)?;
        let mut host = program.host(image);
        let result = host.run(phase, session, arguments)?;
        if !host.balanced() {
            return Err("KLF-RT-0100".to_owned());
        }
        Ok(result)
    }
}

impl BackendDefinitionExecutor {
    #[must_use]
    pub(crate) const fn backend(self) -> Backend {
        self.backend
    }
}

/// Registers a compiled definition image with the selected backend executor.
///
/// # Errors
/// Returns [`DefinitionRegistryError`] when the image's metadata collides with
/// a definition already in the registry.
pub fn register_executable_definition(
    registry: &mut DefinitionRegistry,
    image: DefinitionImage,
    backend: Backend,
) -> Result<(), DefinitionRegistryError> {
    registry.register(ExecutableDefinition::with_executor(
        image,
        BackendDefinitionExecutor::new(backend),
    ))
}

/// Compiles source with a definition, reporting diagnostics into `session`.
#[must_use]
pub fn compile(
    definition: &dyn Definition,
    session: &mut Session,
    source_name: &str,
    source: &str,
    module_name: &str,
    target: Target,
) -> Option<Module> {
    definition.compile(session, source_name, source, module_name, target)
}

/// Loads and compiles a definition package through its phase boundary.
#[must_use]
pub fn compile_package(
    definition: &dyn Definition,
    session: &mut Session,
    package: &DefinitionPackage,
    module_name: &str,
    target: Target,
) -> Option<Module> {
    definition.compile_package(session, package, module_name, target)
}

/// Runs one function of a module on the chosen backend.
///
/// # Errors
/// Returns a message when lowering, linking or running fails, or when a native
/// run is requested but the toolchain is not installed.
pub fn run(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    backend: Backend,
) -> Result<Outcome, String> {
    run_internal(module, entry, arguments, backend, None).map(|(outcome, _)| outcome)
}

/// Runs a compiled definition with one source buffer exposed through the
/// kernel source services.
///
/// # Errors
/// Returns a message when lowering, linking or running fails, or when the
/// native toolchain is not installed.
pub fn run_with_source(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    source: &str,
    backend: Backend,
) -> Result<Outcome, String> {
    run_internal(module, entry, arguments, backend, Some(source)).map(|(outcome, _)| outcome)
}

/// Runs a compiled definition and returns typed runtime diagnostic records.
///
/// # Errors
/// Returns a message when lowering, linking or running fails, or when a native
/// run is requested but the toolchain is not installed.
pub fn run_with_source_and_diagnostics(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    source: &str,
    backend: Backend,
) -> Result<(Outcome, Vec<RuntimeDiagnostic>), String> {
    run_internal(module, entry, arguments, backend, Some(source))
}

/// Runs a module on the VM, returning its outcome and the bytes written to the
/// `klf.io` output sink. The `source` buffer is exposed through the source
/// services so generated code can read it while it runs.
///
/// # Errors
/// Returns a message when the run fails, or when a non-VM backend is requested
/// (output capture is VM-only).
pub fn run_with_output(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    source: &str,
    backend: Backend,
) -> Result<(Outcome, Vec<u8>), String> {
    if backend != Backend::Vm {
        return Err("output capture is only supported on the VM backend".to_owned());
    }
    let module = klf_kir_passes::monomorphize(module);
    let (capabilities, sink, _) = kernel_services_with_source_and_diagnostics(source);
    let outcome = run_module_with_capabilities(&module, entry, arguments, &capabilities)
        .map_err(|error| error.to_string())?;
    let output = sink
        .lock()
        .map_err(|_| "output sink lock poisoned".to_owned())?
        .clone();
    Ok((outcome, output))
}

/// Runs one validated definition-image phase with typed session-owned handles.
///
/// # Errors
/// Returns a handle or backend execution error when the phase cannot be run.
pub fn run_definition_phase(
    image: &DefinitionImage,
    phase: DefinitionPhase,
    session: &Session,
    arguments: &DefinitionPhaseArguments,
    backend: Backend,
) -> Result<Outcome, String> {
    if arguments.phase() != phase {
        return Err(DefinitionHandleError::PhaseMismatch {
            requested: phase,
            arguments: arguments.phase(),
        }
        .to_string());
    }
    let words = arguments
        .marshal(session)
        .map_err(|error| error.to_string())?;
    run(image.module(), phase.symbol(), &words, backend)
}

/// Runs a definition phase and resolves its return word through the Session.
///
/// # Errors
/// Returns a handle, backend or phase-result error when execution does not
/// produce the expected typed value.
pub fn run_definition_phase_result(
    image: &DefinitionImage,
    phase: DefinitionPhase,
    session: &Session,
    arguments: &DefinitionPhaseArguments,
    backend: Backend,
) -> Result<DefinitionPhaseResult, String> {
    let outcome = run_definition_phase(image, phase, session, arguments, backend)?;
    let word = match outcome {
        Outcome::Return(word) => word,
        Outcome::Fail(value) => return Err(format!("definition phase failed with {value}")),
        Outcome::Trap(message) => return Err(message),
    };
    DefinitionPhaseResult::from_return_word(phase, session, arguments, word)
        .map_err(|error| error.to_string())
}

/// Runs a compiled definition and resolves its runtime diagnostics into a
/// session using the registered definition specifications.
///
/// # Errors
/// Returns a message when execution fails or when a runtime diagnostic is not
/// registered or does not identify a valid span in `session`.
pub fn run_with_source_into_session(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    source: &str,
    backend: Backend,
    session: &mut Session,
    file: FileId,
) -> Result<Outcome, String> {
    let (outcome, diagnostics) =
        run_with_source_and_diagnostics(module, entry, arguments, source, backend)?;
    session
        .report_runtime_diagnostics(file, &diagnostics)
        .map_err(|record| {
            format!(
                "unregistered or invalid runtime diagnostic code {}",
                record.code
            )
        })?;
    Ok(outcome)
}

fn run_internal(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    backend: Backend,
    source: Option<&str>,
) -> Result<(Outcome, Vec<RuntimeDiagnostic>), String> {
    if backend == Backend::Native {
        progress!("native build and link");
    }
    let module = klf_kir_passes::monomorphize(module);
    match backend {
        Backend::Vm => {
            let (capabilities, sink, diagnostics) =
                kernel_services_with_source_and_diagnostics(source.unwrap_or_default());
            let outcome = run_module_with_capabilities(&module, entry, arguments, &capabilities)
                .map_err(|e| e.to_string())?;
            // What the program wrote to `klf.io` is its standard output, as a
            // native run's is.
            let output = sink
                .lock()
                .map_err(|_| "output sink lock poisoned".to_owned())?;
            let mut stdout = std::io::stdout().lock();
            std::io::Write::write_all(&mut stdout, &output)
                .and_then(|()| std::io::Write::flush(&mut stdout))
                .map_err(|error| format!("cannot write the program's output: {error}"))?;
            drop(output);
            let diagnostics = diagnostics
                .lock()
                .map_err(|_| "runtime diagnostics lock poisoned".to_owned())?
                .clone();
            Ok((outcome, diagnostics))
        }
        Backend::Native => native::run_native_with_diagnostics(&module, entry, arguments, source)
            .map_err(|e| e.to_string()),
    }
}
