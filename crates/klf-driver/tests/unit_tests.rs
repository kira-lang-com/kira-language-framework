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
    for file in ["language.kcore", "reader.kcore"] {
        std::fs::copy(mini.join(file), package.join(file)).expect("mini copied");
    }
    std::fs::write(package.join("tests/mini.kcore"), TESTS).expect("tests written");

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
        ["tests/mini.kcore:11:5: expectation failed: run(\"main 1\") == 4"]
    );
}

#[test]
fn kira_frontend_tests_do_not_run_against_a_legacy_definition() {
    let package = std::env::temp_dir().join(format!("klf-kira-unit-guard-{}", std::process::id()));
    std::fs::create_dir_all(package.join("app")).expect("app directory");
    std::fs::create_dir_all(package.join("tests")).expect("test directory");
    std::fs::write(package.join("package.kira"), "").expect("manifest");
    std::fs::write(package.join("app/Language.kira"), "").expect("Kira frontend");
    std::fs::write(package.join("tests/legacy.kcore"), TESTS).expect("legacy tests");
    let mut session = Session::new("x86_64-pc-windows-msvc".parse().expect("target"));
    let error = run_unit_tests(&mut session, &package).expect_err("unsupported Kira tests");
    assert!(error.contains("refusing to run the tests against a different definition"), "{error}");
    std::fs::remove_dir_all(package).expect("remove fixture");
}

#[test]
fn kira_only_tests_are_not_silently_skipped() {
    let package = std::env::temp_dir().join(format!("klf-kira-only-tests-{}", std::process::id()));
    std::fs::create_dir_all(package.join("app")).expect("app directory");
    std::fs::create_dir_all(package.join("tests")).expect("test directory");
    std::fs::write(package.join("package.kira"), "").expect("manifest");
    std::fs::write(package.join("app/Language.kira"), "").expect("frontend");
    std::fs::write(package.join("tests/unit.kira"), "@Test function example() {}").expect("test");
    let mut session = Session::new("x86_64-pc-windows-msvc".parse().expect("target"));
    run_unit_tests(&mut session, &package).expect_err("an invalid Kira frontend must not silently skip its tests");
    std::fs::remove_dir_all(package).expect("remove fixture");
}
