use klf_defsvc::FormatFailure;
use klf_driver::{Backend, LanguageDefinition};
use klf_format::Options;
use klf_services::{DefinitionPackage, Session};
use klf_target::Target;

/// Nested lists of numbers, `[1, [2, 3]]`, with `//` comments.
const LANGUAGE: &str = include_str!("lists.klfd");

fn format(source: &str, width: usize) -> Result<String, FormatFailure> {
    let package = DefinitionPackage::single("language.klfd", LANGUAGE);
    let target: Target = "aarch64-apple-darwin".parse().expect("valid target");
    let mut session = Session::new(target.clone());
    let language = LanguageDefinition::from_package(&mut session, &package, Backend::Vm)
        .unwrap_or_else(|| panic!("definition builds: {:?}", session.diagnostics()));
    assert!(language.formats());
    let options = Options {
        width,
        ..Options::default()
    };
    language.format_source(&mut Session::new(target), "input.lists", source, &options)
}

#[test]
fn a_format_hook_lays_out_and_the_kernel_places_comments() {
    assert_eq!(
        format("[1,2,  [3 ,4]]\n\n\n[5]", 80).expect("formats"),
        "[1, 2, [3, 4]]\n\n[5]\n"
    );
    assert_eq!(
        format("[10, 20, [30, 40]]", 12).expect("formats"),
        "[\n    10,\n    20,\n    [30, 40]\n]\n"
    );
    assert_eq!(
        format("// lists\n[1, // one\n2]", 80).expect("formats"),
        "// lists\n[\n    1, // one\n    2\n]\n"
    );
}
