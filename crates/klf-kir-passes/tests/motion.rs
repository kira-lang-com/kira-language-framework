//! Destroy hoisting and copy sinking (sections 14.1 and 14.3): a destroy
//! moves up to its value's last use and a copy down to its first, across
//! instructions that cannot release a value or read a count, unless the
//! module keeps `lifetimes(scoped)`.

use klf_kir::parse_module;
use klf_kir_passes::{Backend, Level, optimize};
use klf_kir_verify::verify_module;
use klf_vm::{Outcome, run_module};

const MOVES: &str = r#"module @Moves lang "test" target "x86_64-unknown-linux-gnu" stage raw
ATTRIBUTE
func private @keep(%r: ref i64) -> i64 engine(runtime) noinline {
bb0(%r: ref i64):
  %a = ref.addr %r
  %v = load [trivial] %a
  %u = ref.isunique %r
  %one = const.int i64, 1
  %zero = const.int i64, 0
  %bit = select %u, %one, %zero
  %b = add [wrap] %v, %bit
  destroy %r
  return %b
}

func private @moves(%v: i64, %w: i64) -> i64 engine(runtime) {
bb0(%v: i64, %w: i64):
  %r = heap.alloc.counted i64
  %a = ref.addr %r
  store [init] %v to %a
  %x = load [trivial] %a
  %c = copy %r
  %y = mul [wrap] %w, %w
  %z = add [wrap] %y, %w
  %k = call @keep(%c)
  %s = add [wrap] %x, %k
  %t = mul [wrap] %z, %z
  destroy %r
  %u = add [wrap] %s, %t
  return %u
}
"#;

fn optimized(attribute: &str, level: Level) -> klf_kir::Module {
    let text = MOVES.replace("ATTRIBUTE", attribute);
    let mut module = parse_module(&text).expect("module parses");
    optimize(&mut module, level, Backend::Vm);
    let diagnostics = verify_module(&module);
    assert!(diagnostics.is_empty(), "{diagnostics:?}\n{module}");
    module
}

fn lines(module: &klf_kir::Module) -> Vec<String> {
    let function = module
        .functions
        .iter()
        .find(|function| function.name.as_str() == "moves")
        .expect("the function exists");
    function.blocks[0]
        .instructions
        .iter()
        .map(|instruction| format!("{:?}", instruction.operation))
        .collect()
}

fn place(lines: &[String], prefix: &str) -> usize {
    lines
        .iter()
        .position(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix} in {lines:#?}"))
}

#[test]
fn every_level_returns_what_o0_returns() {
    for attribute in ["", "attribute lifetimes = \"scoped\"\n"] {
        for level in [Level::O0, Level::O1, Level::O2] {
            let module = optimized(attribute, level);
            assert_eq!(
                run_module(&module, "moves", &[5, 3]).expect("module lowers"),
                Outcome::Return(Some(5 + 5 + 144)),
                "{level:?}\n{module}"
            );
        }
    }
}

#[test]
fn o2_destroys_after_the_last_use_and_copies_before_the_first() {
    let module = optimized("", Level::O2);
    let lines = lines(&module);
    let destroy = place(&lines, "Destroy");
    let copy = place(&lines, "Copy");
    let call = place(&lines, "Call");
    assert_eq!(copy + 1, call, "{lines:#?}");
    assert_eq!(call + 1, destroy, "{lines:#?}");
}

#[test]
fn a_scoped_module_keeps_its_destroys() {
    let module = optimized("attribute lifetimes = \"scoped\"\n", Level::O2);
    let lines = lines(&module);
    let destroy = place(&lines, "Destroy");
    assert!(destroy + 3 >= lines.len(), "{lines:#?}");
}
