//! `klf test`'s unit tests (`KLF-TESTING.md`) over the `mini` language: a
//! passing test passes, and a false `expect!` fails its test with the
//! condition's text and position.

use klf_driver::run_unit_tests;
use klf_services::Session;
use std::path::Path;

const TESTS: &str = r#"import Testing

@Test
function mainAddsTwo() {
    expect!(run("main 40") == 42)
    expect!(compiles("main 1"))
}

@Test
function aFalseExpectationIsAnIssue() {
    expect!(run("main 1") == 4)
    expect!(run("""
        main 5
        """) == 7)
}
"#;

#[test]
fn tests_run_and_a_false_expectation_fails_where_it_is_written() {
    let mini = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../definitions/mini");
    let package = std::env::temp_dir().join(format!("klf-unit-tests-{}", std::process::id()));
    std::fs::create_dir_all(package.join("tests")).expect("package directory");
    for file in ["language.klfd", "reader.klfd"] {
        std::fs::copy(mini.join(file), package.join(file)).expect("mini copied");
    }
    std::fs::write(package.join("tests/mini.klfd"), TESTS).expect("tests written");

    let mut session = Session::new("aarch64-apple-darwin".parse().expect("target"));
    let results = run_unit_tests(&mut session, &package)
        .unwrap_or_else(|error| panic!("{error}: {:?}", session.diagnostics()));
    let _ = std::fs::remove_dir_all(&package);

    let names: Vec<&str> = results.iter().map(|result| result.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "tests/mini::mainAddsTwo",
            "tests/mini::aFalseExpectationIsAnIssue"
        ]
    );
    assert!(
        results[0].mismatches.is_empty(),
        "{:?}",
        results[0].mismatches
    );
    assert_eq!(
        results[1].mismatches,
        ["tests/mini.klfd:11:5: expectation failed: run(\"main 1\") == 4"]
    );
}
