//! The compiler driver inside a natively compiled definition package.
//!
//! `klf build` turns a `.klfd` package into an object that carries the
//! definition's own phase code plus its descriptor blob, a trampoline from a
//! phase number to the phase function, and its global prologue, joined with
//! any other definitions of the binary by a registry (see `bundle`). The
//! registry's `main` runs every prologue and then calls [`klffe_main`] here. Everything a driver does around the phases —
//! argument parsing, reading the inputs, building the Session, running the
//! schedule, and writing the requested output — is this file.
//!
//! The schedule itself is `klf_defsvc::drive`, the same function the VM-hosted
//! driver in `klf-driver` calls, and the services the phases reach for come
//! from the same `klf-defsvc` bridge. A native frontend is therefore the
//! kernel's own pipeline with the phase calls compiled instead of interpreted.

#[cfg(any(unix, windows))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Has the allocator commit memory as it is used and return freed pages a
/// second after they are freed, which keeps a large build's peak near what it
/// holds live. Returning them at once had each phase fault back in the pages
/// the one before it freed, which nearly doubled a build's time in the kernel.
/// The option numbers are those of the mimalloc release `libmimalloc-sys`
/// 0.1.49 builds.
#[cfg(any(unix, windows))]
fn configure_allocator() {
    const ARENA_EAGER_COMMIT: libmimalloc_sys::mi_option_t = 4;
    const PURGE_DELAY: libmimalloc_sys::mi_option_t = 15;
    // SAFETY: setting an option only changes how later allocations and frees
    // commit and return memory.
    unsafe {
        libmimalloc_sys::mi_option_set(ARENA_EAGER_COMMIT, 0);
        libmimalloc_sys::mi_option_set(PURGE_DELAY, 1000);
    }
}

#[cfg(not(any(unix, windows)))]
fn configure_allocator() {}

mod arguments;
mod bundle;
mod command;
mod contract_reach;
mod depfile;
mod editor;
mod format;
mod module_cache;
mod module_pruning;
mod native_link;
mod optimizer_cache;
mod output;
mod program_cache;
mod push;
mod ridl_declarations;
mod ridl_entries;
mod ridl_imports;
mod ridl_sources;
mod ridl_surface;
mod ridl_witnesses;
mod run;
mod session_inputs;
mod runtime;
mod shader;
mod shader_editor;
mod source_closure;
mod toolchain;
mod vm_natives;

use klf_args::{Action, Command, Outcome};
use klf_defsvc::{
    CompatDriverCall, DriveError, FrontendMetadata, PhaseRunner, ServiceSink, complete_phase,
    drive, drive_cached, driver_cli, prepare_phase, render_kept_diagnostics,
    settle_phase_diagnostics,
};
use klf_native_rt::interrupt::{self, Interrupt};
use klf_native_rt::services;
use klf_services::{
    DefinitionPhase, DefinitionPhaseArguments, DefinitionPhaseResult, DriverRequest, Session,
    select_driver_arguments,
};
use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int};

pub(crate) use session_inputs::{add_inputs, expand_inputs, new_session};

unsafe extern "C-unwind" {
    /// Calls one of the definition's four phases. `selector` is the phase's
    /// [`DefinitionPhase::index`]; unused argument slots are passed as zero.
    /// In a language server an interrupt unwinds out of it.
    fn klffe_phase(selector: i64, first: i64, second: i64, third: i64) -> i64;
    /// The NUL-terminated descriptor blob `klf build` embedded.
    fn klffe_metadata() -> *const c_char;
}

/// The trampoline selectors for the generated `compatDriver`, `runProgram`
/// and `emit` entries.
const COMPAT_DRIVER_SELECTOR: i64 = 4;
const RUN_PROGRAM_SELECTOR: i64 = 5;
const EMIT_SELECTOR: i64 = 6;

thread_local! {
    static MATERIALIZES: RefCell<[bool; 4]> = const { RefCell::new([false; 4]) };
    /// The interrupt that stopped the last phase on this thread, with the
    /// phase it stopped.
    static INTERRUPTED: RefCell<Option<(DefinitionPhase, Interrupt)>> = const { RefCell::new(None) };
}

/// Runs the definition's phases in this process.
///
/// One of these is made per unit, so the block count it records on entry is
/// the unit's heap baseline for the section 15.6 check.
pub(crate) struct NativePhases {
    blocks_on_entry: usize,
}

impl NativePhases {
    pub(crate) fn new() -> Self {
        Self {
            blocks_on_entry: klf_native_rt::live_blocks(),
        }
    }
}

