//! Running a definition's phases on the VM, over one machine per unit.
//!
//! A phase artifact is a definition-owned value that the kernel only holds a
//! token for, so the heap the value lives on has to outlive the phase that
//! produced it. `parse`, `elaborate` and `lower` for one unit therefore run as
//! separate entry points over a single [`Machine`], and the section 15.6
//! heap-balance check applies to the unit rather than to each call.

use crate::Backend;
use klf_defsvc::{
    COMMAND_ENTRY, COMPAT_DRIVER_ENTRY, CompatDriverCall, DefinitionServiceBridge, PhaseRunner,
    RUN_PROGRAM_ENTRY, RunProgramCall, ServiceSink, complete_phase, phase_materializes_artifact,
    prepare_phase, register_command_values, settle_phase_diagnostics,
};
use klf_services::{
    DefinitionImage, DefinitionPhase, DefinitionPhaseArguments, DefinitionPhaseResult,
    DriverRequest, Session,
};
use klf_vm::{
    Capabilities, Machine, Outcome, Program, kernel_services_with_source_and_diagnostics,
};

/// A definition image lowered to bytecode once, so every unit of a compilation
/// shares one lowering.
pub(crate) struct PhaseProgram {
    program: Program,
}

impl PhaseProgram {
    pub(crate) fn new(image: &DefinitionImage) -> Result<Self, String> {
        let module = klf_kir_passes::monomorphize(image.module());
        let program = klf_vm::lower_module(&module).map_err(|error| error.to_string())?;
        Ok(Self { program })
    }

    pub(crate) fn host<'a>(&'a self, image: &'a DefinitionImage) -> PhaseHost<'a> {
        PhaseHost::Vm {
            image,
            machine: Box::new(Machine::new(&self.program)),
            program: &self.program,
        }
    }
}

/// Where one unit's phases run. The VM variant keeps the machine alive across
/// the phases; other backends re-enter through the image each time and cannot
/// carry a definition-owned artifact.
pub(crate) enum PhaseHost<'a> {
    Vm {
        image: &'a DefinitionImage,
        machine: Box<Machine<'a>>,
        program: &'a Program,
    },
    Backend {
        image: &'a DefinitionImage,
        backend: Backend,
    },
}

impl PhaseRunner for PhaseHost<'_> {
    fn balanced(&self) -> bool {
        match self {
            Self::Vm { machine, .. } => machine.balanced(),
            Self::Backend { .. } => true,
        }
    }

    fn run(
        &mut self,
        phase: DefinitionPhase,
        session: &mut Session,
        arguments: &DefinitionPhaseArguments,
    ) -> Result<DefinitionPhaseResult, String> {
        match self {
            Self::Backend { image, backend } => {
                crate::run_definition_phase_result(image, phase, session, arguments, *backend)
            }
            Self::Vm {
                image,
                machine,
                program,
            } => run_on_machine(machine, program, image, phase, session, arguments),
        }
    }
}

/// Moves a built service table into the VM's capability table.
pub(crate) fn install(bridge: &DefinitionServiceBridge, capabilities: &mut Capabilities) {
    let mut sink = ServiceSink::new();
    bridge.register(&mut sink);
    for (name, handler) in sink.into_table() {
        capabilities.register_with_memory(name, move |arguments, memory| {
            handler.call_with_memory(arguments, memory)
        });
    }
}

fn run_on_machine(
    machine: &mut Machine<'_>,
    program: &Program,
    image: &DefinitionImage,
    phase: DefinitionPhase,
    session: &mut Session,
    arguments: &DefinitionPhaseArguments,
) -> Result<DefinitionPhaseResult, String> {
    let entry = program
        .function_index(phase.symbol())
        .ok_or_else(|| format!("no function named `{}`", phase.symbol()))?;
    let materializes = phase_materializes_artifact(image, phase);
    let (bridge, words, source) = prepare_phase(session, phase, arguments, materializes)?;
    let file = bridge.file;
    let (mut capabilities, _, diagnostics) = kernel_services_with_source_and_diagnostics(source);
    install(&bridge, &mut capabilities);
    let outcome = machine.call(entry, &words, &capabilities);
    drop(capabilities);
    if matches!(&outcome, Outcome::Trap(code) if code == klf_vm::CANCELLED) {
        // A cancelled phase's records go with it (`KLF-EDITOR.md` 8.5).
        return Err(format!("`{}` cancelled", phase.symbol()));
    }
    bridge.commit_records(session)?;
    let reported = diagnostics
        .lock()
        .map_err(|_| "runtime diagnostics lock poisoned".to_owned())?
        .clone();
    settle_phase_diagnostics(session, file, &reported)?;
    let word = match outcome {
        Outcome::Return(word) => word,
        Outcome::Fail(value) => return Err(format!("definition phase failed with {value}")),
        Outcome::Trap(message) => return Err(message),
    };
    complete_phase(bridge, session, phase, arguments, word)
}

