//! The frontend's KIR survives the text round-trip and the ownership +
//! verifier pipeline the backends require, then still runs on the VM.

use klf_kir_passes::canonicalize;
use klf_kir_verify::verify_module;
use klf_kira_lower::lower_file;
use klf_kira_syntax::parse;
use klf_services::Session;
use klf_vm::{Outcome, run_module};

const SOURCE: &str = "
function add(a: Int, b: Int) -> Int { return a + b }
function sumto(n: Int) -> Int {
    var s = 0
    var i = 0
    while i < n {
        s = add(s, i)
        i = i + 1
    }
    return s
}";

fn lower() -> klf_kir::Module {
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    let mut session = Session::new(target);
    let id = session.add_file("t.klfd", SOURCE);
    let parsed = parse(session.file(id));
    assert!(
        parsed.diagnostics.is_empty(),
        "parse: {:?}",
        parsed.diagnostics
    );
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    lower_file(&mut session, "Test", target, &parsed.file)
        .unwrap_or_else(|| panic!("lower: {:?}", session.diagnostics()))
}

#[test]
fn canonicalizes_and_verifies_clean() {
    let module = lower();
    let (canonical, diagnostics) = canonicalize(&module);
    assert!(diagnostics.is_empty(), "canonicalize: {diagnostics:?}");
    let verifier = verify_module(&canonical);
    assert!(verifier.is_empty(), "verify: {verifier:?}");
    let outcome = run_module(&canonical, "sumto", &[6]).expect("runs");
    assert_eq!(outcome, Outcome::Return(Some(15)));
}
