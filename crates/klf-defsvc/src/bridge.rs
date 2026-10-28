//! Opening one phase call: the handles it was given, the session artifact it
//! will fill in, and the service table its compiled code calls into.

use crate::{
    ArtifactId, DefinitionArtifactKind, DefinitionImage, DefinitionPhase, DefinitionPhaseArguments,
    DefinitionRuntimeArtifact, DefinitionServiceBridge, DefinitionServiceState, DefinitionUnit,
    DefinitionUnitList, FileId, Session, UnitId,
};
use crate::{
    aggregate_builder, aggregate_call_builder, arithmetic_builder, control_flow_builder,
    conversion_builder, enum_array_builder, enum_control_builder, enum_payload_builder,
    float_builder, foreign_builder, kir_builder, legacy_builder, low_level_builder,
    low_level_call_builder, low_level_data_builder, memory_builder, raw_heap_builder,
    struct_array_builder, struct_builder, terminator_builder, type_builder, void_array_builder,
    void_builder,
};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// The unit the phase runs on and the file it reads, resolved against the
/// session.
fn phase_unit(
    session: &Session,
    arguments: &DefinitionPhaseArguments,
) -> Result<(UnitId, Option<FileId>), String> {
    match arguments {
        DefinitionPhaseArguments::Units { inputs } => {
            let file = *inputs
                .first()
                .ok_or_else(|| "definition units phase has no input file".to_owned())?;
            let unit = session
                .unit_id(&DefinitionUnit::new(file))
                .ok_or_else(|| "definition input unit is not registered".to_owned())?;
            Ok((unit, Some(file)))
        }
        DefinitionPhaseArguments::Parse { unit, .. }
        | DefinitionPhaseArguments::Elaborate { unit, .. }
        | DefinitionPhaseArguments::Lower { unit, .. } => {
            let id = session
                .unit_id(unit)
                .ok_or_else(|| "definition unit is not registered".to_owned())?;
            Ok((id, Some(unit.file())))
        }
    }
}

/// The artifact the phase reads, for the two phases that take one.
fn input_artifact(arguments: &DefinitionPhaseArguments) -> Option<ArtifactId> {
    match arguments {
        DefinitionPhaseArguments::Elaborate { tree, .. } => Some(tree.id()),
        DefinitionPhaseArguments::Lower { program, .. } => Some(program.id()),
        _ => None,
    }
}

/// The `units` phase's input files paired with their session units. Empty for
/// every other phase.
fn phase_inputs(
    session: &Session,
    arguments: &DefinitionPhaseArguments,
) -> Result<Vec<(FileId, UnitId)>, String> {
    // A parse is handed the program's files, which its loader reads the
    // other packages of (`docs/conformers.md`); the units phase its inputs.
    let files: &[FileId] = match arguments {
        DefinitionPhaseArguments::Units { inputs } => inputs,
        DefinitionPhaseArguments::Parse { program, .. } => program,
        DefinitionPhaseArguments::Elaborate { .. } | DefinitionPhaseArguments::Lower { .. } => {
            return Ok(Vec::new());
        }
    };
    files
        .iter()
        .map(|file| {
            session
                .unit_id(&DefinitionUnit::new(*file))
                .map(|unit| (*file, unit))
                .ok_or_else(|| "definition input unit is not registered".to_owned())
        })
        .collect()
}

/// The text of every file this phase may read: its own, plus the inputs the
/// `units` phase was given.
fn phase_sources(session: &Session) -> Vec<(FileId, String, Arc<str>)> {
    session
        .files()
        .iter()
        .map(|source| {
            (
                source.id(),
                source.name().as_str().to_owned(),
                source.shared_text(),
            )
        })
        .collect()
}

/// Reserves the empty artifact the phase will fill in, if it materializes one.
/// `setting` of each of the session's files, in file order.
fn per_file<T>(session: &Session, setting: impl Fn(&Session, FileId) -> T) -> Vec<T> {
    session
        .files()
        .iter()
        .map(|file| setting(session, file.id()))
        .collect()
}

