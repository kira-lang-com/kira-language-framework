//! The kernel loads a `.klfd` package, finds its single `Language`
//! declaration, reads the section-7 metadata and drives the four phases.

use klf_driver::{Backend, LanguageDefinition, load_package, run};
use klf_services::{DefaultConvention, DefinitionSource, Exclusivity, MemoryModel, Session};
use std::path::PathBuf;

fn package_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../definitions/mini")
}

fn session() -> Session {
    Session::new("aarch64-apple-darwin".parse().expect("valid target"))
}

#[test]
fn package_loading_skips_std_and_corpus() {
    let package = load_package(&package_root()).expect("package loads");
    let names: Vec<&str> = package
        .sources()
        .iter()
        .map(|source| source.name.as_str())
        .collect();
    assert_eq!(names, ["language.klfd", "reader.klfd"]);
}

#[test]
fn language_definition_drives_the_four_phases() {
    let mut session = session();
    let language = LanguageDefinition::load(&mut session, &package_root(), Backend::Vm)
        .unwrap_or_else(|| panic!("definition loads: {:?}", session.diagnostics()));

    let descriptor = language.descriptor();
    assert_eq!(descriptor.name, "mini");
    assert_eq!(descriptor.driver, "mini");
    assert_eq!(descriptor.version, "0.1.0");
    assert_eq!(descriptor.extensions, ["mini", "minic"]);
    assert_eq!(descriptor.memory, MemoryModel::Unmanaged);
    assert_eq!(descriptor.exclusivity, Exclusivity::None);
    assert_eq!(descriptor.convention, DefaultConvention::Ridl);
    assert_eq!(descriptor.std_root, "std");
    let mode = descriptor.file_mode("raw").expect("raw file mode");
    assert_eq!(mode.pragma, "mini raw");
    assert_eq!(mode.flag, "-fmini-raw");
    assert_eq!(mode.memory, Some(MemoryModel::Unmanaged));
    assert_eq!(mode.convention, None);
    assert!(
        mode.extensions.is_empty(),
        "a pragma-only mode keys on nothing"
    );

    // A mode may instead be keyed by extension, which is how KArc gives `.krc`
    // its memory model without a pragma in every file (section 7.3).
    let counted = descriptor.file_mode("counted").expect("counted file mode");
    assert_eq!(counted.extensions, ["minic"]);
    assert_eq!(counted.memory, Some(MemoryModel::Counted));
    assert_eq!(counted.convention, Some(DefaultConvention::Platform));

    let module = language
        .compile_sources(
            &mut session,
            &[DefinitionSource::new("input.mini", "answer 40")],
        )
        .unwrap_or_else(|| panic!("definition compiles: {:?}", session.diagnostics()));
    assert_eq!(session.diagnostics(), []);
    assert_eq!(
        run(&module, "answer", &[], Backend::Vm).expect("lowered module runs"),
        klf_vm::Outcome::Return(Some(42))
    );
}
