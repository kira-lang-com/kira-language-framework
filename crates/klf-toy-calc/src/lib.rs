//! The toy integer-calculator language: a small `Definition` used to exercise
//! the whole toolchain (M4 exit test). Programs are `def name(params) = expr`
//! over `Int`/i64 with arithmetic, calls and `if c cmp c then e else e`.

mod lower;
mod syntax;

use klf_core::FileId;
use klf_kir::Module;
use klf_kira_lower::extract_language_descriptor;
use klf_kira_syntax::parse;
use klf_services::{
    Definition, DefinitionArtifact, DefinitionPackage, DefinitionUnit, LanguageDescriptor,
    QueryPhase, Session,
};
use klf_source::SourceFile;
use klf_target::Target;
use std::collections::HashMap;
use std::sync::OnceLock;

pub struct CalcDefinition;

#[derive(Clone)]
struct ParsedProgram {
    defs: Vec<syntax::Def>,
}

#[derive(Clone)]
struct ElaboratedProgram {
    defs: Vec<syntax::Def>,
    arities: HashMap<String, usize>,
}

impl Definition for CalcDefinition {
    fn name(&self) -> String {
        "calc".to_owned()
    }

    fn extensions(&self) -> Vec<String> {
        vec!["calc".to_owned()]
    }

    fn language(&self) -> LanguageDescriptor {
        calc_language_descriptor().clone()
    }

    fn supports_phase_queries(&self) -> bool {
        true
    }

    fn parse(&self, session: &mut Session, unit: DefinitionUnit) -> Option<DefinitionArtifact> {
        if let Some(artifact) = session.cached_artifact(self.name(), &unit, QueryPhase::Parse) {
            return Some(artifact);
        }
        let query = session.begin_query(self.name(), &unit, QueryPhase::Parse);
        let result = parse_unit(session, &unit).map(|parsed| {
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
            session.report(klf_diag::Diagnostic::error(
                "KLF-DEF-0003",
                "definition phase artifact belongs to another unit",
            ));
            return None;
        }
        if let Some(artifact) = session.cached_artifact(self.name(), &unit, QueryPhase::Elaborate) {
            return Some(artifact);
        }
        let query = session.begin_query(self.name(), &unit, QueryPhase::Elaborate);
        session.record_query_dependency(self.name(), &unit, QueryPhase::Parse);
        let result = parsed
            .downcast_ref::<ParsedProgram>(session)
            .cloned()
            .map(|parsed| {
                let artifact =
                    DefinitionArtifact::new(session, unit.clone(), elaborate_program(&parsed));
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
        if program.unit() != unit {
            session.report(klf_diag::Diagnostic::error(
                "KLF-DEF-0003",
                "definition phase artifact belongs to another unit",
            ));
            return None;
        }
        let program = program.downcast_ref::<ElaboratedProgram>(session)?.clone();
        let query = session.begin_query(self.name(), &unit, QueryPhase::Lower);
        session.record_query_dependency(self.name(), &unit, QueryPhase::Elaborate);
        let result = lower::lower_with_arities(
            session,
            module_name,
            target,
            &program.defs,
            &program.arities,
        );
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

fn calc_language_descriptor() -> &'static LanguageDescriptor {
    static DESCRIPTOR: OnceLock<LanguageDescriptor> = OnceLock::new();
    DESCRIPTOR.get_or_init(|| {
        let target = "aarch64-apple-darwin".parse().expect("valid target");
        let mut session = Session::new(target);
        let source = include_str!("../../../definitions/calc.klfd");
        let file = SourceFile::new(FileId::from_u32(0), "definitions/calc.klfd", source);
        let parsed = parse(&file);
        assert!(
            parsed.diagnostics.is_empty(),
            "definition parse failed: {:?}",
            parsed.diagnostics
        );
        extract_language_descriptor(&mut session, &parsed.file)
            .unwrap_or_else(|| panic!("definition metadata failed: {:?}", session.diagnostics()))
    })
}

fn compile_file(
    session: &mut Session,
    unit: &DefinitionUnit,
    module_name: &str,
    target: Target,
) -> Option<Module> {
    let parsed = parse_unit(session, unit)?;
    let program = elaborate_program(&parsed);
    lower_program(session, module_name, target, &program)
}

fn parse_unit(session: &mut Session, unit: &DefinitionUnit) -> Option<ParsedProgram> {
    let (defs, diagnostics) = syntax::parse(session.file(unit.file()));
    for diagnostic in diagnostics {
        session.report(diagnostic);
    }
    (session.error_count() == 0).then_some(ParsedProgram { defs })
}

fn elaborate_program(parsed: &ParsedProgram) -> ElaboratedProgram {
    let arities = parsed
        .defs
        .iter()
        .map(|def| (def.name.clone(), def.params.len()))
        .collect();
    ElaboratedProgram {
        defs: parsed.defs.clone(),
        arities,
    }
}

fn lower_program(
    session: &mut Session,
    module_name: &str,
    target: Target,
    program: &ElaboratedProgram,
) -> Option<Module> {
    lower::lower_with_arities(
        session,
        module_name,
        target,
        &program.defs,
        &program.arities,
    )
}