impl PhaseRunner for NativePhases {
    fn balanced(&self) -> bool {
        klf_native_rt::live_blocks() == self.blocks_on_entry
    }

    fn run(
        &mut self,
        phase: DefinitionPhase,
        session: &mut Session,
        arguments: &DefinitionPhaseArguments,
    ) -> Result<DefinitionPhaseResult, String> {
        if interrupt::cancel_requested() {
            return Err(interrupted(phase, Interrupt::Cancelled));
        }
        let materializes = MATERIALIZES.with(|flags| flags.borrow()[phase.index()]);
        let (bridge, words, source) = prepare_phase(session, phase, arguments, materializes)?;
        let file = bridge.file;
        let mut sink = ServiceSink::new();
        bridge.register(&mut sink);
        services::install(sink.into_table(), source);

        let selector = i64::try_from(phase.index()).unwrap_or(-1);
        let argument = |index: usize| words.get(index).map_or(0, |word| *word as i64);
        // SAFETY: `klffe_phase` is emitted into this binary by the backend with
        // exactly this signature, and the words come from `prepare_phase`,
        // which marshalled them for this phase.
        let called = interrupt::catching(|| unsafe {
            klffe_phase(selector, argument(0), argument(1), argument(2))
        });

        let reported = services::uninstall();
        // What an interrupted phase recorded goes with it (section 8.5).
        let word = called.map_err(|stopped| interrupted(phase, stopped))?;
        bridge.commit_records(session)?;
        settle_phase_diagnostics(session, file, &reported)?;
        complete_phase(bridge, session, phase, arguments, Some(word as u64))
    }
}

/// Notes `interrupt` as what stopped `phase` and describes it.
fn interrupted(phase: DefinitionPhase, interrupt: Interrupt) -> String {
    let message = match &interrupt {
        Interrupt::Cancelled => "cancelled".to_owned(),
        Interrupt::Trapped(code) => format!("\n{}", klf_native_rt::trap_report(*code)),
        Interrupt::CheckFailed(message) => format!("\n{message}"),
        Interrupt::Failed(value) => format!("uncaught failure {value}"),
        Interrupt::Panicked(message) => format!("kernel panic: {message}"),
    };
    INTERRUPTED.with(|slot| *slot.borrow_mut() = Some((phase, interrupt)));
    format!("`{}` stopped: {message}", phase.symbol())
}

/// Whether a phase on this thread was stopped and nothing has taken it yet.
pub(crate) fn interrupt_pending() -> bool {
    INTERRUPTED.with(|slot| slot.borrow().is_some())
}

/// The interrupt that stopped a phase on this thread since the last call.
pub(crate) fn take_interrupted() -> Option<(DefinitionPhase, Interrupt)> {
    INTERRUPTED.with(|slot| slot.borrow_mut().take())
}

/// The frontend's entry point, called by the `main` the backend emitted once
/// every definition's globals are in place.
///
/// Returns a process exit status: zero when the requested output was produced,
/// one when the definition rejected the program or the driver could not do
/// what it was told, and for `--emit run` the status of the run.
///
/// # Safety
/// `argv` must hold `argc` NUL-terminated strings, as the C runtime guarantees.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klffe_main(argc: c_int, argv: *const *const c_char) -> c_int {
    configure_allocator();
    // A driver is a process: a trap in its definition reports the trap and
    // fails with status 1, rather than printing a test outcome and exiting 0.
    klf_native_rt::klfrt_process_start(1);
    // SAFETY: the caller promises `argv` holds `argc` NUL-terminated strings.
    let arguments = unsafe { collect_arguments(argc, argv) };
    // SAFETY: as above.
    bundle::select_for_program(&unsafe { program_name(argc, argv) });
    let metadata = read_metadata();
    MATERIALIZES.with(|flags| *flags.borrow_mut() = metadata.materializes);
    let first = arguments.first().map(String::as_str);
    if metadata.shader.is_some() && !matches!(first, Some("lsp" | "lsp-worker" | "format")) {
        return shader::run(&metadata, &arguments);
    }
    let root = driver_cli::command_tree(&metadata);
    // Before the uniform interface parses anything, the definition sees the
    // original arguments, its own command words and the driver's alike
    // (KLF-RIDL section 5.5): `kira build <step>` is a command word the
    // definition gives a meaning of its own.
    let arguments = if metadata.declares(klf_defsvc::Hook::CompatDriver) {
        match compat_driver(&arguments) {
            Ok(request) => select_driver_arguments(&arguments, request),
            Err(message) => {
                klf_diag::err!("{}: error: {message}", metadata.driver);
                return 1;
            }
        }
    } else {
        arguments
    };
    match dispatch(&metadata, &root, &arguments) {
        Ok(status) => status,
        Err(Failure::Rejected(rendered)) => {
            klf_diag::raw_err!("{rendered}");
            1
        }
        Err(Failure::Driver(message)) => {
            klf_diag::err!("{}: error: {message}", metadata.driver);
            1
        }
        Err(Failure::Usage(rendered)) => {
            klf_diag::err!("{}: {rendered}", metadata.driver);
            64
        }
    }
}

