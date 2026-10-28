//! The definition-facing kernel services, independent of where the phase code
//! runs.
//!
//! A `Language` definition builds its KIR through `klf.kir.*` and reaches the
//! Session through `klf.definition.*`. Both surfaces are plain
//! `fn(&[u64]) -> u64` handlers over one [`DefinitionServiceState`], collected
//! into a [`ServiceSink`]. The VM feeds that sink into its capability table;
//! a natively compiled frontend feeds it into its `extern "C"` shims. One
//! implementation backs both.

mod bridge;
mod command_values;
mod drive;
pub mod driver_cli;
mod editor_services;
mod evaluation;
mod file_services;
pub use file_services::{listing, path_present};
mod format_drive;
mod format_services;
mod project_hook;
mod metadata;
mod module_builder;
mod module_evaluation;
mod report;
mod shader;
mod sink;
mod source_services;
mod source_spans;
mod subquery_services;
mod unit_conflict;

mod aggregate_builder;
mod aggregate_call_builder;
mod arithmetic_builder;
mod control_flow_builder;
mod conversion_builder;
mod default_policy;
mod driver_arguments;
mod enum_array_builder;
mod enum_control_builder;
mod enum_payload_builder;
mod flag_services;
mod float_builder;
mod foreign_builder;
mod kir_builder;
mod legacy_builder;
mod lifecycle;
mod low_level_builder;
mod low_level_call_builder;
mod low_level_data_builder;
mod low_level_enum_builder;
mod low_level_float_builder;
mod low_level_type_builder;
mod memory_builder;
mod raw_heap_builder;
mod struct_array_builder;
mod struct_builder;
mod struct_copy;
mod terminator_builder;
mod trap_report;
mod debug_locations;
mod type_builder;
mod type_table;
mod type_value_builder;
mod void_array_builder;
mod void_builder;

pub use bridge::phase_materializes_artifact;

use klf_core::{FileId, Name};
use klf_kir::{Module, Type};
use klf_services::{
    ArtifactId, DefaultConvention, DefinitionArtifactKind, DefinitionImage, DefinitionPhase,
    DefinitionPhaseArguments, DefinitionRuntimeArtifact, DefinitionUnit, DefinitionUnitList,
    DriverFlagValue, Exclusivity, FunctionBuilder, MemoryModel, ModuleBuilder, Session,
    SessionHandle, UnitId,
};
use klf_source::Span;
use klf_target::Target;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub use command_values::{COMMAND_ENTRY, register_command_values};
pub use drive::{
    Decode, DriveError, Kept, ModuleCache, PhaseRunner, Stored, UnitTrace, complete_phase, drive,
    drive_cached, drive_shaders, prepare_phase,
};
pub use driver_arguments::{
    COMPAT_DRIVER_ENTRY, CompatDriverCall, RUN_PROGRAM_ENTRY, RunProgramCall,
    register_driver_arguments,
};
pub use format_drive::{
    FORMAT_ENTRY, FormatFailure, FormatRunner, close_format, format_file, open_format,
};
pub use metadata::{FrontendMetadata, Hook};
pub use project_hook::{HookProject, PROJECT_ENTRY, ProjectCall};
pub use report::{render_diagnostics, render_kept_diagnostics, settle_phase_diagnostics};
pub use shader::read::register_reader;
pub use sink::{Handler, ServiceSink};
pub use subquery_services::keep_subqueries;

pub struct DefinitionServiceBridge {
    pub state: Arc<Mutex<DefinitionServiceState>>,
    pub file: Option<FileId>,
}

