//! Borrow-to-copy and copy-to-borrow conversion (section 14.1, O2): a
//! borrow of a scalar becomes a plain read, and a copy of an implicitly
//! copied value that is only read before its destroy becomes a borrow of
//! the original.

use klf_kir::parse_module;
use klf_kir_passes::{Backend, Level, optimize};
use klf_kir_verify::verify_module;
use klf_vm::{Outcome, run_module};

const CONVERTS: &str = r#"module @Converts lang "test" target "x86_64-unknown-linux-gnu" stage raw

type %Pt = struct ridl { a: i64, b: i64 }
policy { copy: implicit(@dup), destroy: trivial, relocate: bitwise, exclusivity: static, pairing: none }

func private @dup(%p: borrow %Pt) -> %Pt engine(runtime) {
bb0(%p: borrow %Pt):
  %c = copy %p
  return %c
}

func private @sum(%p: borrow %Pt) -> i64 engine(runtime) noinline {
bb0(%p: borrow %Pt):
  %a = struct.extract %p, 0
  %b = struct.extract %p, 1
  %s = add [wrap] %a, %b
  return %s
}

func private @consume(%p: %Pt) -> i64 engine(runtime) noinline {
bb0(%p: %Pt):
  %a = struct.extract %p, 0
  destroy %p
  return %a
}

func private @copied(%x: i64, %y: i64) -> i64 engine(runtime) {
bb0(%x: i64, %y: i64):
  %p = struct.new %Pt(%x, %y)
  %c = copy %p
  %s = call @sum(%c)
  destroy %c
  %t = call @consume(%p)
  %r = add [wrap] %s, %t
  return %r
}

func private @borrowed(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %r = heap.alloc.counted i64
  %slot = ref.addr %r
  store [init] %x to %slot
  %b = load.borrow %slot
  %d = add [wrap] %b, %b
  borrow.end %b
  destroy %r
  return %d
}
"#;

fn optimized(level: Level) -> klf_kir::Module {
    let mut module = parse_module(CONVERTS).expect("module parses");
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

/// `@copied` stays unrun: moving a `%Pt` into storage needs its copy
/// function, which the aggregate rewrite does not call yet.
#[test]
fn converted_functions_return_what_o0_returns() {
    for level in [Level::O0, Level::O1, Level::O2] {
        let mut module = optimized(level);
        module
            .functions
            .retain(|function| function.name.as_str() == "borrowed");
        assert_eq!(
            run_module(&module, "borrowed", &[21]).expect("module lowers"),
            Outcome::Return(Some(42)),
            "{level:?}\n{module}"
        );
    }
}

#[test]
fn o2_borrows_the_original_instead_of_copying_it() {
    let module = optimized(Level::O2);
    let copied = body(&module, "copied");
    assert!(!copied.contains("copy %"), "{copied}");
    assert!(!copied.contains("destroy %"), "{copied}");
    let borrowed = body(&module, "borrowed");
    assert!(!borrowed.contains("load.borrow"), "{borrowed}");
    assert!(!borrowed.contains("borrow.end"), "{borrowed}");
}