/// Calls the definition's `compatDriver` override on the VM with the original
/// process arguments. `None` means the definition has no override or the
/// override kept the original arguments.
pub(crate) fn run_compat_driver(
    image: &DefinitionImage,
    arguments: &[String],
) -> Result<Option<DriverRequest>, String> {
    let program = PhaseProgram::new(image)?;
    let Some(entry) = program.program.function_index(COMPAT_DRIVER_ENTRY) else {
        return Ok(None);
    };
    let mut sink = ServiceSink::new();
    let call = CompatDriverCall::register(arguments.to_vec(), &mut sink);
    let capabilities = driver_capabilities(sink);
    let mut machine = Machine::new(&program.program);
    match machine.call(entry, &[call.count()], &capabilities) {
        Outcome::Return(Some(word)) => call.finish(word),
        Outcome::Return(None) => Err("compatDriver entry returned no value".to_owned()),
        Outcome::Fail(value) => Err(format!("compatDriver failed with {value}")),
        Outcome::Trap(message) => Err(format!("compatDriver trapped: {message}")),
    }
}

/// Calls the definition's `project` hook on the VM for `path`. `None` means
/// the definition has no hook or the hook leaves the file to the kernel's
/// rule (`KLF-EDITOR.md` section 9.2).
pub(crate) fn run_project(
    image: &DefinitionImage,
    session: &mut Session,
    path: &str,
) -> Result<Option<klf_defsvc::HookProject>, String> {
    let program = PhaseProgram::new(image)?;
    let Some(entry) = program.program.function_index(klf_defsvc::PROJECT_ENTRY) else {
        return Ok(None);
    };
    let call = klf_defsvc::ProjectCall::open(session, path)?;
    let (mut capabilities, _, diagnostics) =
        kernel_services_with_source_and_diagnostics(call.source.clone());
    let mut sink = ServiceSink::new();
    call.register(&mut sink);
    for (name, handler) in sink.into_table() {
        capabilities.register_with_memory(name, move |words, memory| {
            handler.call_with_memory(words, memory)
        });
    }
    let mut machine = Machine::new(&program.program);
    let outcome = machine.call(entry, &call.words, &capabilities);
    drop(capabilities);
    let reported = diagnostics
        .lock()
        .map_err(|_| "runtime diagnostics lock poisoned".to_owned())?
        .clone();
    match outcome {
        Outcome::Return(Some(word)) => call.finish(session, &reported, word),
        Outcome::Return(None) => Err("project entry returned no value".to_owned()),
        Outcome::Fail(value) => Err(format!("project failed with {value}")),
        Outcome::Trap(message) => Err(format!("project trapped: {message}")),
    }
}

/// Calls the definition's `runProgram` hook on the VM with the command that
/// runs a built program and the driver arguments of the build, and answers
/// the exit status it returns.
pub(crate) fn run_program(
    image: &DefinitionImage,
    command: &[String],
    driver_arguments: &[String],
) -> Result<i32, String> {
    let program = PhaseProgram::new(image)?;
    let entry = program
        .program
        .function_index(RUN_PROGRAM_ENTRY)
        .ok_or_else(|| "the definition has no runProgram entry".to_owned())?;
    let mut sink = ServiceSink::new();
    let call = RunProgramCall::register(command, driver_arguments, &mut sink);
    let capabilities = driver_capabilities(sink);
    let mut machine = Machine::new(&program.program);
    match machine.call(entry, &call.words(), &capabilities) {
        Outcome::Return(Some(word)) => Ok(RunProgramCall::status(word)),
        Outcome::Return(None) => Err("runProgram entry returned no value".to_owned()),
        Outcome::Fail(value) => Err(format!("runProgram failed with {value}")),
        Outcome::Trap(message) => Err(format!("runProgram trapped: {message}")),
    }
}

