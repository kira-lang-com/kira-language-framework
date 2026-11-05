//! Driving a definition's four phases over a set of input files.
//!
//! The order (`units`, then `parse`/`elaborate`/`lower` per unit), the rule
//! that one unit's phases share one artifact heap, the merge of the per-unit
//! modules and the section 7.5 rule 4 verification are properties of the
//! `Language` contract, not of any one backend. They live here so the
//! VM-hosted driver and a natively compiled frontend run the same schedule.

use crate::{DefinitionServiceBridge, DefinitionServiceState};
use klf_core::{FileId, Name};
use klf_diag::progress;
use klf_kir::{Function, Module};
use klf_services::{
    DefinitionPhase, DefinitionPhaseArguments, DefinitionPhaseResult, DefinitionUnit, Session,
};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

/// What runs definition phases for one compilation. A runner is reused for the
/// units it lowers; [`PhaseRunner::balanced`] closes one unit's temporary
/// artifact frame before the next unit starts.
pub trait PhaseRunner {
    /// # Errors
    /// Returns a rendered message when the phase fails or reports errors.
    fn run(
        &mut self,
        phase: DefinitionPhase,
        session: &mut Session,
        arguments: &DefinitionPhaseArguments,
    ) -> Result<DefinitionPhaseResult, String>;

    /// Whether the unit's phases freed every tracked block they allocated.
    fn balanced(&self) -> bool {
        true
    }
}

#[derive(Debug)]
pub enum DriveError {
    NoInputs,
    Runner(String),
    Phase {
        phase: DefinitionPhase,
        message: String,
    },
    Mismatch(DefinitionPhase),
    Leaked,
    Conflict(String),
    Verify(Vec<String>),
}

impl fmt::Display for DriveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoInputs => formatter.write_str("a definition needs at least one input file"),
            Self::Runner(message) | Self::Conflict(message) => formatter.write_str(message),
            Self::Phase { phase, message } => {
                write!(formatter, "phase `{}` failed: {message}", phase.symbol())
            }
            Self::Mismatch(phase) => write!(
                formatter,
                "phase `{}` returned the wrong value",
                phase.symbol()
            ),
            Self::Leaked => formatter.write_str("leaked a heap block while compiling"),
            Self::Verify(failures) => write!(formatter, "{}", failures.join("; ")),
        }
    }
}

/// Runs `units` and then one full pipeline per unit, merging and verifying.
///
/// `make_runner` is called once for the whole compilation. One unit's `parse`
/// allocates the tree its `lower` reads back, so the runner keeps its backend
/// and artifact heap alive across those phases, then `balanced` closes that
/// unit's temporary frame before the next one begins.
///
/// # Errors
/// Returns [`DriveError`] when a phase fails, returns the wrong handle, leaks,
/// produces conflicting definitions across units, or emits KIR the raw verifier
/// rejects.
pub fn drive<R: PhaseRunner>(
    session: &mut Session,
    inputs: &[FileId],
    make_runner: &mut dyn FnMut() -> Result<R, String>,
) -> Result<Module, DriveError> {
    let mut runner = make_runner().map_err(DriveError::Runner)?;
    let units = run_units_with_runner(session, inputs, &mut runner)?;
    progress!("compilation graph units={} phase_runners=1", units.len());
    let module = lower_units(session, units, &mut runner)?;
    progress!("verify");
    verify(&module)?;
    Ok(module)
}

mod cache;
mod merge;
pub use cache::{Decode, Kept, ModuleCache, Stored, UnitTrace, drive_cached};
pub(crate) use merge::merge_units;
pub(crate) use merge::{same_operation, same_parameters};

/// One full pipeline per unit, merged into one module.
fn lower_units<R: PhaseRunner>(
    session: &mut Session,
    units: Vec<DefinitionUnit>,
    runner: &mut R,
) -> Result<Module, DriveError> {
    let program = program_files(&units);
    let mut lowered = Vec::new();
    for unit in units {
        lowered.push(lower_one(session, unit, &program, runner)?);
    }
    merge_units(session, lowered)
}

/// The files of every unit the `units` phase returned, in its order: the
/// program each unit's parse reads the other packages of.
fn program_files(units: &[DefinitionUnit]) -> Vec<FileId> {
    units.iter().map(DefinitionUnit::file).collect()
}