/// Parses `arguments` against the driver's command tree and runs what they
/// select.
fn dispatch(
    metadata: &FrontendMetadata,
    root: &Command,
    arguments: &[String],
) -> Result<i32, Failure> {
    let driver_cli::FileModeSplit { rest, globs } =
        driver_cli::split_file_modes(arguments, &metadata.file_modes).map_err(Failure::Driver)?;
    let matches = match klf_args::parse(root, &rest) {
        Ok(Outcome::Run(matches)) => matches,
        Ok(Outcome::Help(path)) => {
            klf_diag::raw!("{}", klf_args::help(root, &path));
            return Ok(0);
        }
        Ok(Outcome::Version) => {
            klf_diag::out!("{} {}", metadata.driver, metadata.version);
            return Ok(0);
        }
        Ok(Outcome::Completion(shell)) => {
            klf_diag::raw!("{}", klf_args::completion_script(root, shell));
            return Ok(0);
        }
        Err(error) => return Err(Failure::Usage(error.render(root))),
    };
    match &matches.command(root).action {
        Action::Builtin(key) if key == "lsp" => Ok(editor::serve(metadata)),
        Action::Builtin(key) if key == "lsp-worker" => Ok(editor::worker(metadata)),
        Action::Builtin(key) if key == "format" => Ok(format::run(metadata, root, &matches, globs)),
        Action::Builtin(_) => {
            let invocation = driver_cli::invocation(root, &matches, metadata, globs);
            let Some(depfile) = invocation.depfile.clone() else {
                return build(metadata, invocation, arguments);
            };
            let target = invocation
                .output
                .clone()
                .unwrap_or_else(|| invocation.inputs[0].clone());
            klf_services::record_accesses();
            let status = build(metadata, invocation, arguments)?;
            depfile::write(&depfile, &target).map_err(Failure::Driver)?;
            Ok(status)
        }
        Action::Definition(index) => command::run(*index, matches.values).map_err(Failure::Driver),
        Action::None => {
            klf_diag::raw!("{}", klf_args::help(root, &matches.path));
            Ok(0)
        }
    }
}

/// Why the driver is exiting non-zero.
enum Failure {
    /// The definition rejected the program. The text is already a rendered
    /// diagnostic, so it is printed as it stands.
    Rejected(String),
    /// The driver could not do what it was told.
    Driver(String),
    /// The command line was wrong. The text is the rendered error.
    Usage(String),
}

/// # Safety
/// `argv` must hold `argc` NUL-terminated strings, as the C runtime guarantees.
unsafe fn program_name(argc: c_int, argv: *const *const c_char) -> String {
    if argv.is_null() || argc < 1 {
        return String::new();
    }
    // SAFETY: argv holds at least one entry, NUL-terminated.
    let pointer = unsafe { *argv };
    if pointer.is_null() {
        return String::new();
    }
    // SAFETY: as above.
    unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned()
}

/// # Safety
/// `argv` must hold `argc` NUL-terminated strings, as the C runtime guarantees.
unsafe fn collect_arguments(argc: c_int, argv: *const *const c_char) -> Vec<String> {
    let mut arguments = Vec::new();
    if argv.is_null() {
        return arguments;
    }
    for index in 1..argc.max(0) {
        // SAFETY: index stays inside the argc entries of argv.
        let pointer = unsafe { *argv.offset(index as isize) };
        if pointer.is_null() {
            continue;
        }
        // SAFETY: each argv entry is NUL-terminated.
        let text = unsafe { CStr::from_ptr(pointer) };
        arguments.push(text.to_string_lossy().into_owned());
    }
    arguments
}

fn read_metadata() -> FrontendMetadata {
    // SAFETY: `klffe_metadata` returns a pointer to a NUL-terminated constant
    // the backend defined in this binary's data section.
    let text = unsafe { CStr::from_ptr(klffe_metadata()) };
    FrontendMetadata::decode(&text.to_string_lossy())
}

