//! Destroy insertion: raw KIR with unbalanced owned values becomes canonical
//! KIR that passes the verifier (V4) and runs balanced on the VM.

use klf_kir::parse_module;
use klf_kir_passes::canonicalize;
use klf_kir_verify::verify_module;
use klf_vm::{Outcome, run_module};

fn canonical_run(source: &str, entry: &str, arguments: &[u64]) -> Outcome {
    let raw = parse_module(source).expect("module parses");
    let (canonical, diagnostics) = canonicalize(&raw);
    assert!(
        diagnostics.is_empty(),
        "canonicalize diagnostics: {diagnostics:?}"
    );
    let verifier = verify_module(&canonical);
    assert!(
        verifier.is_empty(),
        "canonical KIR fails verification: {verifier:?}"
    );
    run_module(&canonical, entry, arguments).expect("module lowers")
}

const LEAK: &str = r#"module @Leak lang "test" target "aarch64-apple-darwin" stage raw

func private @leak() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted i64
  %z = const.int i64, 0
  return %z
}
"#;

const DIAMOND: &str = r#"module @Diamond lang "test" target "aarch64-apple-darwin" stage raw

func private @pick(%c: i1) -> i64 engine(runtime) {
bb0(%c: i1):
  %r = heap.alloc.counted i64
  cond.br %c, bb1(), bb2()
bb1():
  %a = const.int i64, 1
  return %a
bb2():
  %b = const.int i64, 2
  return %b
}
"#;

const CONDITIONAL: &str = r#"module @Conditional lang "test" target "aarch64-apple-darwin" stage raw

func private @maybe(%c: i1) -> i64 engine(runtime) {
bb0(%c: i1):
  %r = heap.alloc.counted i64
  cond.br %c, bb1(%r), bb2()
bb1(%s: ref i64):
  destroy %s
  %a = const.int i64, 1
  return %a
bb2():
  %b = const.int i64, 2
  return %b
}
"#;

#[test]
fn raw_leak_would_fail_the_heap_balance() {
    let raw = parse_module(LEAK).unwrap();
    assert_eq!(
        run_module(&raw, "leak", &[]).unwrap(),
        Outcome::Trap("KLF-RT-0100".to_owned())
    );
}

#[test]
fn diamond_destroys_before_the_branch() {
    assert_eq!(
        canonical_run(DIAMOND, "pick", &[1]),
        Outcome::Return(Some(1))
    );
    assert_eq!(
        canonical_run(DIAMOND, "pick", &[0]),
        Outcome::Return(Some(2))
    );
}

#[test]
fn conditional_consume_destroys_only_the_other_path() {
    assert_eq!(
        canonical_run(CONDITIONAL, "maybe", &[1]),
        Outcome::Return(Some(1))
    );
    assert_eq!(
        canonical_run(CONDITIONAL, "maybe", &[0]),
        Outcome::Return(Some(2))
    );
}
