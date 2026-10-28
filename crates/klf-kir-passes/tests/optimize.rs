//! The section 14.1 optimizer: an optimized module behaves as the O0 one,
//! with stack slots promoted, small callees inlined and repeated loads gone.

use klf_kir::parse_module;
use klf_kir_passes::{Backend, Level, optimize};
use klf_kir_verify::verify_module;
use klf_vm::{Outcome, run_module};

const LOOP: &str = r#"module @Loop lang "test" target "x86_64-unknown-linux-gnu" stage raw

type %Pair = struct ridl { low: i64, high: i64 }
policy { copy: trivial, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none }

func private @step(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %one = const.int i64, 1
  %y = add [wrap] %x, %one
  return %y
}

func private @sum(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  %i = stack.alloc i64
  %total = stack.alloc i64
  %zero = const.int i64, 0
  store [trivial] %zero to %i
  store [trivial] %zero to %total
  br bb1()
bb1():
  %iv = load [trivial] %i
  %more = icmp slt %iv, %n
  cond.br %more, bb2(), bb3()
bb2():
  %tv = load [trivial] %total
  %again = load [trivial] %i
  %next = add [wrap] %tv, %again
  store [trivial] %next to %total
  %stepped = call @step(%iv)
  store [trivial] %stepped to %i
  br bb1()
bb3():
  %result = load [trivial] %total
  return %result
}

func private @fields(%a: i64, %b: i64) -> i64 engine(runtime) {
bb0(%a: i64, %b: i64):
  %pair = stack.alloc %Pair
  %low = field.addr %pair, 0
  store [trivial] %a to %low
  %high = field.addr %pair, 1
  store [trivial] %b to %high
  %again = field.addr %pair, 0
  %l = load [trivial] %again
  %h = load [trivial] %high
  %r = sub [wrap] %h, %l
  return %r
}
"#;

