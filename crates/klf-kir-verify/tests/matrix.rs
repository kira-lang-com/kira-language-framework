//! The section-12 fixture matrix.
//!
//! Every file in `fixtures/matrix` is checked by one of two rules. A
//! `*.positive.kir` module must verify without a diagnostic and must print the
//! same text after a reparse. A `*.negative.kir` module must
//! report the diagnostic code named in its first `// expect` comment.

use klf_kir::parse_module;
use klf_kir_verify::verify_module;
use std::path::Path;

#[test]
fn matrix_fixtures_behave_as_declared() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/matrix");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("fixture directory") {
        let path = entry.expect("directory entry").path();
        let name = path.file_name().expect("file name").to_string_lossy();
        if !name.ends_with(".kir") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("fixture text");
        if name.ends_with(".positive.kir") {
            check_positive(&name, &source);
        } else {
            check_negative(&name, &source);
        }
        checked += 1;
    }
    assert!(checked > 0, "the fixture matrix is empty");
}

fn check_positive(name: &str, source: &str) {
    let module = match parse_module(source) {
        Ok(module) => module,
        Err(error) => panic!("{name}: {error}"),
    };
    let diagnostics = verify_module(&module);
    assert!(
        diagnostics.is_empty(),
        "{name}: expected no diagnostics, got {:?}",
        diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
            .collect::<Vec<_>>()
    );
    let printed = module.to_string();
    let reparsed = match parse_module(&printed) {
        Ok(reparsed) => reparsed,
        Err(error) => panic!("{name}: reparse failed: {error}\n{printed}"),
    };
    assert_eq!(
        printed,
        reparsed.to_string(),
        "{name}: round trip changed the module"
    );
}

fn check_negative(name: &str, source: &str) {
    let expected = expected_code(source)
        .unwrap_or_else(|| panic!("{name}: a negative fixture needs an `// expect <code>` line"));
    let module = match parse_module(source) {
        Ok(module) => module,
        Err(error) => panic!("{name}: {error}"),
    };
    let diagnostics = verify_module(&module);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == expected),
        "{name}: expected {expected}, got {:?}",
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>()
    );
}

fn expected_code(source: &str) -> Option<&str> {
    source
        .lines()
        .find_map(|line| line.trim().strip_prefix("// expect "))
        .map(str::trim)
}
