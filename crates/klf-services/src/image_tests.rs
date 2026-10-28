//! Phase-ABI tests for [`super::image`].

use super::*;
use crate::definition::FileMode;

fn complete_image() -> DefinitionImage {
    let module = klf_kir::parse_module(
        r#"module @Definition lang "klfkira" target "aarch64-apple-darwin" stage raw

func module @units() -> unit engine(any) {
bb0():
  return
}
func module @parse() -> unit engine(any) {
bb0():
  return
}
func module @elaborate() -> unit engine(any) {
bb0():
  return
}
func module @lower() -> unit engine(any) {
bb0():
  return
}
"#,
    )
    .expect("definition module parses");
    DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), module)
        .expect("complete image")
}

struct StubExecutor;

impl DefinitionPhaseExecutor for StubExecutor {
    fn execute(
        &self,
        image: &DefinitionImage,
        phase: DefinitionPhase,
        session: &mut Session,
        arguments: &DefinitionPhaseArguments,
    ) -> Result<DefinitionPhaseResult, String> {
        match (phase, arguments) {
            (DefinitionPhase::Units, DefinitionPhaseArguments::Units { inputs }) => {
                let unit = DefinitionUnit::new(inputs[0]);
                session.register_unit(unit.clone());
                Ok(DefinitionPhaseResult::Units(vec![unit]))
            }
            (DefinitionPhase::Parse, DefinitionPhaseArguments::Parse { unit, .. })
            | (DefinitionPhase::Elaborate, DefinitionPhaseArguments::Elaborate { unit, .. }) => Ok(
                DefinitionPhaseResult::Artifact(DefinitionArtifact::new(session, unit.clone(), ())),
            ),
            (DefinitionPhase::Lower, DefinitionPhaseArguments::Lower { .. }) => Ok(
                DefinitionPhaseResult::Module(Box::new(image.module().clone())),
            ),
            _ => Err("phase argument mismatch".to_owned()),
        }
    }
}
#[test]
fn definition_image_requires_all_language_phases() {
    let image = complete_image();
    assert_eq!(image.phase(DefinitionPhase::Lower).name.as_str(), "lower");
    assert_eq!(image.descriptor().name, "calc");

    let incomplete = klf_kir::parse_module(
        r#"module @Definition lang "klfkira" target "aarch64-apple-darwin" stage raw

func module @units() -> unit engine(any) {
bb0():
  return
}
"#,
    )
    .expect("incomplete module parses");
    assert_eq!(
        DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), incomplete),
        Err(DefinitionImageError::MissingPhase {
            phase: DefinitionPhase::Parse
        })
    );

    let mut declaration_only = image.module().clone();
    declaration_only
        .functions
        .iter_mut()
        .find(|function| function.name.as_str() == "parse")
        .expect("parse phase")
        .blocks
        .clear();
    assert_eq!(
        DefinitionImage::new(
            LanguageDescriptor::minimal("calc", &["calc"]),
            declaration_only
        ),
        Err(DefinitionImageError::NonExecutablePhase {
            phase: DefinitionPhase::Parse
        })
    );
}

#[test]
fn definition_image_validates_file_mode_extensions() {
    let module = complete_image().module().clone();
    let unclaimed =
        LanguageDescriptor::minimal("calc", &["calc"]).with_file_modes(vec![FileMode {
            name: "alternate".to_owned(),
            pragma: "alternate".to_owned(),
            flag: "alternate".to_owned(),
            extensions: vec!["alt".to_owned()],
            memory: None,
            convention: None,
        }]);
    assert_eq!(
        DefinitionImage::new(unclaimed, module.clone()),
        Err(DefinitionImageError::UnclaimedFileModeExtension {
            mode: "alternate".to_owned(),
            extension: "alt".to_owned(),
        })
    );

    let duplicate =
        LanguageDescriptor::minimal("calc", &["calc"]).with_file_modes(vec![FileMode {
            name: "alternate".to_owned(),
            pragma: "alternate".to_owned(),
            flag: "alternate".to_owned(),
            extensions: vec!["calc".to_owned(), "calc".to_owned()],
            memory: None,
            convention: None,
        }]);
    assert_eq!(
        DefinitionImage::new(duplicate, module),
        Err(DefinitionImageError::DuplicateFileModeExtension {
            extension: "calc".to_owned(),
        })
    );
}