pub struct DefinitionServiceState {
    session: SessionHandle,
    driver_flags: HashMap<String, DriverFlagValue>,
    std_root: String,
    file_modes: Vec<Option<usize>>,
    file_memories: Vec<Option<MemoryModel>>,
    file_exclusivities: Vec<Option<Exclusivity>>,
    file_conventions: Vec<Option<DefaultConvention>>,
    unit: UnitId,
    file: Option<FileId>,
    source: String,
    target: Target,
    /// The package inputs the `units` phase splits, paired with the unit the
    /// session registered for each.
    inputs: Vec<(FileId, UnitId)>,
    /// The text of every file the phase can reach, so a whole-program check
    /// can read a unit other than its own (section 8.1: a `SourceFile` carries
    /// its text).
    sources: Vec<(FileId, String, std::sync::Arc<str>)>,
    /// `source_hash` of each entry of `sources`, computed once.
    source_hashes: Vec<u64>,
    /// The first entry of `sources` with each hash, for the entries before
    /// the count it holds.
    sources_by_hash: std::cell::RefCell<(usize, HashMap<u64, usize>)>,
    /// The SHA-256 of each file's text a sub-query key was asked for, by
    /// file.
    source_digests: HashMap<u64, [u8; 32]>,
    /// The first entry of `sources` under each name it has and each canonical
    /// path it resolves to, for the entries before `paths_indexed`.
    source_paths: std::collections::HashMap<std::path::PathBuf, FileId>,
    paths_indexed: usize,
    /// How many entries at the end of `sources` this phase read itself and
    /// the Session does not hold yet.
    read_count: usize,
    /// The macro expansions this phase reported, for the Session.
    expansions: Vec<klf_services::Expansion>,
    /// The files of the last `klf.source.directory` call.
    listing: Vec<FileId>,
    /// The units the running `units` phase has pushed so far.
    unit_list: Vec<PushedUnit>,
    input_artifact: Option<ArtifactId>,
    /// The word the definition parked in the artifact this phase consumes.
    input_payload: Option<u64>,
    output: Option<(ArtifactId, DefinitionArtifactKind)>,
    materialized: Option<DefinitionMaterialization>,
    module_error: Option<String>,
    modules: Vec<Option<ModuleBuilder>>,
    functions: Vec<Option<BuilderFunction>>,
    comptime_functions: Vec<Name>,
    comptime_buffers: Vec<evaluation::BufferResult>,
    values: Vec<Option<BuilderValue>>,
    blocks: Vec<Option<BuilderBlock>>,
    arrays: Vec<(u64, aggregate_builder::BuilderArray)>,
    addresses: Vec<Option<BuilderAddress>>,
    i32_values: HashSet<u64>,
    bool_values: HashSet<u64>,
    types: type_table::TypeTable,
    enum_values: Vec<(u64, u64)>,
    enum_variants: Vec<(u64, u32)>,
    struct_values: Vec<(u64, u64, Vec<bool>)>,
    struct_enum_variants: Vec<(u64, usize, u32)>,
    struct_known_enum_variants: Vec<(u64, Vec<struct_copy::CopyStep>, u32)>,
    enum_struct_payloads: Vec<(u64, u64)>,
    // The general low-level builder's arbitrary KIR types (indexed by handle)
    // and the KIR type of each value handle it produces.
    kir_types: Vec<Type>,
    /// The owning function and type of each typed value, at its value id.
    /// The type is an index into `value_type_table`: a module's values share
    /// a few hundred types, so each is stored once.
    kir_value_types: Vec<Option<(u64, u32)>>,
    value_type_table: Vec<Type>,
    value_type_ids: HashMap<Type, u32>,
    /// The value types most recently given, newest last: a run of values
    /// mostly shares a few, which compare cheaper than they hash.
    recent_value_types: Vec<(Type, u32)>,
    /// The second result of a two-result instruction, by function and first
    /// result: a constant string's length, a compare-exchange's success.
    kir_second_results: HashMap<(u64, u64), u64>,
    kir_text_bytes: HashMap<u64, Vec<u8>>,
    evaluations: Vec<module_evaluation::EvaluatedValue>,
    /// The entries each detached builder is to be evaluated from.
    evaluation_roots: Vec<(u64, u64, Name)>,
    evaluators: Vec<Option<Result<module_evaluation::PreparedEvaluation, String>>>,
    kir_signatures: Vec<Option<PendingSignature>>,
    /// The text of the next module attribute, a byte at a time.
    kir_attribute_bytes: Vec<u8>,
    /// The report of the next trap, a byte at a time (`trap_report.rs`).
    kir_trap_report: Vec<u8>,
    /// Where each line starts in the sources `klf.kir.function.at` named,
    /// by path (`debug_locations.rs`).
    line_starts: HashMap<String, Arc<Vec<usize>>>,
    kir_pending_arguments: Vec<(u64, Name)>,
    kir_pending_parameters: Vec<(u64, Type)>,
    kir_records: HashMap<u64, usize>,
    kir_globals: Vec<Option<PendingGlobal>>,
    kir_block_parameters: Vec<(u64, Type)>,
    kir_type_parameters: Vec<Type>,
    kir_else_arguments: Vec<(u64, Name)>,
    kir_switch_cases: Vec<(u64, i128, Name, Vec<Name>)>,
    /// The editor facts this phase recorded, absent in `Build` mode.
    facts: Option<klf_facts::Facts>,
    /// The layout a `format` hook call describes (`KLF-FORMAT.md`).
    format: Option<klf_format::Builder>,
    /// The shader builder (`KLF-KSL.md` section 6).
    shader: shader::ShaderState,
    /// The open documents that stand in for files on disk.
    overlay: Arc<HashMap<std::path::PathBuf, Arc<str>>>,
    /// What this phase learned from the file system, for the session.
    observations: Vec<klf_services::Observation>,
}

