//! The KLF Kira frontend's checking and lowering to KIR for the M4 subset.
//!
//! Resolution and type checking are folded into lowering here (D-122): the
//! subset — top-level functions over `Int`/sized integers/`Bool`/`Void`, with
//! arithmetic, comparisons, bindings, `if`, `while`, calls and `return` — is
//! small enough that a single tree walk resolves names, checks types and emits
//! KIR. A separate `klf-kira-sema` pass is split out when the language grows.

mod builtins;
mod cli;
mod constants;
mod definition_diagnostics;
mod definition_editor;
mod definition_entrypoints;
mod definition_file_modes;
mod definition_flags;
mod definition_metadata;
mod definition_pipeline;
mod definition_shader;
mod enums;
mod generics;
mod incremental;
mod lower;
mod methods;
mod service_surface;
mod session_types;
mod signatures;
mod structs;
pub mod testing;
mod ty;
mod wrappers;

pub use definition_pipeline::{
    ElaboratedFile, elaborate_file, lower_elaborated, lower_file, parse_registered_file, parse_unit,
};
use definition_pipeline::{compile_file, lower_context, lower_elaborated_cached};
pub(crate) use definition_pipeline::{resolve_type, trait_id_for, type_error};

use incremental::{
    parse_file_incremental, parse_file_incremental_artifact, parse_package_incremental,
};
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{Function, Module};
use klf_kira_syntax::ast::{CMember, DeclKind, File, FunctionDecl, MemberKind};
use klf_kira_syntax::{Parsed, parse};
use klf_services::{
    DefaultConvention, Definition, DefinitionArtifact, DefinitionPackage, DefinitionUnit,
    DiagnosticSpec, Exclusivity, LanguageDescriptor, MemoryModel, ModuleBuilder, QueryPhase,
    Requirement, Session, TraitId,
};
use klf_target::Target;
use session_types::TypeHandles;
pub(crate) use signatures::Signature;
use signatures::is_definition_phase;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use ty::FeType;

pub use cli::COMMAND_ENTRY;
pub use definition_metadata::{
    DefinitionEntrypoints, LanguageDefinitionMetadata, extract_language_descriptor,
    extract_language_metadata,
};
pub use service_surface::{KCoreService, KCoreServiceType, kcore_services};

/// The KLF Kira frontend as a kernel [`Definition`].
pub struct KiraDefinition;

impl Definition for KiraDefinition {
    fn name(&self) -> String {
        "klfkira".to_owned()
    }

    fn extensions(&self) -> Vec<String> {
        vec!["kcore".to_owned(), "klfk".to_owned(), "klfd".to_owned()]
    }

    fn language(&self) -> LanguageDescriptor {
        LanguageDescriptor::new(
            "klfkira",
            "klf",
            "1.0.0",
            vec!["kcore".to_owned(), "klfk".to_owned(), "klfd".to_owned()],
            MemoryModel::Counted,
            Exclusivity::None,
            DefaultConvention::Ridl,
            "std",
        )
        .with_diagnostics(vec![DiagnosticSpec::new(
            2001,
            klf_diag::Severity::Error,
            "runtime diagnostic",
            "a definition reported an error",
        )])
    }

    fn units(
        &self,
        session: &mut Session,
        package: &DefinitionPackage,
    ) -> Option<Vec<DefinitionUnit>> {
        if package.sources().is_empty() {
            session.report(Diagnostic::error(
                "KLF-DEF-0001",
                "definition package has no source files",
            ));
            return None;
        }
        if package.sources().len() == 1 {
            let source = &package.sources()[0];
            let file =
                session.upsert_file(source.name.as_ref(), std::sync::Arc::clone(&source.text));
            let unit = DefinitionUnit::new(file);
            session.register_unit(unit.clone());
            return Some(vec![unit]);
        }
        let files = package
            .sources()
            .iter()
            .map(|source| {
                let file =
                    session.upsert_file(source.name.as_ref(), std::sync::Arc::clone(&source.text));
                session.register_unit(DefinitionUnit::new(file));
                file
            })
            .collect();
        let unit = DefinitionUnit::from_files(files)?;
        session.register_unit(unit.clone());
        Some(vec![unit])
    }

    fn supports_phase_queries(&self) -> bool {
        true
    }

