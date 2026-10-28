//! The generics engine: a generic call is monomorphized into a specialized copy
//! that verifies and runs, and the generic template is dropped.

use klf_kir::parse_module;
use klf_kir_passes::monomorphize;
use klf_kir_verify::verify_module;
use klf_vm::{Outcome, run_module};

const GENERIC: &str = r#"module @Gen lang "test" target "aarch64-apple-darwin" stage raw

func private @id<$T>(%x: $T) -> $T engine(runtime) {
bb0(%x: $T):
  return %x
}

func private @pick<$T>(%a: $T, %b: $T) -> $T engine(runtime) {
bb0(%a: $T, %b: $T):
  return %a
}

func private @main() -> i64 engine(runtime) {
bb0():
  %n = const.int i64, 42
  %m = const.int i64, 7
  %r = call @id<i64>(%n)
  %s = call @pick<i64>(%r, %m)
  return %s
}
"#;

#[test]
fn generic_syntax_round_trips() {
    let module = parse_module(GENERIC).unwrap();
    let reparsed = parse_module(&module.to_string()).unwrap();
    assert_eq!(module, reparsed);
}

#[test]
fn monomorphization_specializes_and_runs() {
    let module = parse_module(GENERIC).unwrap();
    let mono = monomorphize(&module);

    let names: Vec<&str> = mono.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"id$i64"), "expected id$i64, got {names:?}");
    assert!(
        names.contains(&"pick$i64"),
        "expected pick$i64, got {names:?}"
    );
    assert!(!names.contains(&"id"), "generic template should be dropped");
    assert!(
        !names.contains(&"pick"),
        "generic template should be dropped"
    );

    assert!(
        verify_module(&mono).is_empty(),
        "{:?}",
        verify_module(&mono)
    );
    assert_eq!(
        run_module(&mono, "main", &[]).unwrap(),
        Outcome::Return(Some(42))
    );
}