/// A module-scope global under construction. Its initializer is filled in
/// incrementally so a definition can supply decoded bytes itself.
struct PendingGlobal {
    module: u64,
    name: Name,
    ty: Type,
    mutable: bool,
    /// Each thread has its own copy (section 12.7's `tls.addr`).
    thread_local: bool,
    init: klf_kir::GlobalInit,
}

/// A function signature under construction. `FunctionBuilder` takes its
/// parameters up front, so a definition accumulates them here and then defines
/// the function in one step.
struct PendingSignature {
    module: u64,
    name: Name,
    /// The name's bytes as `signature.name_byte` supplies them, one at a
    /// time; they replace `name` when the signature is declared.
    name_bytes: Option<Vec<u8>>,
    span: Span,
    result: Type,
    parameters: Vec<Type>,
    c_parameters: Vec<klf_kir::CType>,
    c_result: Option<klf_kir::CType>,
    c_variadic: Option<usize>,
    c_syscall: Option<Vec<u8>>,
    exported: bool,
}

struct BuilderFunction {
    module: u64,
    name: Name,
    builder: FunctionBuilder,
    result_type: Type,
    parameters: Vec<Name>,
    parameter_types: Vec<Type>,
    /// Whether a call or reference names the function yet: a lowering may
    /// leave the body of one nothing names unwritten.
    referenced: std::cell::Cell<bool>,
}

impl BuilderFunction {
    /// The function's name, for a call or reference that names it.
    fn reference(&self) -> Name {
        self.referenced.set(true);
        self.name.clone()
    }
}

struct BuilderAddress {
    function: u64,
    name: Name,
    element: Type,
    initialized: bool,
}

struct BuilderValue {
    function: u64,
    name: Name,
}

struct BuilderBlock {
    function: u64,
    name: Name,
    parameters: Vec<Name>,
    parameter_types: Vec<Type>,
}

/// A unit the `units` phase pushed: one the session holds for an input, or
/// one named by a file the phase read, which the session registers once the
/// phase has committed the files it read (section 7.2).
#[derive(Clone, Copy)]
enum PushedUnit {
    Input(UnitId),
    Read(FileId),
}

/// The flag that marks a `unit` handle as naming a file the phase read rather
/// than a session unit, whose ids fit 32 bits.
const READ_UNIT: u64 = 1 << 32;

enum DefinitionMaterialization {
    Units(Vec<PushedUnit>),
    Artifact(DefinitionArtifactKind, u64),
    Module(Box<Module>),
    Shader(Box<klf_shader::Module>),
}

impl DefinitionServiceState {
    /// Returns the descriptor exclusivity rule captured for a source file.
    #[must_use]
    pub fn file_exclusivity(&self, file: FileId) -> Option<Exclusivity> {
        self.file_exclusivities.get(file.index()).copied().flatten()
    }

    fn file_value<T: Copy>(
        &self,
        arguments: &[u64],
        values: &[Option<T>],
        encode: impl FnOnce(T) -> u64,
    ) -> u64 {
        let [session, file] = arguments else {
            return u64::MAX;
        };
        if *session != self.session.as_u64() {
            return u64::MAX;
        }
        let Some(value) = usize::try_from(*file)
            .ok()
            .and_then(|index| values.get(index))
            .copied()
            .flatten()
        else {
            return u64::MAX;
        };
        encode(value)
    }

