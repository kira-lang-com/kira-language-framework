use klf_kira_syntax::{Parsed, ast::File, parse};
use klf_services::{DefinitionArtifact, DefinitionUnit, QueryPhase, Session};
use std::sync::Arc;

pub(crate) fn parse_package_incremental(
    session: &mut Session,
    unit: &DefinitionUnit,
) -> Option<Parsed> {
    let mut parsed_files: Vec<Option<Arc<Parsed>>> = std::iter::repeat_with(|| None)
        .take(unit.files().len())
        .collect();
    let mut missing = Vec::new();
    for (index, file_id) in unit.files().iter().copied().enumerate() {
        let file_unit = DefinitionUnit::new(file_id);
        session.register_unit(file_unit.clone());
        if let Some(artifact) = session.cached_artifact("klfkira", &file_unit, QueryPhase::Parse) {
            parsed_files[index] = Some(artifact.downcast_arc::<Parsed>(session)?);
        } else {
            missing.push((index, file_id, session.file(file_id).clone()));
        }
    }
    let estimated_work = missing
        .iter()
        .map(|(_, _, source)| source.text().len())
        .sum();
    let parsed_missing =
        klf_services::parallel_map(missing, estimated_work, |(index, file, source)| {
            (index, file, parse(&source))
        });
    for (index, file, parsed) in parsed_missing {
        let file_unit = DefinitionUnit::new(file);
        let query = session.begin_query("klfkira", &file_unit, QueryPhase::Parse);
        for diagnostic in &parsed.diagnostics {
            session.report(diagnostic.clone());
        }
        let parsed = Arc::new(parsed);
        let artifact = DefinitionArtifact::new_shared(session, file_unit, Arc::clone(&parsed));
        session.cache_artifact("klfkira", QueryPhase::Parse, &artifact);
        session.finish_query(query);
        parsed_files[index] = Some(parsed);
    }

    let mut file = File {
        imports: Vec::new(),
        decls: Vec::new(),
    };
    let mut diagnostics = Vec::new();
    for parsed in parsed_files {
        let parsed = parsed?;
        diagnostics.extend(parsed.diagnostics.iter().cloned());
        file.imports.extend(parsed.file.imports.iter().cloned());
        file.decls.extend(parsed.file.decls.iter().cloned());
    }
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == klf_diag::Severity::Error)
    {
        return None;
    }
    Some(Parsed { file, diagnostics })
}

pub(crate) fn parse_file_incremental(
    session: &mut Session,
    file: klf_core::FileId,
) -> Option<Parsed> {
    let artifact = parse_file_incremental_artifact(session, file)?;
    artifact.downcast_ref::<Parsed>(session).cloned()
}

pub(crate) fn parse_file_incremental_artifact(
    session: &mut Session,
    file: klf_core::FileId,
) -> Option<DefinitionArtifact> {
    let unit = DefinitionUnit::new(file);
    session.register_unit(unit.clone());
    if let Some(artifact) = session.cached_artifact("klfkira", &unit, QueryPhase::Parse) {
        return Some(artifact);
    }
    let query = session.begin_query("klfkira", &unit, QueryPhase::Parse);
    let parsed = parse(session.file(file));
    for diagnostic in &parsed.diagnostics {
        session.report(diagnostic.clone());
    }
    let result = Arc::new(Parsed {
        file: parsed.file,
        diagnostics: parsed.diagnostics,
    });
    let artifact = DefinitionArtifact::new_shared(session, unit.clone(), result);
    session.cache_artifact("klfkira", QueryPhase::Parse, &artifact);
    session.finish_query(query);
    Some(artifact)
}
