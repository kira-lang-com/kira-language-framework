use klf_kir::parse_module;
use klf_vm::{Outcome, run_module};

fn run(source: &str, entry: &str, arguments: &[u64]) -> Outcome {
    let module = parse_module(source).expect("module parses");
    run_module(&module, entry, arguments).expect("module lowers")
}

const ARITHMETIC: &str = r#"module @Arith lang "test" target "aarch64-apple-darwin" stage raw

func private @add(%lhs: i64, %rhs: i64) -> i64 engine(runtime) {
bb0(%lhs: i64, %rhs: i64):
  %sum = add %lhs, %rhs
  return %sum
}
"#;

#[test]
fn adds_two_integers() {
    assert_eq!(run(ARITHMETIC, "add", &[7, 5]), Outcome::Return(Some(12)));
}

const SUM: &str = r#"module @Sum lang "test" target "aarch64-apple-darwin" stage raw

func private @sumto(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  %zero = const.int i64, 0
  %one = const.int i64, 1
  br bb1(%zero, %one)
bb1(%acc: i64, %i: i64):
  %done = icmp sgt %i, %n
  cond.br %done, bb2(%acc), bb3(%acc, %i)
bb2(%result: i64):
  return %result
bb3(%acc2: i64, %i2: i64):
  %next_acc = add %acc2, %i2
  %next_i = add %i2, %one
  br bb1(%next_acc, %next_i)
}
"#;

#[test]
fn sums_a_loop() {
    assert_eq!(run(SUM, "sumto", &[5]), Outcome::Return(Some(15)));
    assert_eq!(run(SUM, "sumto", &[100]), Outcome::Return(Some(5050)));
}

const DIV_ZERO: &str = r#"module @DivZero lang "test" target "aarch64-apple-darwin" stage raw

func private @divzero(%a: i64) -> i64 engine(runtime) {
bb0(%a: i64):
  %z = const.int i64, 0
  %r = sdiv %a, %z
  return %r
}
"#;

#[test]
fn division_by_zero_traps() {
    assert_eq!(
        run(DIV_ZERO, "divzero", &[10]),
        Outcome::Trap("KLF-RT-0010".to_owned())
    );
}

const GLOBALS: &str = r#"module @Globals lang "test" target "aarch64-apple-darwin" stage raw

global @counter : i32 = 7

func private @read() -> i32 engine(runtime) {
bb0:
  %p = global.addr @counter
  %v = load [trivial] %p
  return %v
}

func private @bump() -> i32 engine(runtime) {
bb0:
  %p = global.addr @counter
  %n = const.int i32, 35
  store [assign] %n to %p
  %v = load [trivial] %p
  return %v
}
"#;

const INDIRECT: &str = r#"module @Indirect lang "test" target "aarch64-apple-darwin" stage raw

func private @double(%x: i32) -> i32 engine(runtime) {
bb0(%x: i32):
  %two = const.int i32, 2
  %r = mul %x, %two
  return %r
}

func private @triple(%x: i32) -> i32 engine(runtime) {
bb0(%x: i32):
  %three = const.int i32, 3
  %r = mul %x, %three
  return %r
}

func private @apply(%pick: i32, %v: i32) -> i32 engine(runtime) {
bb0(%pick: i32, %v: i32):
  %zero = const.int i32, 0
  %isdouble = icmp eq %pick, %zero
  cond.br %isdouble, bb1(), bb2()
bb1():
  %fd = func.ref @double
  %rd = call.indirect %fd(%v) : (i32) -> i32
  return %rd
bb2():
  %ft = func.ref @triple
  %rt = call.indirect %ft(%v) : (i32) -> i32
  return %rt
}
"#;

#[test]
fn calls_through_a_function_pointer() {
    assert_eq!(run(INDIRECT, "apply", &[0, 21]), Outcome::Return(Some(42)));
    assert_eq!(run(INDIRECT, "apply", &[1, 10]), Outcome::Return(Some(30)));
    assert_eq!(run(STORED_FP, "run", &[21]), Outcome::Return(Some(22)));
}

const STORED_FP: &str = r#"module @StoredFp lang "test" target "aarch64-apple-darwin" stage raw

