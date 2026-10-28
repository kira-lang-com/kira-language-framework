//! Copy/move resolution: an implicit-copy value consumed twice gets a copy
//! inserted; an affine value consumed twice is rejected.

use klf_kir::parse_module;
use klf_kir_passes::canonicalize;
use klf_kir_verify::verify_module;
use klf_vm::{Outcome, run_module};

const TWICE_REF: &str = r#"module @TwiceRef lang "test" target "aarch64-apple-darwin" stage raw

func private @take(%x: ref i64) -> i64 engine(runtime) {
bb0(%x: ref i64):
  destroy %x
  %z = const.int i64, 0
  return %z
}

func private @twice() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted i64
  %a = call @take(%r)
  %b = call @take(%r)
  return %b
}
"#;

const TWICE_UNIQUE: &str = r#"module @TwiceUniq lang "test" target "aarch64-apple-darwin" stage raw

func private @take(%x: uniq i64) -> i64 engine(runtime) {
bb0(%x: uniq i64):
  destroy %x
  %z = const.int i64, 0
  return %z
}

func private @twice() -> i64 engine(runtime) {
bb0():
  %u = heap.alloc.unique i64
  %a = call @take(%u)
  %b = call @take(%u)
  return %b
}
"#;

#[test]
fn implicit_ref_consumed_twice_gets_a_copy() {
    let raw = parse_module(TWICE_REF).unwrap();
    let (canonical, diagnostics) = canonicalize(&raw);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(verify_module(&canonical).is_empty(), "verification failed");
    assert_eq!(
        run_module(&canonical, "twice", &[]).unwrap(),
        Outcome::Return(Some(0))
    );
}

#[test]
fn affine_unique_consumed_twice_is_rejected() {
    let raw = parse_module(TWICE_UNIQUE).unwrap();
    let (_canonical, diagnostics) = canonicalize(&raw);
    assert!(
        diagnostics.iter().any(|d| d.code == "KLF-KIR-1101"),
        "expected own.use-after-move, got {diagnostics:?}"
    );
}