#[test]
fn executable_definition_reports_unimplemented_session_bridge() {
    let definition = ExecutableDefinition::new(complete_image());
    assert_eq!(definition.language().name, "calc");

    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    let target = session.target().clone();
    assert!(
        definition
            .compile(
                &mut session,
                "input.calc",
                "def main() = 42",
                "Main",
                target
            )
            .is_none()
    );
    assert_eq!(session.diagnostics()[0].code, "KLF-DEF-0005");
}
#[test]
fn executable_definition_runs_typed_phase_executor() {
    let definition = ExecutableDefinition::with_executor(complete_image(), StubExecutor);
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));

    let module = definition
        .compile(
            &mut session,
            "input.calc",
            "1 + 1",
            "Main",
            "aarch64-apple-darwin".parse().expect("valid target"),
        )
        .expect("executor compiles");
    assert_eq!(module.name.as_str(), "Definition");
    assert_eq!(session.diagnostics(), [] as [klf_diag::Diagnostic; 0]);
}
#[test]
fn phase_arguments_marshal_registered_session_handles() {
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    let file = session.add_file("input.calc", "1 + 1");
    let unit = DefinitionUnit::new(file);
    let unit_id = session.register_unit(unit.clone());
    let tree = DefinitionArtifact::new(&mut session, unit.clone(), ());
    let arguments = DefinitionPhaseArguments::Elaborate { unit, tree };

    assert_eq!(
        arguments.marshal(&session).expect("handles marshal"),
        vec![session.handle().as_u64(), u64::from(unit_id.as_u32()), 0,]
    );
}
#[test]
fn parse_arguments_marshal_the_program_file_count() {
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    let file = session.add_file("input.calc", "1 + 1");
    let other = session.add_file("other.calc", "2 + 2");
    let unit = DefinitionUnit::new(file);
    let unit_id = session.register_unit(unit.clone());
    let arguments = DefinitionPhaseArguments::Parse {
        unit,
        program: vec![file, other],
    };

    assert_eq!(
        arguments.marshal(&session).expect("handles marshal"),
        vec![session.handle().as_u64(), u64::from(unit_id.as_u32()), 2]
    );
}

#[test]
fn parse_arguments_reject_an_unregistered_program_file() {
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    let file = session.add_file("input.calc", "1 + 1");
    let unit = DefinitionUnit::new(file);
    session.register_unit(unit.clone());
    let arguments = DefinitionPhaseArguments::Parse {
        unit,
        program: vec![klf_core::FileId::from_u32(99)],
    };

    assert!(matches!(
        arguments.marshal(&session),
        Err(DefinitionHandleError::FileNotRegistered { .. })
    ));
}

#[test]
fn phase_arguments_reject_unregistered_handles() {
    let session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    let unit = DefinitionUnit::new(klf_core::FileId::from_u32(7));
    let arguments = DefinitionPhaseArguments::Parse {
        unit,
        program: Vec::new(),
    };

    assert!(matches!(
        arguments.marshal(&session),
        Err(DefinitionHandleError::UnitNotRegistered { .. })
    ));
}
#[test]
fn phase_results_resolve_session_owned_modules() {
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    let file = session.add_file("input.calc", "1 + 1");
    let unit = DefinitionUnit::new(file);
    session.register_unit(unit.clone());
    let module = complete_image().module().clone();
    let module_id = session.store_artifact(module.clone());
    let arguments = DefinitionPhaseArguments::Lower {
        unit,
        program: DefinitionArtifact::new(&mut session, DefinitionUnit::new(file), ()),
    };

    let result = DefinitionPhaseResult::from_return_word(
        DefinitionPhase::Lower,
        &session,
        &arguments,
        Some(u64::try_from(module_id.index()).expect("artifact index fits u64")),
    )
    .expect("module handle resolves");
    assert_eq!(result, DefinitionPhaseResult::Module(Box::new(module)));
}