func private @inc(%x: i32) -> i32 engine(runtime) {
bb0(%x: i32):
  %one = const.int i32, 1
  %r = add %x, %one
  return %r
}

func private @run(%v: i32) -> i32 engine(runtime) {
bb0(%v: i32):
  %slot = stack.alloc fn(i32) -> i32
  %fp = func.ref @inc
  store [trivial] %fp to %slot
  %loaded = load [trivial] %slot
  %r = call.indirect %loaded(%v) : (i32) -> i32
  stack.dealloc %slot
  return %r
}
"#;

#[test]
fn reads_and_writes_a_global() {
    assert_eq!(run(GLOBALS, "read", &[]), Outcome::Return(Some(7)));
    assert_eq!(run(GLOBALS, "bump", &[]), Outcome::Return(Some(35)));
}

const OVERFLOW: &str = r#"module @Overflow lang "test" target "aarch64-apple-darwin" stage raw

func private @overflow() -> i8 engine(runtime) {
bb0():
  %a = const.int i8, 127
  %b = const.int i8, 1
  %r = add [trap] %a, %b
  return %r
}
"#;

#[test]
fn signed_overflow_traps() {
    assert_eq!(
        run(OVERFLOW, "overflow", &[]),
        Outcome::Trap("KLF-RT-0002".to_owned())
    );
}

const FCMP: &str = r#"module @Fcmp lang "test" target "aarch64-apple-darwin" stage raw

func private @below(%x: f64) -> i64 engine(runtime) {
bb0(%x: f64):
  %one = const.int i64, 1
  %zero = const.int i64, 0
  %t = const.float f64, 2.25
  %c = fcmp olt %x, %t
  cond.br %c, bb1(%one), bb2(%zero)
bb1(%a: i64):
  return %a
bb2(%b: i64):
  return %b
}
"#;

#[test]
fn branches_on_float_comparison() {
    assert_eq!(
        run(FCMP, "below", &[1.5f64.to_bits()]),
        Outcome::Return(Some(1))
    );
    assert_eq!(
        run(FCMP, "below", &[5.0f64.to_bits()]),
        Outcome::Return(Some(0))
    );
}

const WRAP: &str = r#"module @Wrap lang "test" target "aarch64-apple-darwin" stage raw

func private @wrap() -> i8 engine(runtime) {
bb0():
  %a = const.int i8, 127
  %b = const.int i8, 1
  %r = add %a, %b
  return %r
}
"#;

const FACTORIAL: &str = r#"module @Factorial lang "test" target "aarch64-apple-darwin" stage raw

func private @fact(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  %one = const.int i64, 1
  %base = icmp sle %n, %one
  cond.br %base, bb1(%one), bb2(%n)
bb1(%r: i64):
  return %r
bb2(%m: i64):
  %prev = sub %m, %one
  %sub = call @fact(%prev)
  %result = mul %m, %sub
  return %result
}
"#;

#[test]
fn recursive_call_computes_factorial() {
    assert_eq!(run(FACTORIAL, "fact", &[5]), Outcome::Return(Some(120)));
    assert_eq!(
        run(FACTORIAL, "fact", &[10]),
        Outcome::Return(Some(3_628_800))
    );
}

const TRAP_PROPAGATES: &str = r#"module @TrapCall lang "test" target "aarch64-apple-darwin" stage raw

func private @boom(%a: i64) -> i64 engine(runtime) {
bb0(%a: i64):
  %z = const.int i64, 0
  %r = udiv %a, %z
  return %r
}

func private @main() -> i64 engine(runtime) {
bb0():
  %x = const.int i64, 7
  %r = call @boom(%x)
  return %r
}
"#;

#[test]
fn callee_trap_propagates_to_caller() {
    assert_eq!(
        run(TRAP_PROPAGATES, "main", &[]),
        Outcome::Trap("KLF-RT-0010".to_owned())
    );
}

const CONVERT: &str = r#"module @Convert lang "test" target "aarch64-apple-darwin" stage raw

func private @trunc_wide(%v: i64) -> i8 engine(runtime) {
bb0(%v: i64):
  %r = trunc %v to i8
  return %r
}