/// One unit's full pipeline, on a runner of its own.
fn lower_one<R: PhaseRunner>(
    session: &mut Session,
    unit: DefinitionUnit,
    program: &[FileId],
    runner: &mut R,
) -> Result<Module, DriveError> {
    let DefinitionPhaseResult::Module(module) = lower_unit(runner, session, unit, program)? else {
        return Err(DriveError::Mismatch(DefinitionPhase::Lower));
    };
    if !runner.balanced() {
        return Err(DriveError::Leaked);
    }
    Ok(*module)
}

/// The schedule of a `ShaderLanguage` (`KLF-KSL.md` section 4.4): `units`,
/// then one pipeline per unit, each lowering to its own KSIR module. Shader
/// modules link nothing, so none is merged.
///
/// # Errors
/// Returns [`DriveError`] when a phase fails, returns the wrong handle or
/// leaks.
pub fn drive_shaders<R: PhaseRunner>(
    session: &mut Session,
    inputs: &[FileId],
    make_runner: &mut dyn FnMut() -> Result<R, String>,
) -> Result<Vec<(DefinitionUnit, klf_shader::Module)>, DriveError> {
    let mut runner = make_runner().map_err(DriveError::Runner)?;
    let mut modules = Vec::new();
    let units = run_units_with_runner(session, inputs, &mut runner)?;
    progress!("shader graph units={} phase_runners=1", units.len());
    let program = program_files(&units);
    for unit in units {
        let DefinitionPhaseResult::Shader(module) =
            lower_unit(&mut runner, session, unit.clone(), &program)?
        else {
            return Err(DriveError::Mismatch(DefinitionPhase::Lower));
        };
        if !runner.balanced() {
            return Err(DriveError::Leaked);
        }
        modules.push((unit, *module));
    }
    Ok(modules)
}

/// Runs the `units` phase and checks the units it returned.
pub(crate) fn run_units<R: PhaseRunner>(
    session: &mut Session,
    inputs: &[FileId],
    make_runner: &mut dyn FnMut() -> Result<R, String>,
) -> Result<Vec<DefinitionUnit>, DriveError> {
    let mut runner = make_runner().map_err(DriveError::Runner)?;
    run_units_with_runner(session, inputs, &mut runner)
}

pub(crate) fn run_units_with_runner<R: PhaseRunner>(
    session: &mut Session,
    inputs: &[FileId],
    runner: &mut R,
) -> Result<Vec<DefinitionUnit>, DriveError> {
    if inputs.is_empty() {
        return Err(DriveError::NoInputs);
    }
    let result = run_phase(
        runner,
        session,
        DefinitionPhase::Units,
        &DefinitionPhaseArguments::Units {
            inputs: inputs.to_vec(),
        },
    )?;
    let DefinitionPhaseResult::Units(units) = result else {
        return Err(DriveError::Mismatch(DefinitionPhase::Units));
    };
    if !runner.balanced() {
        return Err(DriveError::Leaked);
    }
    if units.is_empty() || units.iter().any(|unit| session.unit_id(unit).is_none()) {
        return Err(DriveError::Mismatch(DefinitionPhase::Units));
    }
    Ok(units)
}

fn run_phase<R: PhaseRunner>(
    runner: &mut R,
    session: &mut Session,
    phase: DefinitionPhase,
    arguments: &DefinitionPhaseArguments,
) -> Result<DefinitionPhaseResult, DriveError> {
    progress!("{}", phase_label(session, phase, arguments));
    runner
        .run(phase, session, arguments)
        .map_err(|message| DriveError::Phase { phase, message })
}

/// What a phase calls itself in the command's progress: its own name and the
/// unit it is running on, so a build over a package says which file each row is
/// about rather than repeating `parse` four times.
fn phase_label(
    session: &Session,
    phase: DefinitionPhase,
    arguments: &DefinitionPhaseArguments,
) -> String {
    let unit = match arguments {
        DefinitionPhaseArguments::Units { .. } => return phase.symbol().to_owned(),
        DefinitionPhaseArguments::Parse { unit, .. }
        | DefinitionPhaseArguments::Elaborate { unit, .. }
        | DefinitionPhaseArguments::Lower { unit, .. } => unit,
    };
    let file = unit.file();
    if session.has_file(file) {
        format!("{} {}", phase.symbol(), session.file(file).name())
    } else {
        phase.symbol().to_owned()
    }
}