    fn file_mode(&self, arguments: &[u64]) -> u64 {
        let [session, file] = arguments else {
            return u64::MAX;
        };
        if *session != self.session.as_u64() {
            return u64::MAX;
        }
        let Some(mode) = usize::try_from(*file)
            .ok()
            .and_then(|index| self.file_modes.get(index))
        else {
            return u64::MAX;
        };
        mode.and_then(|index| i64::try_from(index).ok())
            .map_or(u64::MAX, i64::cast_unsigned)
    }

    fn file_memory(&self, arguments: &[u64]) -> u64 {
        self.file_value(arguments, &self.file_memories, |memory| match memory {
            MemoryModel::Unmanaged => 0,
            MemoryModel::Counted => 1,
            MemoryModel::Affine => 2,
        })
    }

    fn file_convention(&self, arguments: &[u64]) -> u64 {
        self.file_value(
            arguments,
            &self.file_conventions,
            |convention| match convention {
                DefaultConvention::Ridl => 0,
                DefaultConvention::Platform => 1,
            },
        )
    }

    fn valid_session_unit(&self, arguments: &[u64]) -> bool {
        arguments.first().copied() == Some(self.session.as_u64())
            && arguments.get(1).copied() == Some(u64::from(self.unit.as_u32()))
    }

    /// The unit of one of the package inputs, or of a file the phase read. A
    /// definition calls this once per file it decides to make a translation
    /// unit of.
    fn unit(&self, arguments: &[u64]) -> u64 {
        if arguments.first().copied() != Some(self.session.as_u64()) {
            return u64::MAX;
        }
        let Some(file) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        if self.inputs.is_empty() {
            return match self.file {
                Some(current) if u64::from(current.as_u32()) == file => {
                    u64::from(self.unit.as_u32())
                }
                _ => u64::MAX,
            };
        }
        if let Some((_, unit)) = self
            .inputs
            .iter()
            .find(|(input, _)| u64::from(input.as_u32()) == file)
        {
            return u64::from(unit.as_u32());
        }
        if self
            .sources
            .iter()
            .any(|(known, _, _)| u64::from(known.as_u32()) == file)
        {
            return READ_UNIT | file;
        }
        u64::MAX
    }

    fn input(&self, arguments: &[u64]) -> u64 {
        if arguments.first().copied() != Some(self.session.as_u64()) {
            return u64::MAX;
        }
        let Some(index) = arguments
            .get(1)
            .copied()
            .and_then(|index| usize::try_from(index).ok())
        else {
            return u64::MAX;
        };
        self.inputs
            .get(index)
            .map_or(u64::MAX, |(file, _)| u64::from(file.as_u32()))
    }

    /// The word the definition parked in the artifact this phase consumes. The
    /// kernel never looks inside it; it only guarantees the phase that reads it
    /// back is the one the artifact was handed to.
    fn payload(&self, arguments: &[u64]) -> u64 {
        if arguments.first().copied() != Some(self.session.as_u64()) {
            return u64::MAX;
        }
        let expected = self
            .input_artifact
            .map(|id| u64::try_from(id.index()).unwrap_or(u64::MAX));
        if arguments.get(1).copied() != expected {
            return u64::MAX;
        }
        self.input_payload.unwrap_or(u64::MAX)
    }

    fn units_push(&mut self, arguments: &[u64]) -> u64 {
        if arguments.first().copied() != Some(self.session.as_u64()) {
            return u64::MAX;
        }
        let Some(word) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let pushed = if word & READ_UNIT != 0 {
            let Ok(file) = u32::try_from(word & !READ_UNIT) else {
                return u64::MAX;
            };
            let file = FileId::from_u32(file);
            if self.source_entry(file).is_none() {
                return u64::MAX;
            }
            PushedUnit::Read(file)
        } else {
            let Ok(unit) = u32::try_from(word) else {
                return u64::MAX;
            };
            let unit = UnitId::from_u32(unit);
            if !self.inputs.iter().any(|(_, known)| *known == unit) {
                return u64::MAX;
            }
            PushedUnit::Input(unit)
        };
        self.unit_list.push(pushed);
        0
    }

