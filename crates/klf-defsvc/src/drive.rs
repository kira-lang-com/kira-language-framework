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

/// What runs one phase of one unit. The implementation owns the call into the
/// definition's compiled code and the transport of the diagnostics it reports.
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
/// `make_runner` is called once for the `units` phase and once per unit: one
/// unit's `parse` allocates the tree its `lower` reads back, so the artifact
/// heap has to stay alive across exactly those three calls.
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
    let units = run_units(session, inputs, make_runner)?;
    let module = lower_units(session, units, make_runner)?;
    progress!("verify");
    verify(&module)?;
    Ok(module)
}

mod cache;
pub use cache::{Decode, Kept, ModuleCache, Stored, UnitTrace, drive_cached};


/// One full pipeline per unit, merged into one module.
fn lower_units<R: PhaseRunner>(
    session: &mut Session,
    units: Vec<DefinitionUnit>,
    make_runner: &mut dyn FnMut() -> Result<R, String>,
) -> Result<Module, DriveError> {
    let program = program_files(&units);
    let mut lowered = Vec::new();
    for unit in units {
        lowered.push(lower_one(session, unit, &program, make_runner)?);
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
    make_runner: &mut dyn FnMut() -> Result<R, String>,
) -> Result<Module, DriveError> {
    let mut runner = make_runner().map_err(DriveError::Runner)?;
    let DefinitionPhaseResult::Module(module) = lower_unit(&mut runner, session, unit, program)? else {
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
    let mut modules = Vec::new();
    let units = run_units(session, inputs, make_runner)?;
    let program = program_files(&units);
    for unit in units {
        let mut runner = make_runner().map_err(DriveError::Runner)?;
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
    if inputs.is_empty() {
        return Err(DriveError::NoInputs);
    }
    let mut runner = make_runner().map_err(DriveError::Runner)?;
    let result = run_phase(
        &mut runner,
        session,
        DefinitionPhase::Units,
        &DefinitionPhaseArguments::Units {
            inputs: inputs.to_vec(),
        },
    )?;
    let DefinitionPhaseResult::Units(units) = result else {
        return Err(DriveError::Mismatch(DefinitionPhase::Units));
    };
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
fn merge_units(session: &Session, modules: Vec<Module>) -> Result<Module, DriveError> {
    let name = modules
        .last()
        .ok_or(DriveError::Mismatch(DefinitionPhase::Units))?
        .name
        .clone();
    let mut units = modules.into_iter();
    let first = units
        .next()
        .ok_or(DriveError::Mismatch(DefinitionPhase::Units))?;
    let mut merged = Merged::new(first);
    for module in units {
        merged.add(session, module)?;
    }
    merged.module.name = name;
    crate::evaluation::fold_deferred(&mut merged.module).map_err(|message| DriveError::Phase {
        phase: DefinitionPhase::Lower,
        message,
    })?;
    Ok(merged.module)
}

/// The program being built from the units, with where each function, type
/// and global of it sits.
struct Merged {
    module: Module,
    functions: HashMap<Name, usize>,
    types: HashMap<Name, usize>,
    globals: HashMap<Name, usize>,
    // The unit each function was taken from, for a conflict to name.
    origins: HashMap<Name, Name>,
}

impl Merged {
    fn new(module: Module) -> Self {
        let functions = positions(module.functions.iter().map(|function| &function.name));
        let origins = module
            .functions
            .iter()
            .map(|function| (function.name.clone(), module.name.clone()))
            .collect();
        let types = positions(module.types.iter().map(|declaration| &declaration.name));
        let globals = positions(module.globals.iter().map(|global| &global.name));
        Self {
            module,
            functions,
            types,
            globals,
            origins,
        }
    }

    /// Merges one unit's KIR into the program.
    ///
    /// A unit that only declares a function emits a body-less declaration,
    /// so a name may appear in several units; the one definition of it wins.
    /// Two definitions of the same name are the error.
    fn add(&mut self, session: &Session, extra: Module) -> Result<(), DriveError> {
        let base = &mut self.module;
        let unit = extra.name.clone();
        for function in extra.functions {
            let Some(&index) = self.functions.get(&function.name) else {
                self.functions.insert(function.name.clone(), base.functions.len());
                self.origins.insert(function.name.clone(), unit.clone());
                base.functions.push(function);
                continue;
            };
            if base.functions[index].foreign {
                // A unit that does not reach a function declares it by name
                // alone; any other unit's form of it says more.
                if !function.foreign || by_name(&base.functions[index]) {
                    self.origins.insert(function.name.clone(), unit.clone());
                    base.functions[index] = function;
                }
            } else if !function.foreign
                && !same_function_definition(&base.functions[index], &function)
            {
                // A unit gives a function it lowers but never reaches an
                // empty body without a signature; the unit that reaches it
                // defines it.
                if unreached_stub(&function) {
                    continue;
                }
                if unreached_stub(&base.functions[index]) {
                    self.origins.insert(function.name.clone(), unit.clone());
                    base.functions[index] = function;
                    continue;
                }
                let first = self.origins.get(&function.name).map_or("", Name::as_str);
                return Err(DriveError::Conflict(crate::unit_conflict::function_conflict(
                    session,
                    diagnostic_function_name(function.name.as_str()),
                    &crate::unit_conflict::Holder { unit: first, function: &base.functions[index] },
                    &crate::unit_conflict::Holder { unit: unit.as_str(), function: &function },
                )));
            }
        }
        for declaration in extra.types {
            let Some(&index) = self.types.get(&declaration.name) else {
                self.types.insert(declaration.name.clone(), base.types.len());
                base.types.push(declaration);
                continue;
            };
            // The same type declared by two units is one type, as long as the
            // units agree on what it is. A unit that only names a struct
            // agrees with the unit that completes it.
            if declaration.is_incomplete() {
                continue;
            }
            if base.types[index].is_incomplete() {
                base.types[index] = declaration;
                continue;
            }
            if !base.types[index].describes_same(&declaration) {
                if std::env::var_os("KLF_DEBUG_MERGE").is_some() {
                    let cases = |declaration: &klf_kir::TypeDeclaration| -> Vec<String> {
                        declaration.variants.iter().map(|variant| format!("{}/{:?}", variant.name.as_str(), variant.payload)).collect()
                    };
                    eprintln!(
                        "KLF_DEBUG_MERGE {}: unit `{}` cases {:?} against earlier cases {:?} | kind {:?}/{:?} generics {:?}/{:?} fields {:?}/{:?} layout {:?}/{:?}",
                        declaration.name.as_str(),
                        extra.name,
                        cases(&declaration),
                        cases(&base.types[index]),
                        declaration.kind,
                        base.types[index].kind,
                        declaration.generics,
                        base.types[index].generics,
                        declaration.fields.len(),
                        base.types[index].fields.len(),
                        declaration.layout,
                        base.types[index].layout,
                    );
                }
                return Err(DriveError::Conflict(crate::unit_conflict::type_conflict(
                    session,
                    &base.types[index],
                    &declaration,
                )));
            }
        }
        for global in extra.globals {
            let Some(&index) = self.globals.get(&global.name) else {
                self.globals.insert(global.name.clone(), base.globals.len());
                base.globals.push(global);
                continue;
            };
            // A C unit that only declares a global (`extern T x;`, or a
            // tentative `T x;`) holds zeroed storage for it; the unit that
            // defines it with an initializer gives the program its storage.
            if global.init == klf_kir::GlobalInit::Zero && base.globals[index].init != klf_kir::GlobalInit::Zero {
                continue;
            }
            if base.globals[index].init == klf_kir::GlobalInit::Zero && global.init != klf_kir::GlobalInit::Zero {
                base.globals[index] = global;
                continue;
            }
            if global.init == klf_kir::GlobalInit::Zero
                && base.globals[index].init == klf_kir::GlobalInit::Zero
                && global.ty != base.globals[index].ty
            {
                // A C declaration of an array of unknown bound names its
                // element; the definition gives the bound.
                if declared_storage(&global.ty) > declared_storage(&base.globals[index].ty) {
                    base.globals[index] = global;
                }
                continue;
            }
            if base.globals[index] != global {
                return Err(DriveError::Conflict(format!(
                    "defines global `{}` in more than one unit",
                    crate::unit_conflict::source_name(global.name.as_str())
                )));
            }
        }
        // Each unit records what it links, imports and witnesses itself.
        extend_distinct_hashed(&mut base.attributes, extra.attributes);
        // Two units that build the same instance record its witnesses alike.
        extend_distinct(&mut base.witnesses, extra.witnesses);
        extend_distinct(&mut base.vtables, extra.vtables);
        extend_distinct(&mut base.traits, extra.traits);
        extend_distinct(&mut base.conformances, extra.conformances);
        extend_distinct(&mut base.imports, extra.imports);
        extend_distinct(&mut base.exports, extra.exports);
        Ok(())
    }
}

/// How many elements of storage a global's type declares: an array's bound,
/// one for anything else.
fn declared_storage(ty: &klf_kir::Type) -> u64 {
    match ty {
        klf_kir::Type::Array(count, _) => *count,
        _ => 1,
    }
}

fn positions<'a>(names: impl Iterator<Item = &'a Name>) -> HashMap<Name, usize> {
    names
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect()
}

/// A declaration without a signature: what a unit declares of a function it
/// does not reach.
fn by_name(function: &Function) -> bool {
    function.parameters.is_empty()
        && function.result == klf_kir::Type::Unit
        && function.c_prototype.is_none()
}

/// The empty body a unit gives a function nothing in it reaches: no
/// signature, and one block that only returns.
fn unreached_stub(function: &Function) -> bool {
    let [block] = function.blocks.as_slice() else {
        return false;
    };
    let returns = |instruction: &klf_kir::Instruction| {
        matches!(instruction.operation, klf_kir::Operation::Return(None))
    };
    by_name(function)
        && block.parameters.is_empty()
        && matches!(block.instructions.as_slice(), [only] if returns(only))
}

fn extend_distinct<T: PartialEq>(base: &mut Vec<T>, extra: Vec<T>) {
    for item in extra {
        if !base.contains(&item) {
            base.push(item);
        }
    }
}

/// [`extend_distinct`] for attributes: units of one program share most of
/// theirs and carry thousands, so the ones already there are looked up by
/// hash.
fn extend_distinct_hashed<T: Eq + std::hash::Hash + Clone>(base: &mut Vec<T>, extra: Vec<T>) {
    let mut seen: std::collections::HashSet<T> = base.iter().cloned().collect();
    for item in extra {
        if seen.insert(item.clone()) {
            base.push(item);
        }
    }
}

fn diagnostic_function_name(name: &str) -> &str {
    let Some((base, suffix)) = name.rsplit_once('#') else {
        return name;
    };
    if !base.is_empty() && !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        base
    } else {
        name
    }
}

fn same_function_definition(left: &Function, right: &Function) -> bool {
    left.name == right.name
        && left.linkage == right.linkage
        && left.abi == right.abi
        && left.nounwind == right.nounwind
        && left.foreign == right.foreign
        && left.generics == right.generics
        && left.attributes == right.attributes
        && same_parameters(&left.parameters, &right.parameters)
        && left.result == right.result
        && left.failure == right.failure
        && left.engine == right.engine
        && same_blocks(&left.blocks, &right.blocks)
}

fn same_blocks(left: &[klf_kir::Block], right: &[klf_kir::Block]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut anonymous = Vec::new();
    left.iter().zip(right).all(|(left, right)| {
        left.name == right.name
            && same_parameters(&left.parameters, &right.parameters)
            && same_instructions(&left.instructions, &right.instructions, &mut anonymous)
    })
}

fn same_instructions(
    left: &[klf_kir::Instruction],
    right: &[klf_kir::Instruction],
    anonymous: &mut Vec<(String, String)>,
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.results == right.results
                && same_operation(&left.operation, &right.operation, anonymous)
        })
}

pub(crate) fn same_operation(
    left: &klf_kir::Operation,
    right: &klf_kir::Operation,
    anonymous: &mut Vec<(String, String)>,
) -> bool {
    match (left, right) {
        (
            klf_kir::Operation::GlobalAddress {
                global: left,
                thread_local: left_tls,
            },
            klf_kir::Operation::GlobalAddress {
                global: right,
                thread_local: right_tls,
            },
        ) if left.as_str().starts_with("anon.") && right.as_str().starts_with("anon.") => {
            if left_tls != right_tls {
                return false;
            }
            if let Some((_, mapped)) = anonymous.iter().find(|(name, _)| name == left.as_str()) {
                return mapped == right.as_str();
            }
            anonymous.push((left.to_string(), right.to_string()));
            true
        }
        _ => left == right,
    }
}

pub(crate) fn same_parameters(left: &[klf_kir::Parameter], right: &[klf_kir::Parameter]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.name == right.name
                && left.convention == right.convention
                && left.context == right.context
                && left.ty == right.ty
        })
}

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