fn lower_unit<R: PhaseRunner>(
    runner: &mut R,
    session: &mut Session,
    unit: DefinitionUnit,
    program: &[FileId],
) -> Result<DefinitionPhaseResult, DriveError> {
    let parsed = run_phase(
        runner,
        session,
        DefinitionPhase::Parse,
        &DefinitionPhaseArguments::Parse {
            unit: unit.clone(),
            program: program.to_vec(),
        },
    )?;
    let DefinitionPhaseResult::Artifact(tree) = parsed else {
        return Err(DriveError::Mismatch(DefinitionPhase::Parse));
    };
    if tree.unit() != unit {
        return Err(DriveError::Mismatch(DefinitionPhase::Parse));
    }

    let elaborated = run_phase(
        runner,
        session,
        DefinitionPhase::Elaborate,
        &DefinitionPhaseArguments::Elaborate {
            unit: unit.clone(),
            tree,
        },
    )?;
    let DefinitionPhaseResult::Artifact(program) = elaborated else {
        return Err(DriveError::Mismatch(DefinitionPhase::Elaborate));
    };
    if program.unit() != unit {
        return Err(DriveError::Mismatch(DefinitionPhase::Elaborate));
    }

    run_phase(
        runner,
        session,
        DefinitionPhase::Lower,
        &DefinitionPhaseArguments::Lower { unit, program },
    )
}

/// Merges the units' modules into one. A language returns its units
/// dependencies first, so the last is the program's own and names the merged
/// module, and a unit's functions come after those of the units it depends
/// on: initializers run in that order, a package's before its dependents'.
fn verify(module: &Module) -> Result<(), DriveError> {
    verify_where(module, |_| true)
}

fn verify_where(
    module: &Module,
    chosen: impl Fn(&Function) -> bool + Sync,
) -> Result<(), DriveError> {
    let failures = klf_kir_verify::verify_module_functions(module, chosen);
    if failures.is_empty() {
        return Ok(());
    }
    // A definition author debugging a frontend can ask for the module that
    // failed, since nothing else writes it out.
    if let Some(path) = std::env::var_os("KLF_DUMP_UNVERIFIED") {
        let _ = std::fs::write(path, module.to_string());
    }
    Err(DriveError::Verify(
        failures
            .into_iter()
            .map(|failure| {
                let place: String = failure
                    .notes
                    .iter()
                    .flat_map(|note| [" (", note.message.as_str(), ")"])
                    .collect();
                format!(
                    "produced KIR that fails the raw verifier: [{}] {}{place}",
                    failure.code, failure.message
                )
            })
            .collect(),
    ))
}

/// Opens one phase call: marshals the handle arguments, reserves the Session
/// artifact and builds the service table the phase code will call.
///
/// # Errors
/// Returns a message when an argument handle is not registered in `session`.
pub fn prepare_phase(
    session: &mut Session,
    phase: DefinitionPhase,
    arguments: &DefinitionPhaseArguments,
    materializes: bool,
) -> Result<(DefinitionServiceBridge, Vec<u64>, String), String> {
    if arguments.phase() != phase {
        return Err(format!(
            "phase `{}` was given `{}` arguments",
            phase.symbol(),
            arguments.phase().symbol()
        ));
    }
    let words = arguments
        .marshal(session)
        .map_err(|error| error.to_string())?;
    let bridge = DefinitionServiceBridge::new(session, phase, arguments, materializes)?;
    let source = bridge
        .file
        .map(|file| session.file(file).text().to_owned())
        .unwrap_or_default();
    Ok((bridge, words, source))
}

/// Closes one phase call: commits what the phase built and resolves its return
/// handle into a typed result.
///
/// # Errors
/// Returns a message when the bridge is still borrowed, the phase did not
/// materialize its artifact, or the handle it returned is not valid.
pub fn complete_phase(
    bridge: DefinitionServiceBridge,
    session: &mut Session,
    phase: DefinitionPhase,
    arguments: &DefinitionPhaseArguments,
    word: Option<u64>,
) -> Result<DefinitionPhaseResult, String> {
    let state: Mutex<DefinitionServiceState> = Arc::try_unwrap(bridge.state)
        .map_err(|_| "definition service bridge is still in use".to_owned())?;
    let state = state
        .into_inner()
        .map_err(|_| "definition service bridge lock poisoned".to_owned())?;
    let word = state.finish(session, word)?;
    DefinitionPhaseResult::take_return_word(phase, session, arguments, word)
        .map_err(|error| error.to_string())
}