fn reserve_artifact(
    session: &mut Session,
    phase: DefinitionPhase,
    reserve_output: bool,
) -> Option<(ArtifactId, DefinitionArtifactKind)> {
    if !reserve_output {
        return None;
    }
    let kind = match phase {
        DefinitionPhase::Units => {
            let id = session.store_artifact(DefinitionUnitList { units: Vec::new() });
            return Some((id, DefinitionArtifactKind::Units));
        }
        DefinitionPhase::Parse => DefinitionArtifactKind::Tree,
        DefinitionPhase::Elaborate => DefinitionArtifactKind::Program,
        DefinitionPhase::Lower => DefinitionArtifactKind::Module,
    };
    let id = session.store_artifact(DefinitionRuntimeArtifact { kind, payload: 0 });
    Some((id, kind))
}

impl DefinitionServiceBridge {
    /// Reserves this phase's Session artifact and opens the builder state the
    /// service handlers write into.
    ///
    /// # Errors
    /// Returns a message when the phase arguments name a unit, file or
    /// artifact the session does not know.
    pub fn new(
        session: &mut Session,
        phase: DefinitionPhase,
        arguments: &DefinitionPhaseArguments,
        reserve_output: bool,
    ) -> Result<Self, String> {
        let (unit, file) = phase_unit(session, arguments)?;
        let input_artifact = input_artifact(arguments);
        let input_payload = input_artifact.and_then(|id| {
            session
                .artifact::<DefinitionRuntimeArtifact>(id)
                .map(|artifact| artifact.payload)
        });
        let inputs = phase_inputs(session, arguments)?;
        let source = file
            .map(|file| session.file(file).text().to_owned())
            .unwrap_or_default();
        let sources = phase_sources(session);
        let source_hashes = session
            .files()
            .iter()
            .map(klf_source::SourceFile::hash)
            .collect();
        let output = reserve_artifact(session, phase, reserve_output);
        Ok(Self {
            state: Arc::new(Mutex::new(DefinitionServiceState {
                session: session.handle(),
                driver_flags: session.driver_flags().clone(),
                std_root: session.std_root().to_owned(),
                file_modes: per_file(session, Session::file_mode),
                file_memories: per_file(session, Session::file_memory),
                file_exclusivities: per_file(session, Session::file_exclusivity),
                file_conventions: per_file(session, Session::file_convention),
                unit,
                file,
                source,
                sources,
                source_hashes,
                sources_by_hash: std::cell::RefCell::default(),
                source_digests: std::collections::HashMap::new(),
                source_paths: std::collections::HashMap::new(),
                paths_indexed: 0,
                read_count: 0,
                expansions: Vec::new(),
                listing: Vec::new(),
                target: session.target().clone(),
                inputs,
                unit_list: Vec::new(),
                input_artifact,
                input_payload,
                output,
                materialized: None,
                module_error: None,
                modules: Vec::new(),
                functions: Vec::new(),
                values: Vec::new(),
                blocks: Vec::new(),
                arrays: Vec::new(),
                addresses: Vec::new(),
                i32_values: HashSet::new(),
                bool_values: HashSet::new(),
                types: crate::type_table::TypeTable::default(),
                enum_values: Vec::new(),
                enum_variants: Vec::new(),
                struct_values: Vec::new(),
                struct_enum_variants: Vec::new(),
                struct_known_enum_variants: Vec::new(),
                enum_struct_payloads: Vec::new(),
                kir_types: Vec::new(),
                kir_value_types: Vec::new(),
                value_type_table: Vec::new(),
                value_type_ids: std::collections::HashMap::new(),
                recent_value_types: Vec::new(),
                kir_second_results: std::collections::HashMap::new(),
                kir_text_bytes: std::collections::HashMap::new(),
                evaluations: Vec::new(),
                evaluation_roots: Vec::new(),
                evaluators: Vec::new(),
                kir_signatures: Vec::new(),
                kir_attribute_bytes: Vec::new(),
                kir_trap_report: Vec::new(),
                line_starts: std::collections::HashMap::new(),
                kir_pending_arguments: Vec::new(),
                kir_pending_parameters: Vec::new(),
                kir_records: std::collections::HashMap::new(),
                kir_globals: Vec::new(),
                kir_block_parameters: Vec::new(),
                kir_type_parameters: Vec::new(),
                kir_else_arguments: Vec::new(),
                kir_switch_cases: Vec::new(),
                facts: crate::editor_services::open_facts(session, phase),
                format: None,
                shader: crate::shader::ShaderState::new(session.shader().cloned()),
                overlay: session.overlay(),
                observations: Vec::new(),
                comptime_functions: Vec::new(),
                comptime_buffers: Vec::new(),
            })),
            file,
        })
    }