fn optimized(source: &str, level: Level) -> klf_kir::Module {
    let mut module = parse_module(source).expect("module parses");
    optimize(&mut module, level, Backend::Cranelift);
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
fn optimized_loop_returns_what_o0_returns() {
    for level in [Level::O0, Level::O1, Level::O2] {
        let module = optimized(LOOP, level);
        assert_eq!(
            run_module(&module, "sum", &[10]).expect("module lowers"),
            Outcome::Return(Some(45)),
            "{level:?}"
        );
        assert_eq!(
            run_module(&module, "fields", &[3, 10]).expect("module lowers"),
            Outcome::Return(Some(7)),
            "{level:?}"
        );
    }
}

#[test]
fn o2_promotes_slots_and_inlines_small_callees() {
    let module = optimized(LOOP, Level::O2);
    let sum = body(&module, "sum");
    assert!(!sum.contains("stack.alloc"), "{sum}");
    assert!(!sum.contains("call @step"), "{sum}");
    let fields = body(&module, "fields");
    assert!(!fields.contains("load"), "{fields}");
}

const OVERFLOW: &str = r#"module @Overflow lang "test" target "x86_64-unknown-linux-gnu" stage raw

func private @slow(%a: i64, %b: i64) -> i64 engine(runtime) {
bb0(%a: i64, %b: i64):
  %r = sub [wrap] %a, %b
  return %r
}

func private @checked(%n: i64, %start: i64) -> i64 engine(runtime) {
bb0(%n: i64, %start: i64):
  %zero = const.int i64, 0
  br bb1(%zero, %start)
bb1(%i: i64, %a: i64):
  %more = icmp slt %i, %n
  cond.br %more, bb2(), bb4(%a)
bb2():
  %r = add [wrap] %a, %i
  %x1 = xor [wrap] %a, %r
  %x2 = xor [wrap] %i, %r
  %both = and [wrap] %x1, %x2
  %none = const.int i64, 0
  %exact = icmp sge %both, %none
  %one = const.int i64, 1
  %next = add [wrap] %i, %one
  cond.br %exact, bb1(%next, %r), bb3(%a, %i, %next)
bb3(%sa: i64, %si: i64, %sn: i64):
  %s = call @slow(%sa, %si)
  br bb1(%sn, %s)
bb4(%result: i64):
  return %result
}

func private @carried(%n: i64, %start: i64) -> i64 engine(runtime) {
bb0(%n: i64, %start: i64):
  %zero = const.int i64, 0
  br bb1(%zero, %start)
bb1(%i: i64, %a: i64):
  %more = icmp slt %i, %n
  cond.br %more, bb2(), bb5(%a)
bb2():
  (%r, %f) = add.ovf s %a, %i
  cond.br %f, bb3(%a, %i), bb4()
bb4():
  %one = const.int i64, 1
  (%next, %g) = add.ovf s %i, %one
  cond.br %g, bb3(%r, %i), bb1(%next, %r)
bb3(%sa: i64, %si: i64):
  %s = call @slow(%sa, %si)
  %step = const.int i64, 1
  %sn = add [wrap] %si, %step
  br bb1(%sn, %s)
bb5(%result: i64):
  return %result
}
"#;

#[test]
fn overflow_paths_keep_their_results() {
    let start = (i64::MAX - 3).cast_unsigned();
    let expected = |name: &str| {
        run_module(&optimized(OVERFLOW, Level::O0), name, &[6, start]).expect("module lowers")
    };
    for name in ["checked", "carried"] {
        let module = optimized(OVERFLOW, Level::O2);
        let got = run_module(&module, name, &[6, start]).expect("module lowers");
        assert_eq!(got, expected(name), "{name}\n{}", body(&module, name));
    }
}

#[test]
fn overflow_tests_read_machine_flags_and_hand_off_through_slots() {
    let module = optimized(OVERFLOW, Level::O2);
    let checked = body(&module, "checked");
    assert!(checked.contains("add.ovf"), "{checked}");
    assert!(!checked.contains("xor"), "{checked}");
    let carried = body(&module, "carried");
    assert!(carried.contains(".handed = stack.alloc"), "{carried}");
    assert!(carried.contains("sub [wrap]"), "{carried}");
}

const INVARIANT: &str = r#"module @Invariant lang "test" target "x86_64-unknown-linux-gnu" stage raw

func private @scaled(%n: i64, %k: i64) -> i64 engine(runtime) {
bb0(%n: i64, %k: i64):
  %zero = const.int i64, 0
  br bb1(%zero, %zero)
bb1(%i: i64, %total: i64):
  %more = icmp slt %i, %n
  cond.br %more, bb2(), bb3()
bb2():
  %square = mul [wrap] %k, %k
  %three = const.int i64, 3
  %factor = add [wrap] %square, %three
  %term = mul [wrap] %i, %factor
  %next = add [wrap] %total, %term
  %one = const.int i64, 1
  %stepped = add [wrap] %i, %one
  br bb1(%stepped, %next)
bb3():
  return %total
}
"#;

#[test]
fn loop_invariant_arithmetic_leaves_the_loop() {
    for level in [Level::O0, Level::O1, Level::O2] {
        let module = optimized(INVARIANT, level);
        assert_eq!(
            run_module(&module, "scaled", &[4, 5]).expect("module lowers"),
            Outcome::Return(Some(168)),
            "{level:?}"
        );
        if level != Level::O2 {
            continue;
        }
        let function = &module.functions[0];
        let squares_k = |instruction: &klf_kir::Instruction| {
            matches!(
                &instruction.operation,
                klf_kir::Operation::Binary { lhs, rhs, .. }
                    if lhs.as_str() == "k" && rhs.as_str() == "k"
            )
        };
        assert!(
            function.blocks[0].instructions.iter().any(squares_k),
            "`%k * %k` moved to the entry block:\n{}",
            body(&module, "scaled")
        );
        assert!(
            function.blocks[1..]
                .iter()
                .all(|block| !block.instructions.iter().any(squares_k)),
            "no loop block squares `%k`:\n{}",
            body(&module, "scaled")
        );
    }
}

const WITNESS: &str = r#"module @Witness lang "test" target "x86_64-unknown-linux-gnu" stage raw

trait %Describe { associated: [], methods: [size : fn(i64) -> i64, twice : fn(i64) -> i64] }
witness i64 : %Describe { witnesses: [@size_i64, @twice_i64], associated: [] }

func private @size_i64(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %eight = const.int i64, 8
  return %eight
}

func private @twice_i64(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %y = add [wrap] %x, %x
  return %y
}

func module @describe(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %t = call.witness i64, %Describe.twice(%x) -> i64
  %s = call.witness i64, %Describe.size(%x) -> i64
  %r = add [wrap] %t, %s
  return %r
}
"#;

#[test]
fn o2_calls_a_concrete_witness_directly() {
    let module = optimized(WITNESS, Level::O2);
    let text = body(&module, "describe");
    assert!(!text.contains("call.witness"), "{text}");
    assert_eq!(
        run_module(&module, "describe", &[20]).expect("module lowers"),
        Outcome::Return(Some(48))
    );
}

const UNIQUE: &str = r#"module @Unique lang "test" target "x86_64-unknown-linux-gnu" stage raw

func private @fresh(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %cell = heap.alloc.counted i64
  %addr = ref.addr %cell
  store [trivial] %x to %addr
  %only = ref.isunique %cell
  %alias = copy %cell
  %shared = ref.isunique %cell
  destroy %alias
  destroy %cell
  %one = const.int i64, 1
  %ten = const.int i64, 10
  %zero = const.int i64, 0
  %a = select %only, %one, %zero
  %b = select %shared, %ten, %zero
  %r = add [wrap] %a, %b
  return %r
}
"#;

#[test]
fn o2_knows_a_fresh_allocation_is_unique() {
    for level in [Level::O0, Level::O2] {
        let module = optimized(UNIQUE, level);
        assert_eq!(
            run_module(&module, "fresh", &[5]).expect("module lowers"),
            Outcome::Return(Some(1)),
            "{level:?}"
        );
    }
    let text = body(&optimized(UNIQUE, Level::O2), "fresh");
    assert_eq!(text.matches("ref.isunique").count(), 1, "{text}");
}

/// One allocation checked after a branch: on `paths` nothing copies it on
/// either path, so the check after the join is known; on `copied` one path
/// copies it, so the check stays.
const UNIQUE_ACROSS_BLOCKS: &str = r#"module @UniqueAcross lang "test" target "x86_64-unknown-linux-gnu" stage raw

func private @paths(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %cell = heap.alloc.counted i64
  %addr = ref.addr %cell
  store [trivial] %x to %addr
  %zero = const.int i64, 0
  %small = icmp slt %x, %zero
  cond.br %small, bb1, bb2
bb1:
  %one = const.int i64, 1
  store [assign] %one to %addr
  br bb3
bb2:
  br bb3
bb3:
  %only = ref.isunique %cell
  %v = load [trivial] %addr
  destroy %cell
  %ten = const.int i64, 10
  %a = select %only, %ten, %zero
  %r = add [wrap] %a, %v
  return %r
}

func private @copied(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %cell = heap.alloc.counted i64
  %addr = ref.addr %cell
  store [trivial] %x to %addr
  %zero = const.int i64, 0
  %small = icmp slt %x, %zero
  cond.br %small, bb1, bb2
bb1:
  %alias = copy %cell
  br bb3(%alias)
bb2:
  %same = copy %cell
  destroy %same
  br bb3(%cell)
bb3(%held: ref i64):
  %only = ref.isunique %held
  destroy %held
  %ten = const.int i64, 10
  %r = select %only, %ten, %zero
  return %r
}
"#;

#[test]
fn o2_follows_a_fresh_allocation_across_blocks() {
    for level in [Level::O0, Level::O2] {
        let module = optimized(UNIQUE_ACROSS_BLOCKS, level);
        assert_eq!(
            run_module(&module, "paths", &[5]).expect("module lowers"),
            Outcome::Return(Some(15)),
            "{level:?}"
        );
        assert_eq!(
            run_module(&module, "copied", &[5]).expect("module lowers"),
            Outcome::Return(Some(10)),
            "{level:?}"
        );
    }
    let module = optimized(UNIQUE_ACROSS_BLOCKS, Level::O2);
    let paths = body(&module, "paths");
    assert!(
        paths.contains("cond.br"),
        "the check still follows a join: {paths}"
    );
    assert!(!paths.contains("ref.isunique"), "{paths}");
    let copied = body(&module, "copied");
    assert!(copied.contains("ref.isunique"), "{copied}");
}