    fn units_finish(&mut self, arguments: &[u64]) -> u64 {
        if arguments.first().copied() != Some(self.session.as_u64()) {
            return u64::MAX;
        }
        let Some((id, DefinitionArtifactKind::Units)) = self.output else {
            return u64::MAX;
        };
        self.materialized = Some(DefinitionMaterialization::Units(std::mem::take(
            &mut self.unit_list,
        )));
        u64::try_from(id.index()).unwrap_or(u64::MAX)
    }

    fn artifact(&mut self, kind: DefinitionArtifactKind, arguments: &[u64]) -> u64 {
        if !self.valid_session_unit(arguments) {
            return u64::MAX;
        }
        let Some((id, expected)) = self.output else {
            return u64::MAX;
        };
        if expected != kind {
            return u64::MAX;
        }
        self.materialized = Some(DefinitionMaterialization::Artifact(
            kind,
            arguments.get(2).copied().unwrap_or(0),
        ));
        u64::try_from(id.index()).unwrap_or(u64::MAX)
    }

    fn module_new(&mut self, arguments: &[u64]) -> u64 {
        if !self.valid_session_unit(arguments)
            || arguments.get(2).copied()
                != self
                    .input_artifact
                    .map(|id| u64::try_from(id.index()).unwrap_or(u64::MAX))
        {
            return u64::MAX;
        }
        let id = u64::try_from(self.modules.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.modules.push(Some(ModuleBuilder::new(
            Name::new("definition"),
            "klfkira",
            self.target.clone(),
        )));
        id
    }

    fn value_name(&self, function: u64, value: Option<u64>) -> Option<Name> {
        let value = usize::try_from(value?).ok()?;
        let value = self.values.get(value)?.as_ref()?;
        (value.function == function).then(|| value.name.clone())
    }

    /// Commits what the phase built into the Session artifact it reserved.
    ///
    /// # Errors
    /// Returns a message when the phase returned a different handle or did not
    /// materialize the artifact its phase owes.
    pub fn finish(self, session: &mut Session, word: Option<u64>) -> Result<Option<u64>, String> {
        if let Some(error) = self.module_error {
            return Err(error);
        }
        let Some((id, kind)) = self.output else {
            return Ok(word);
        };
        let expected = u64::try_from(id.index()).unwrap_or(u64::MAX);
        if word != Some(expected) {
            return Err(
                "definition phase returned a handle different from its Session artifact".to_owned(),
            );
        }
        match (kind, self.materialized) {
            (DefinitionArtifactKind::Units, Some(DefinitionMaterialization::Units(pushed))) => {
                let units = pushed
                    .into_iter()
                    .map(|unit| match unit {
                        PushedUnit::Input(unit) => unit,
                        PushedUnit::Read(file) => session.register_unit(DefinitionUnit::new(file)),
                    })
                    .collect();
                if !session.replace_artifact(id, DefinitionUnitList { units }) {
                    return Err("definition phase artifact slot disappeared".to_owned());
                }
            }
            (
                DefinitionArtifactKind::Tree | DefinitionArtifactKind::Program,
                Some(DefinitionMaterialization::Artifact(materialized_kind, payload)),
            ) if materialized_kind == kind => {
                if !session.replace_artifact(id, DefinitionRuntimeArtifact { kind, payload }) {
                    return Err("definition phase artifact slot disappeared".to_owned());
                }
            }
            (DefinitionArtifactKind::Module, Some(DefinitionMaterialization::Module(module))) => {
                if !session.replace_artifact(id, *module) {
                    return Err("definition phase artifact slot disappeared".to_owned());
                }
            }
            (DefinitionArtifactKind::Module, Some(DefinitionMaterialization::Shader(module))) => {
                if !session.replace_artifact(id, *module) {
                    return Err("definition phase artifact slot disappeared".to_owned());
                }
            }
            _ => {
                return Err(
                    "definition phase materialized the wrong or missing artifact".to_owned(),
                );
            }
        }
        Ok(word)
    }
}