    /// Registers the files the phase read and the editor facts it recorded in
    /// `session`. Call it as soon as the phase returns, failed or not, before
    /// its diagnostics are resolved, since they may point into those files.
    ///
    /// # Errors
    /// Returns a message when the bridge lock is poisoned or the session gave
    /// a file an unexpected id.
    pub fn commit_records(&self, session: &mut Session) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|_| "definition service bridge lock poisoned".to_owned())?
            .commit_records(session)
    }

    pub fn register(&self, capabilities: &mut crate::ServiceSink) {
        crate::flag_services::register(&self.state, capabilities);
        crate::source_services::register(&self.state, capabilities);
        crate::editor_services::register(&self.state, capabilities);
        crate::format_services::register(&self.state, capabilities);
        crate::shader::register(&self.state, capabilities);
        capabilities.register_state(
            "klfsvc_session_file_mode",
            &self.state,
            |state, arguments| state.file_mode(arguments),
        );
        capabilities.register_state(
            "klfsvc_session_file_memory",
            &self.state,
            |state, arguments| state.file_memory(arguments),
        );
        capabilities.register_state(
            "klfsvc_session_file_convention",
            &self.state,
            |state, arguments| state.file_convention(arguments),
        );
        capabilities.register_state(
            "klfsvc_source_file_len",
            &self.state,
            move |state, arguments| state.source_file_len(arguments),
        );
        capabilities.register_state(
            "klfsvc_source_file_byte",
            &self.state,
            move |state, arguments| state.source_file_byte(arguments),
        );
        capabilities.register_state_with_memory(
            "klfsvc_source_file_copy",
            &self.state,
            move |state, arguments, memory| state.source_file_copy(arguments, memory),
        );
        capabilities.register_state(
            "klfsvc_definition_unit",
            &self.state,
            move |state, arguments| state.unit(arguments),
        );
        capabilities.register_state(
            "klfsvc_definition_tree",
            &self.state,
            move |state, arguments| state.artifact(DefinitionArtifactKind::Tree, arguments),
        );
        capabilities.register_state(
            "klfsvc_definition_program",
            &self.state,
            move |state, arguments| state.artifact(DefinitionArtifactKind::Program, arguments),
        );
        capabilities.register_state(
            "klfsvc_definition_input",
            &self.state,
            move |state, arguments| state.input(arguments),
        );
        capabilities.register_state(
            "klfsvc_definition_tree_payload",
            &self.state,
            move |state, arguments| state.payload(arguments),
        );
        capabilities.register_state(
            "klfsvc_definition_program_payload",
            &self.state,
            move |state, arguments| state.payload(arguments),
        );
        capabilities.register_state(
            "klfsvc_definition_units_push",
            &self.state,
            DefinitionServiceState::units_push,
        );
        capabilities.register_state(
            "klfsvc_definition_units_finish",
            &self.state,
            DefinitionServiceState::units_finish,
        );
        self.register_module_services(capabilities);
        crate::module_evaluation::register(&self.state, capabilities);
        kir_builder::register(&self.state, capabilities);
        aggregate_builder::register(&self.state, capabilities);
        arithmetic_builder::register(&self.state, capabilities);
        conversion_builder::register(&self.state, capabilities);
        memory_builder::register(&self.state, capabilities);
        type_builder::register(&self.state, capabilities);
        enum_control_builder::register(&self.state, capabilities);
        struct_builder::register(&self.state, capabilities);
        aggregate_call_builder::register(&self.state, capabilities);
        void_builder::register(&self.state, capabilities);
        void_array_builder::register(&self.state, capabilities);
        enum_payload_builder::register(&self.state, capabilities);
        struct_array_builder::register(&self.state, capabilities);
        enum_array_builder::register(&self.state, capabilities);
        terminator_builder::register(&self.state, capabilities);
        foreign_builder::register(&self.state, capabilities);
        float_builder::register(&self.state, capabilities);
        low_level_builder::register(&self.state, capabilities);
        low_level_call_builder::register(&self.state, capabilities);
        low_level_data_builder::register(&self.state, capabilities);
        raw_heap_builder::register(&self.state, capabilities);
        control_flow_builder::register(&self.state, capabilities);
        legacy_builder::register(&self.state, capabilities);
    }

    fn register_module_services(&self, capabilities: &mut crate::ServiceSink) {
        capabilities.register_state(
            "klfsvc_kir_module_new",
            &self.state,
            DefinitionServiceState::module_new,
        );
        capabilities.register_state(
            "klfsvc_kir_module_finish",
            &self.state,
            DefinitionServiceState::module_finish,
        );
        capabilities.register_state(
            "klfsvc_kir_function_referenced",
            &self.state,
            DefinitionServiceState::function_referenced,
        );
        capabilities.register_state(
            "klfsvc_kir_function_discard",
            &self.state,
            DefinitionServiceState::function_discard,
        );
        capabilities.register_state(
            "klfsvc_kir_function_comptime",
            &self.state,
            DefinitionServiceState::function_comptime,
        );
        capabilities.register_state(
            "klfsvc_kir_function_comptime_buffer",
            &self.state,
            DefinitionServiceState::function_comptime_buffer,
        );
        capabilities.register_state(
            "klfsvc_kir_function_comptime_array",
            &self.state,
            DefinitionServiceState::function_comptime_array,
        );
        capabilities.register_state(
            "klfsvc_kir_function_comptime_record",
            &self.state,
            DefinitionServiceState::function_comptime_record,
        );
    }
}

