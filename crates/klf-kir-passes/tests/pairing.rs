//! Retain and release pairing (section 14.3): a copy of a counted value that
//! a destroy of either value follows cancels against it, unless something
//! between could release a value or read the count.

use klf_kir::parse_module;
use klf_kir_passes::{Backend, Level, optimize};
use klf_kir_verify::verify_module;
use klf_vm::{Outcome, run_module};

const PAIRS: &str = r#"module @Pairs lang "test" target "x86_64-unknown-linux-gnu" stage raw

func private @opaque(%x: i64) -> i64 engine(runtime) noinline {
bb0(%x: i64):
  return %x
}

func private @unique(%r: ref i64) -> i64 engine(runtime) {
bb0(%r: ref i64):
  %u = ref.isunique %r
  cond.br %u, bb1(), bb2()
bb1():
  %one = const.int i64, 1
  destroy %r
  return %one
bb2():
  %zero = const.int i64, 0
  destroy %r
  return %zero
}

func private @copy_released(%v: i64) -> i64 engine(runtime) {
bb0(%v: i64):
  %r = heap.alloc.counted i64
  %a = ref.addr %r
  store [init] %v to %a
  %c = copy %r
  %ca = ref.addr %c
  %read = load [trivial] %ca
  destroy %c
  %u = call @unique(%r)
  %sum = add [wrap] %read, %u
  return %sum
}

func private @original_released(%v: i64) -> i64 engine(runtime) {
bb0(%v: i64):
  %r = heap.alloc.counted i64
  %a = ref.addr %r
  store [init] %v to %a
  %c = copy %r
  destroy %r
  %u = call @unique(%c)
  return %u
}

func private @call_between(%v: i64) -> i64 engine(runtime) {
bb0(%v: i64):
  %r = heap.alloc.counted i64
  %a = ref.addr %r
  store [init] %v to %a
  %c = copy %r
  %x = call @opaque(%v)
  destroy %c
  %u = call @unique(%r)
  %sum = add [wrap] %x, %u
  return %sum
}

func private @count_read_between(%v: i64) -> i64 engine(runtime) {
bb0(%v: i64):
  %r = heap.alloc.counted i64
  %a = ref.addr %r
  store [init] %v to %a
  %c = copy %r
  %shared = ref.isunique %r
  destroy %c
  %u = call @unique(%r)
  cond.br %shared, bb1(%u), bb2(%u)
bb1(%p: i64):
  return %p
bb2(%q: i64):
  %ten = const.int i64, 10
  %s = add [wrap] %q, %ten
  return %s
}

func private @released_on_each_path(%v: i64) -> i64 engine(runtime) {
bb0(%v: i64):
  %r = heap.alloc.counted i64
  %a = ref.addr %r
  store [init] %v to %a
  %c = copy %r
  %ten = const.int i64, 10
  %small = icmp slt %v, %ten
  cond.br %small, bb1(), bb2()
bb1():
  destroy %c
  %u = call @unique(%r)
  return %u
bb2():
  destroy %r
  %w = call @unique(%c)
  %s = add [wrap] %w, %ten
  return %s
}
"#;

fn optimized(level: Level) -> klf_kir::Module {
    let mut module = parse_module(PAIRS).expect("module parses");
    optimize(&mut module, level, Backend::Vm);
    let diagnostics = verify_module(&module);
    assert!(diagnostics.is_empty(), "{diagnostics:?}\n{module}");
    module
}

fn body(module: &klf_kir::Module, name: &str) -> String {
    let function = module
        .functions
        .iter()
        .find(|function| function.name.as_str() == name)
        .expect("the function exists");
    let mut single = module.clone();
    single.functions = vec![function.clone()];
    single.to_string()
}

#[test]
fn paired_functions_return_what_o0_returns() {
    let cases = [
        ("copy_released", 8),
        ("original_released", 1),
        ("call_between", 8),
        ("count_read_between", 11),
        ("released_on_each_path", 1),
    ];
    for level in [Level::O0, Level::O1, Level::O2] {
        let module = optimized(level);
        for (name, want) in cases {
            assert_eq!(
                run_module(&module, name, &[7]).expect("module lowers"),
                Outcome::Return(Some(want)),
                "{name} at {level:?}"
            );
        }
    }
}

#[test]
fn o1_cancels_a_copy_a_destroy_follows() {
    let module = optimized(Level::O1);
    for name in [
        "copy_released",
        "original_released",
        "released_on_each_path",
    ] {
        let text = body(&module, name);
        assert!(!text.contains("copy %"), "{text}");
    }
}

#[test]
fn a_call_or_a_count_read_keeps_the_pair() {
    let module = optimized(Level::O1);
    for name in ["call_between", "count_read_between"] {
        let text = body(&module, name);
        assert!(text.contains("copy %"), "{text}");
    }
}

const LENT: &str = r#"module @Lent lang "test" target "x86_64-unknown-linux-gnu" stage raw

func private @peek(%r: ref i64) -> i64 engine(runtime) noinline {
bb0(%r: ref i64):
  %a = ref.addr %r
  %v = load [trivial] %a
  destroy %r
  return %v
}

func private @caller(%v: i64) -> i64 engine(runtime) {
bb0(%v: i64):
  %r = heap.alloc.counted i64
  %a = ref.addr %r
  store [init] %v to %a
  %c = copy %r
  %seen = call @peek(%c)
  %u = ref.isunique %r
  destroy %r
  %one = const.int i64, 1
  %zero = const.int i64, 0
  %bit = select %u, %one, %zero
  %r2 = add [wrap] %seen, %bit
  return %r2
}
"#;

#[test]
fn an_argument_the_callee_only_destroys_is_lent() {
    for level in [Level::O0, Level::O2] {
        let mut module = parse_module(LENT).expect("module parses");
        optimize(&mut module, level, Backend::Vm);
        let diagnostics = verify_module(&module);
        assert!(diagnostics.is_empty(), "{diagnostics:?}\n{module}");
        assert_eq!(
            run_module(&module, "caller", &[41]).expect("module lowers"),
            Outcome::Return(Some(42)),
            "{level:?}"
        );
        if level == Level::O2 {
            let caller = body(&module, "caller");
            assert!(!caller.contains("copy %"), "{caller}");
            let peek = body(&module, "peek");
            assert!(peek.contains("borrow ref i64"), "{peek}");
            assert!(!peek.contains("destroy"), "{peek}");
        }
    }
}