func private @sext_byte(%v: i8) -> i64 engine(runtime) {
bb0(%v: i8):
  %r = sext %v to i64
  return %r
}

func private @zext_byte(%v: i8) -> i64 engine(runtime) {
bb0(%v: i8):
  %r = zext %v to i64
  return %r
}

func private @to_float(%v: i64) -> f64 engine(runtime) {
bb0(%v: i64):
  %r = sitofp %v to f64
  return %r
}

func private @from_float(%v: f64) -> i64 engine(runtime) {
bb0(%v: f64):
  %r = fptosi %v to i64
  return %r
}
"#;

#[test]
fn truncates_to_narrow_width() {
    assert_eq!(
        run(CONVERT, "trunc_wide", &[300]),
        Outcome::Return(Some(44))
    );
}

#[test]
fn sign_and_zero_extend() {
    assert_eq!(
        run(CONVERT, "sext_byte", &[255]),
        Outcome::Return(Some(u64::MAX))
    );
    assert_eq!(
        run(CONVERT, "zext_byte", &[255]),
        Outcome::Return(Some(255))
    );
}

#[test]
fn integer_float_round_trip() {
    assert_eq!(
        run(CONVERT, "to_float", &[7]),
        Outcome::Return(Some(7.0f64.to_bits()))
    );
    assert_eq!(
        run(CONVERT, "from_float", &[3.9f64.to_bits()]),
        Outcome::Return(Some(3))
    );
}

const SWITCH: &str = r#"module @Switch lang "test" target "aarch64-apple-darwin" stage raw

func private @classify(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  switch.int %n, [0: bb_zero, 1: bb_one, 2: bb_two], default bb_default
bb_zero():
  %a = const.int i64, 100
  return %a
bb_one():
  %b = const.int i64, 200
  return %b
bb_two():
  %c = const.int i64, 300
  return %c
bb_default():
  %d = const.int i64, -1
  return %d
}
"#;

#[test]
fn switch_dispatches_cases_and_default() {
    assert_eq!(run(SWITCH, "classify", &[0]), Outcome::Return(Some(100)));
    assert_eq!(run(SWITCH, "classify", &[1]), Outcome::Return(Some(200)));
    assert_eq!(run(SWITCH, "classify", &[2]), Outcome::Return(Some(300)));
    assert_eq!(
        run(SWITCH, "classify", &[9]),
        Outcome::Return(Some(u64::MAX))
    );
}

const MEMORY: &str = r#"module @Memory lang "test" target "aarch64-apple-darwin" stage raw

func private @stack_roundtrip(%v: i64) -> i64 engine(runtime) {
bb0(%v: i64):
  %slot = stack.alloc i64
  store [trivial] %v to %slot
  %back = load [trivial] %slot
  stack.dealloc %slot
  return %back
}

func private @heap_ok() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted i64
  %addr = ref.addr %r
  %v = const.int i64, 55
  store [trivial] %v to %addr
  %back = load [trivial] %addr
  destroy %r
  return %back
}

func private @retained() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted i64
  %alias = copy %r
  destroy %r
  destroy %alias
  %z = const.int i64, 0
  return %z
}

func private @leak() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted i64
  %v = const.int i64, 1
  return %v
}
"#;

#[test]
fn stack_slot_round_trips() {
    assert_eq!(
        run(MEMORY, "stack_roundtrip", &[7]),
        Outcome::Return(Some(7))
    );
}

#[test]
fn leaked_counted_alloc_fails_heap_balance() {
    assert_eq!(
        run(MEMORY, "leak", &[]),
        Outcome::Trap("KLF-RT-0100".to_owned())
    );
}

#[test]
fn wrapping_add_masks_to_width() {
    match run(WRAP, "wrap", &[]) {
        Outcome::Return(Some(bits)) => assert_eq!(bits & 0xFF, 128),
        other => panic!("unexpected {other:?}"),
    }
}

const EDGES: &str = r#"module @Edges lang "test" target "aarch64-apple-darwin" stage raw

func private @saturate() -> i8 engine(runtime) {
bb0():
  %a = const.int i8, 120
  %b = const.int i8, 50
  %r = add [sat] %a, %b
  return %r
}