#[must_use]
pub fn phase_materializes_artifact(image: &DefinitionImage, phase: DefinitionPhase) -> bool {
    let symbols: &[&str] = match phase {
        DefinitionPhase::Parse => &["klfsvc_definition_tree"],
        DefinitionPhase::Elaborate => &["klfsvc_definition_program"],
        DefinitionPhase::Lower => &["klfsvc_kir_module_finish", "klfsvc_shader_module_finish"],
        DefinitionPhase::Units => &["klfsvc_definition_units_finish"],
    };
    symbols
        .iter()
        .any(|symbol| calls_foreign_symbol(image.module(), phase.symbol(), symbol))
}

/// Whether `entry` reaches `symbol` through direct calls. A definition is free
/// to materialize its artifact in a helper, so the search follows the call
/// graph rather than looking at the phase function alone.
fn calls_foreign_symbol(module: &klf_kir::Module, entry: &str, symbol: &str) -> bool {
    let mut pending = vec![entry.to_owned()];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(function) = module
            .functions
            .iter()
            .find(|function| function.name.as_str() == name)
        else {
            continue;
        };
        for instruction in function
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
        {
            match &instruction.operation {
                klf_kir::Operation::CallForeign { symbol: called, .. }
                    if called.as_str() == symbol =>
                {
                    return true;
                }
                klf_kir::Operation::Call { callee, .. } => pending.push(callee.as_str().to_owned()),
                klf_kir::Operation::TryCall(call) => pending.push(call.callee.as_str().to_owned()),
                _ => {}
            }
        }
    }
    false
}
