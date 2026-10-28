//! Definite initialization (pass 1): loads and assigns require an initialized
//! slot; a second `store [init]` is a double init.

use klf_kir::parse_module;
use klf_kir_passes::canonicalize;

fn diagnostics(source: &str) -> Vec<String> {
    let raw = parse_module(source).unwrap();
    canonicalize(&raw)
        .1
        .into_iter()
        .map(|diagnostic| diagnostic.code.to_owned())
        .collect()
}

const OK: &str = r#"module @Init lang "test" target "aarch64-apple-darwin" stage raw

func private @ok() -> i64 engine(runtime) {
bb0():
  %s = stack.alloc i64
  %a = const.int i64, 5
  store [init] %a to %s
  %v = load [trivial] %s
  stack.dealloc %s
  return %v
}
"#;

const USE_BEFORE_INIT: &str = r#"module @Init lang "test" target "aarch64-apple-darwin" stage raw

func private @bad() -> i64 engine(runtime) {
bb0():
  %s = stack.alloc i64
  %v = load [trivial] %s
  stack.dealloc %s
  return %v
}
"#;

const DOUBLE_INIT: &str = r#"module @Init lang "test" target "aarch64-apple-darwin" stage raw

func private @bad() -> i64 engine(runtime) {
bb0():
  %s = stack.alloc i64
  %a = const.int i64, 1
  store [init] %a to %s
  store [init] %a to %s
  %v = load [trivial] %s
  stack.dealloc %s
  return %v
}
"#;

#[test]
fn initialized_slot_is_clean() {
    assert!(diagnostics(OK).is_empty(), "{:?}", diagnostics(OK));
}

#[test]
fn load_before_init_is_rejected() {
    assert!(diagnostics(USE_BEFORE_INIT).contains(&"KLF-KIR-1103".to_owned()));
}

#[test]
fn double_init_is_rejected() {
    assert!(diagnostics(DOUBLE_INIT).contains(&"KLF-KIR-1104".to_owned()));
}