    fn parse(&self, session: &mut Session, unit: DefinitionUnit) -> Option<DefinitionArtifact> {
        if let Some(artifact) = session.cached_artifact(self.name(), &unit, QueryPhase::Parse) {
            let parsed = artifact.downcast_ref::<Parsed>(session)?;
            return (!parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == klf_diag::Severity::Error))
            .then_some(artifact);
        }
        if unit.files().len() == 1 {
            let artifact = parse_file_incremental_artifact(session, unit.file())?;
            let parsed = artifact.downcast_ref::<Parsed>(session)?;
            if parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == klf_diag::Severity::Error)
            {
                return None;
            }
            return Some(artifact);
        }
        let query = session.begin_derived_query(self.name(), &unit, QueryPhase::Parse);
        let result = parse_package_incremental(session, &unit).map(|parsed| {
            let artifact = DefinitionArtifact::new(session, unit.clone(), parsed);
            session.cache_artifact(self.name(), QueryPhase::Parse, &artifact);
            artifact
        });
        session.finish_query(query);
        result
    }

    fn elaborate(
        &self,
        session: &mut Session,
        unit: DefinitionUnit,
        parsed: &DefinitionArtifact,
    ) -> Option<DefinitionArtifact> {
        if parsed.unit() != unit {
            session.report(Diagnostic::error(
                "KLF-DEF-0003",
                "definition phase artifact belongs to another unit",
            ));
            return None;
        }
        if let Some(artifact) = session.cached_artifact(self.name(), &unit, QueryPhase::Elaborate) {
            return Some(artifact);
        }
        let query = session.begin_derived_query(self.name(), &unit, QueryPhase::Elaborate);
        session.record_query_dependency(self.name(), &unit, QueryPhase::Parse);
        let result = parsed
            .downcast_ref::<Parsed>(session)
            .cloned()
            .and_then(|parsed| elaborate_file(session, &parsed.file))
            .map(|elaborated| {
                let artifact = DefinitionArtifact::new(session, unit.clone(), elaborated);
                session.cache_artifact(self.name(), QueryPhase::Elaborate, &artifact);
                artifact
            });
        session.finish_query(query);
        result
    }

    fn lower(
        &self,
        session: &mut Session,
        unit: DefinitionUnit,
        program: &DefinitionArtifact,
        module_name: &str,
        target: Target,
    ) -> Option<Module> {
        self.lower_shared(session, unit, program, module_name, target)
            .map(Arc::unwrap_or_clone)
    }

    fn lower_shared(
        &self,
        session: &mut Session,
        unit: DefinitionUnit,
        program: &DefinitionArtifact,
        module_name: &str,
        target: Target,
    ) -> Option<Arc<Module>> {
        if program.unit() != unit {
            session.report(Diagnostic::error(
                "KLF-DEF-0003",
                "definition phase artifact belongs to another unit",
            ));
            return None;
        }
        if let Some(artifact) = session.cached_artifact_with_context(
            self.name(),
            &unit,
            QueryPhase::Lower,
            lower_context(module_name, &target),
        ) {
            return artifact.downcast_arc::<Module>(session).or_else(|| {
                artifact
                    .downcast_ref::<Module>(session)
                    .cloned()
                    .map(Arc::new)
            });
        }
        let program = program.downcast_ref::<ElaboratedFile>(session)?.clone();
        let query = session.begin_derived_query_with_context(
            self.name(),
            &unit,
            QueryPhase::Lower,
            lower_context(module_name, &target),
        );
        session.record_query_dependency(self.name(), &unit, QueryPhase::Elaborate);
        let result = lower_elaborated_cached(session, &unit, module_name, target.clone(), &program)
            .map(|module| {
                let module = Arc::new(module);
                let artifact =
                    DefinitionArtifact::new_shared(session, unit.clone(), Arc::clone(&module));
                session.cache_artifact_with_context(
                    self.name(),
                    QueryPhase::Lower,
                    lower_context(module_name, &target),
                    &artifact,
                );
                module
            });
        session.finish_query(query);
        result
    }

    fn compile(
        &self,
        session: &mut Session,
        source_name: &str,
        source: &str,
        module_name: &str,
        target: Target,
    ) -> Option<Module> {
        let package = DefinitionPackage::single(source_name, source);
        self.compile_package(session, &package, module_name, target)
    }

    fn compile_unit(
        &self,
        session: &mut Session,
        unit: DefinitionUnit,
        module_name: &str,
        target: Target,
    ) -> Option<Module> {
        compile_file(session, &unit, module_name, target)
    }
}