/// Calls the definition's `__klf_command` entry on the VM for the command
/// numbered `index`, serving it `values`, and answers the exit status its
/// `run` returns.
pub(crate) fn run_command(
    image: &DefinitionImage,
    index: u32,
    values: Vec<Vec<klf_args::Value>>,
) -> Result<i32, String> {
    let program = PhaseProgram::new(image)?;
    let entry = program
        .program
        .function_index(COMMAND_ENTRY)
        .ok_or_else(|| "the definition has no runnable command".to_owned())?;
    let mut sink = ServiceSink::new();
    register_command_values(values, &mut sink);
    let capabilities = driver_capabilities(sink);
    let mut machine = Machine::new(&program.program);
    match machine.call(entry, &[u64::from(index)], &capabilities) {
        Outcome::Return(Some(word)) => Ok(i32::try_from(word.cast_signed()).unwrap_or(1)),
        Outcome::Return(None) => Err("command entry returned no value".to_owned()),
        Outcome::Fail(value) => Err(format!("command failed with {value}")),
        Outcome::Trap(message) => Err(format!("command trapped: {message}")),
    }
}

/// Calls the `@Test` function `name` on the VM, serving it `sink`'s
/// `klf.test.*` services.
pub(crate) fn run_test(
    program: &PhaseProgram,
    name: &str,
    sink: ServiceSink,
) -> Result<(), String> {
    let entry = program
        .program
        .function_index(name)
        .ok_or_else(|| format!("the image has no test function `{name}`"))?;
    let capabilities = driver_capabilities(sink);
    let mut machine = Machine::new(&program.program);
    match machine.call(entry, &[], &capabilities) {
        Outcome::Return(_) => Ok(()),
        Outcome::Fail(value) => Err(format!("the test failed with {value}")),
        Outcome::Trap(message) => Err(format!("the test trapped: {message}")),
    }
}

/// The kernel services with a driver call's own on top.
fn driver_capabilities(sink: ServiceSink) -> Capabilities {
    let (mut capabilities, _, _) = kernel_services_with_source_and_diagnostics(Vec::new());
    for (name, handler) in sink.into_table() {
        capabilities.register_with_memory(name, move |words, memory| {
            handler.call_with_memory(words, memory)
        });
    }
    capabilities
}

/// Only the VM backend hosts a definition's own heap across phases.
pub(crate) const fn hosts_artifacts(backend: Backend) -> bool {
    matches!(backend, Backend::Vm)
}

impl klf_defsvc::FormatRunner for PhaseHost<'_> {
    fn format(
        &mut self,
        session: &mut Session,
        arguments: &DefinitionPhaseArguments,
        syntax: klf_format::Syntax,
    ) -> Result<(klf_format::Builder, bool), String> {
        let Self::Vm {
            machine, program, ..
        } = self
        else {
            return Err("only the VM backend hosts a format hook".to_owned());
        };
        let entry = program
            .function_index(klf_defsvc::FORMAT_ENTRY)
            .ok_or_else(|| "the definition has no format hook".to_owned())?;
        let (bridge, words, source) = klf_defsvc::open_format(session, arguments, syntax)?;
        let (mut capabilities, _, diagnostics) =
            kernel_services_with_source_and_diagnostics(source);
        install(&bridge, &mut capabilities);
        let outcome = machine.call(entry, &words, &capabilities);
        drop(capabilities);
        let reported = diagnostics
            .lock()
            .map_err(|_| "runtime diagnostics lock poisoned".to_owned())?
            .clone();
        let word = match outcome {
            Outcome::Return(word) => word.unwrap_or(0),
            Outcome::Fail(value) => return Err(format!("format failed with {value}")),
            Outcome::Trap(message) => return Err(format!("format trapped: {message}")),
        };
        klf_defsvc::close_format(&bridge, session, &reported, word)
    }
}