/// Calls the definition's `compatDriver` override through the trampoline's
/// compatibility selector, over the same services the VM path installs.
fn compat_driver(arguments: &[String]) -> Result<Option<DriverRequest>, String> {
    let mut sink = ServiceSink::new();
    let call = CompatDriverCall::register(arguments.to_vec(), &mut sink);
    services::install(sink.into_table(), String::new());
    let count = i64::try_from(call.count()).unwrap_or(i64::MAX);
    // SAFETY: `klffe_phase` is emitted into this binary with this signature;
    // selector 4 calls the generated `compatDriver` entry with the count.
    let word = unsafe { klffe_phase(COMPAT_DRIVER_SELECTOR, count, 0, 0) };
    let reported = services::uninstall();
    if !reported.is_empty() {
        return Err("compatDriver reported a diagnostic".to_owned());
    }
    call.finish(word as u64)
}

/// Writes the `.ridl` contract of the library `module` lowers at `path`.
fn write_contract(
    module: &klf_kir::Module,
    name: &str,
    session: &Session,
    path: &std::path::Path,
    sources: bool,
) -> Result<(), Failure> {
    let text = match ridl_surface::contract(module) {
        Some(contract) => contract,
        None => match ridl_surface::described(module).map_err(Failure::Driver)? {
            Some(surface) => surface.text,
            None => klf_ridl::emit::emit(&ridl_surface::surface(module, name)),
        },
    };
    let text = if sources {
        ridl_sources::mark(&text, module, session, path)
    } else {
        text
    };
    std::fs::write(path, text)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
        .map_err(Failure::Driver)
}