func private @unsigned_shift(%v: i32, %n: i32) -> i32 engine(runtime) {
bb0(%v: i32, %n: i32):
  %r = lshr %v, %n
  return %r
}

func private @spin(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  %one = const.int i64, 1
  %next = add %n, %one
  %r = call @spin(%next)
  return %r
}
"#;

#[test]
fn saturating_add_clamps_to_max() {
    assert_eq!(run(EDGES, "saturate", &[]), Outcome::Return(Some(127)));
}

#[test]
fn logical_shift_is_unsigned() {
    assert_eq!(
        run(EDGES, "unsigned_shift", &[0xFFFF_FFFF, 4]),
        Outcome::Return(Some(0x0FFF_FFFF))
    );
}

#[test]
fn unbounded_recursion_traps_on_stack_depth() {
    assert_eq!(
        run(EDGES, "spin", &[0]),
        Outcome::Trap("KLF-RT-0021".to_owned())
    );
}

fn overflowing(operator: &str, sign: &str, bits: u32) -> String {
    let narrow = if bits == 64 {
        "  %zero = const.int i64, 0\n  %x = add [wrap] %a, %zero\n  %y = add [wrap] %b, %zero\n"
            .to_owned()
    } else {
        format!("  %x = trunc %a to i{bits}\n  %y = trunc %b to i{bits}\n")
    };
    let function = |name: &str, out: &str| {
        format!(
            "func private @{name}(%a: i64, %b: i64) -> i64 engine(runtime) {{\n\
             bb0(%a: i64, %b: i64):\n{narrow}  (%r, %f) = {operator}.ovf {sign} %x, %y\n  \
             %out = zext {out} to i64\n  return %out\n}}\n"
        )
    };
    format!(
        "module @Overflowing lang \"test\" target \"x86_64-unknown-linux-gnu\" stage raw\n\n{}\n{}",
        function("flag", "%f"),
        function("value", "%r")
    )
}

macro_rules! expected {
    ($signed:ty, $unsigned:ty, $operator:expr, $sign:expr, $a:expr, $b:expr) => {{
        let narrow = |value: i64| {
            let bytes = value.to_le_bytes()[..size_of::<$signed>()].try_into();
            <$signed>::from_le_bytes(bytes.expect("width fits"))
        };
        let (a, b) = (narrow($a), narrow($b));
        let (value, flag) = match ($operator, $sign) {
            ("add", "s") => a.overflowing_add(b),
            ("sub", "s") => a.overflowing_sub(b),
            ("mul", "s") => a.overflowing_mul(b),
            (operator, _) => {
                let (a, b) = (a.cast_unsigned(), b.cast_unsigned());
                let (value, flag) = match operator {
                    "add" => a.overflowing_add(b),
                    "sub" => a.overflowing_sub(b),
                    _ => a.overflowing_mul(b),
                };
                (value.cast_signed(), flag)
            }
        };
        (u64::from(value.cast_unsigned()), flag)
    }};
}

#[test]
fn overflowing_operations_report_overflow() {
    let samples: [i64; 9] = [0, 1, -1, 2, -2, 3, 100, i64::MAX, i64::MIN];
    let returned = |source: &str, entry: &str, arguments: &[u64]| {
        let Outcome::Return(Some(value)) = run(source, entry, arguments) else {
            panic!("{entry} did not return");
        };
        value
    };
    for operator in ["add", "sub", "mul"] {
        for sign in ["s", "u"] {
            for bits in [64, 8] {
                let source = overflowing(operator, sign, bits);
                for (&a, &b) in samples
                    .iter()
                    .flat_map(|a| samples.iter().map(move |b| (a, b)))
                {
                    let arguments = [a.cast_unsigned(), b.cast_unsigned()];
                    let got = (
                        returned(&source, "value", &arguments),
                        returned(&source, "flag", &arguments) == 1,
                    );
                    let expected = if bits == 64 {
                        expected!(i64, u64, operator, sign, a, b)
                    } else {
                        expected!(i8, u8, operator, sign, a, b)
                    };
                    assert_eq!(got, expected, "i{bits} {operator} {sign} {a} {b}");
                }
            }
        }
    }
}