fn build(
    metadata: &FrontendMetadata,
    invocation: arguments::Invocation,
    driver_arguments: &[String],
) -> Result<i32, Failure> {
    let triple = invocation
        .target
        .clone()
        .unwrap_or_else(|| metadata.target.clone());
    if matches!(invocation.emit.as_str(), "bin" | "lib" | "shared" | "run") {
        toolchain::check_target(&triple).map_err(Failure::Driver)?;
    }
    // The same status surface `klf` draws, from the same crate: a driver is a
    // client CLI too, and its phases are the ones the definition reports.
    let mut display =
        klf_diag::status::begin(&format!("Building {}", metadata.driver), invocation.timings);
    // A run kept under the cache root is reused whole while every path its
    // compile touched holds what it did.
    let reuses = invocation.emit == "run" && invocation.output.is_none();
    if reuses {
        klf_services::record_accesses();
    }
    let mut session = new_session(metadata, &invocation, &triple).map_err(Failure::Driver)?;
    let files = add_inputs(&mut session, metadata, &invocation).map_err(Failure::Driver)?;

    let mut make_runner = || Ok(NativePhases::new());
    let mut cache =
        module_cache::DiskModuleCache::open(metadata, &invocation, &triple, session.std_root());
    let program_key = cache.as_ref().filter(|_| reuses).map(|cache| {
        let mut key = cache.build_key();
        for input in &invocation.inputs {
            let input = std::path::absolute(input).unwrap_or_else(|_| input.clone());
            key.push_str(&format!("input {}\n", input.display()));
        }
        key
    });
    if let Some(key) = &program_key {
        let (executable, _) = run::executable_path(None, &invocation.inputs, &triple);
        if let Some(warnings) = program_cache::reusable(&executable, key) {
            if !warnings.is_empty() {
                klf_diag::raw_err!("{warnings}");
            }
            display.end();
            klf_diag::out!("Built: Ok {}", executable.display());
            return run::run(
                metadata.declares(klf_defsvc::Hook::RunProgram),
                &executable,
                &invocation.program_arguments,
                driver_arguments,
            )
            .map_err(Failure::Driver);
        }
    }
    let driven = match &mut cache {
        Some(cache) => drive_cached(&mut session, &files, &mut make_runner, cache),
        None => drive(&mut session, &files, &mut make_runner),
    };
    let module = driven.map_err(|error| match error {
        // A phase failure is the definition rejecting the program: the message
        // is the diagnostic it reported, rendered against the source.
        DriveError::Phase { message, .. } => Failure::Rejected(message),
        // Two units disagreeing is the compiler's fault, said as a diagnostic
        // of its own with what each unit holds.
        DriveError::Conflict(message) => Failure::Rejected(format!(
            "{}: error[KLF-KIR-9002]: {message}",
            metadata.driver
        )),
        other => Failure::Driver(other.to_string()),
    })?;
    let warnings = cache.as_ref().map_or_else(
        || render_kept_diagnostics(&session),
        |cache| cache.kept_diagnostics(&session),
    );
    if !warnings.is_empty() {
        klf_diag::raw_err!("{warnings}");
    }
    let level = optimization_level(&session);
    if invocation.emit == "check" {
        display.end();
        klf_diag::out!("Check: Ok");
        return Ok(0);
    }
    if let Some(device) = &invocation.device {
        display.end();
        return push::push(metadata, &module, device).map_err(Failure::Driver);
    }

    if invocation.emit == "run" {
        let (executable, temporary) =
            run::executable_path(invocation.output, &invocation.inputs, &triple);
        run::prepare(&executable).map_err(Failure::Driver)?;
        let links = program_key
            .as_ref()
            .and_then(|_| output::linked_archives(&module, &metadata.name, &triple));
        output::emit_binary(module, &executable, &metadata.name, metadata.entry, level)
            .map_err(Failure::Driver)?;
        if let (Some(key), Some(links)) = (&program_key, &links) {
            program_cache::keep(&executable, key, links, &warnings);
        }
        display.end();
        klf_diag::out!("Built: Ok {}", executable.display());
        let status = run::run(
            metadata.declares(klf_defsvc::Hook::RunProgram),
            &executable,
            &invocation.program_arguments,
            driver_arguments,
        );
        if temporary {
            run::remove(&executable);
        }
        return status.map_err(Failure::Driver);
    }
    let output = match invocation.output {
        Some(output) => output,
        None if invocation.emit == "bin" => {
            let input = &invocation.inputs[0];
            let stem = input.file_stem().ok_or_else(|| {
                Failure::Driver(format!(
                    "cannot derive output name from {}",
                    input.display()
                ))
            })?;
            input.with_file_name(stem)
        }
        None => {
            let mut path = invocation.inputs[0].clone();
            path.set_extension(match invocation.emit.as_str() {
                "klbc" => "klbc",
                "lib" => "a",
                "ridl" => "ridl",
                "shared" if toolchain::is_apple(&triple) => "dylib",
                "shared" => "so",
                _ => "kir",
            });
            path
        }
    };
    if invocation.emit == "bin" {
        output::emit_binary(module, &output, &metadata.name, metadata.entry, level)
            .map_err(Failure::Driver)?;
        display.end();
        klf_diag::out!("Built: Ok {}", output.display());
        return Ok(0);
    }
    if invocation.emit == "lib" {
        output::emit_library(
            &module,
            &output,
            &metadata.name,
            invocation.export.as_deref(),
        )
        .map_err(Failure::Driver)?;
        display.end();
        klf_diag::out!("Built: Ok {}", output.display());
        return Ok(0);
    }
    if invocation.emit == "ridl" {
        let written =
            write_contract(&module, &metadata.name, &session, &output, invocation.ridl_source);
        if written.is_ok() {
            display.end();
            klf_diag::out!("Built: Ok {}", output.display());
        }
        return written.map(|()| 0);
    }
    if invocation.emit == "shared" {
        if let Some(contract) = &invocation.contract {
            write_contract(&module, &metadata.name, &session, contract, invocation.ridl_source)?;
        }
        output::emit_shared(&module, &output, &metadata.name).map_err(Failure::Driver)?;
        display.end();
        klf_diag::out!("Built: Ok {}", output.display());
        return Ok(0);
    }
    let bytes = if invocation.emit == "klbc" {
        klf_diag::progress::report("lower bytecode");
        let mut program = klf_vm::lower_module(&module)
            .map_err(|error| format!("cannot lower KIR to bytecode: {error}"))
            .map_err(Failure::Driver)?;
        output::link_std_bytecode(&mut program, &module, &metadata.name)
            .map_err(Failure::Driver)?;
        vm_natives::emit_natives(&module, &mut program, &output).map_err(Failure::Driver)?;
        klf_vm::encode(&program)
    } else if level == klf_kir_passes::Level::O0 {
        module.to_string().into_bytes()
    } else {
        output::machine_module(module, level, &output)
            .map_err(Failure::Driver)?
            .to_string()
            .into_bytes()
    };
    let written = std::fs::write(&output, bytes)
        .map_err(|error| format!("cannot write {}: {error}", output.display()))
        .map_err(Failure::Driver);
    if written.is_ok() {
        display.end();
        klf_diag::out!("Built: Ok {}", output.display());
    }
    written.map(|()| 0)
}

/// The tier-1 level of section 14.1: a build asking for the KIR optimizer
/// runs O2, any other O0. A language asks for it with its `optimize` driver
/// flag.
fn optimization_level(session: &Session) -> klf_kir_passes::Level {
    match session.flag("optimize") {
        Some(klf_services::DriverFlagValue::Boolean(true)) => klf_kir_passes::Level::O2,
        _ => klf_kir_passes::Level::O0,
    }
}
