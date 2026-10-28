//! The shared backend-parity corpus: hand-written `.kir` programs exercised on
//! the VM and on every native backend. Both `klf-llvm` and `klf-cranelift` run
//! this same set (the latter includes this file by path).

pub const ARITHMETIC: &str = r#"module @Arith lang "test" target "aarch64-apple-darwin" stage raw

func private @add(%lhs: i64, %rhs: i64) -> i64 engine(runtime) {
bb0(%lhs: i64, %rhs: i64):
  %sum = add %lhs, %rhs
  return %sum
}
"#;

pub const MAX: &str = r#"module @Max lang "test" target "aarch64-apple-darwin" stage raw

func private @max(%a: i64, %b: i64) -> i64 engine(runtime) {
bb0(%a: i64, %b: i64):
  %gt = icmp sgt %a, %b
  cond.br %gt, bb1(%a), bb2(%b)
bb1(%left: i64):
  return %left
bb2(%right: i64):
  return %right
}
"#;

pub const SUM: &str = r#"module @Sum lang "test" target "aarch64-apple-darwin" stage raw

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

pub const DIV_ZERO: &str = r#"module @DivZero lang "test" target "aarch64-apple-darwin" stage raw

func private @divzero(%a: i64) -> i64 engine(runtime) {
bb0(%a: i64):
  %z = const.int i64, 0
  %r = sdiv %a, %z
  return %r
}
"#;

pub const OVERFLOW: &str = r#"module @Overflow lang "test" target "aarch64-apple-darwin" stage raw

func private @overflow() -> i8 engine(runtime) {
bb0():
  %a = const.int i8, 127
  %b = const.int i8, 1
  %r = add [trap] %a, %b
  return %r
}
"#;

pub const FLOAT: &str = r#"module @Float lang "test" target "aarch64-apple-darwin" stage raw

func private @fadd() -> f64 engine(runtime) {
bb0():
  %a = const.float f64, 1.5
  %b = const.float f64, 2.25
  %r = fadd %a, %b
  return %r
}
"#;

pub const FCMP: &str = r#"module @Fcmp lang "test" target "aarch64-apple-darwin" stage raw

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

pub const WRAP: &str = r#"module @Wrap lang "test" target "aarch64-apple-darwin" stage raw

func private @wrap() -> i8 engine(runtime) {
bb0():
  %a = const.int i8, 127
  %b = const.int i8, 1
  %r = add %a, %b
  return %r
}
"#;

pub const CONVERT: &str = r#"module @Convert lang "test" target "aarch64-apple-darwin" stage raw

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

pub const SWITCH: &str = r#"module @Switch lang "test" target "aarch64-apple-darwin" stage raw

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

pub const MEMORY: &str = r#"module @Memory lang "test" target "aarch64-apple-darwin" stage raw

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

pub const EDGES: &str = r#"module @Edges lang "test" target "aarch64-apple-darwin" stage raw

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

pub const FAILING: &str = r#"module @Failing lang "test" target "aarch64-apple-darwin" stage raw

func private @might_fail(%x: i64) -> i64 fails i64 engine(runtime) {
bb0(%x: i64):
  %z = const.int i64, 0
  %bad = icmp eq %x, %z
  cond.br %bad, bb_fail(), bb_ok(%x)
bb_fail():
  %e = const.int i64, 99
  fail %e
bb_ok(%v: i64):
  return %v
}

func private @caller(%x: i64) -> i64 fails i64 engine(runtime) {
bb0(%x: i64):
  %r = call @might_fail(%x)
  return %r
}
"#;

pub const STRING_CONSTANT: &str = r#"module @StringConstant lang "test" target "aarch64-apple-darwin" stage raw

func private @length() -> i64 engine(runtime) {
bb0:
  (%ptr, %length) = const.string "KLF"
  return %length
}
"#;

pub const GLOBALS: &str = r#"module @Globals lang "test" target "aarch64-apple-darwin" stage raw

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

pub const INDIRECT: &str = r#"module @Indirect lang "test" target "aarch64-apple-darwin" stage raw

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

/// A RIDL closure record `{code, ctx}` called directly and through the
/// platform callback thunk `closure.callback` makes for it (sections 21.8,
/// 26.4). `run(2)` adds the captured 40 twice: 42, then 82.
pub const CLOSURES: &str = r#"module @Closures lang "test" target "aarch64-apple-darwin" stage raw

func private @add_captured(%ctx: context ptr, %x: i64) -> i64 engine(runtime) abi(ridl) {
bb0(%ctx: context ptr, %x: i64):
  %slot = bitcast %ctx to *i64
  %captured = load [trivial] %slot
  %r = add %captured, %x
  return %r
}

func private @each(%callback: fn(i64, ptr) -> i64, %user: ptr, %x: i64) -> i64 engine(runtime) {
bb0(%callback: fn(i64, ptr) -> i64, %user: ptr, %x: i64):
  %r = call.indirect %callback(%x, %user) : (i64, ptr) -> i64
  return %r
}

func module @run(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %captured = stack.alloc i64
  %forty = const.int i64, 40
  store [trivial] %forty to %captured
  %record = stack.alloc [2 x ptr]
  %code = func.ref @add_captured
  %code_word = bitcast %code to ptr
  %zero = const.int i64, 0
  %code_slot = elem.addr %record, %zero
  store [trivial] %code_word to %code_slot
  %context = bitcast %captured to ptr
  %one = const.int i64, 1
  %context_slot = elem.addr %record, %one
  store [trivial] %context to %context_slot
  %direct = call.closure %record(%x) : (i64) -> i64
  %callback = closure.callback 1, fn(i64, ptr) -> i64
  %user = bitcast %record to ptr
  %via = call @each(%callback, %user, %direct)
  stack.dealloc %record
  stack.dealloc %captured
  return %via
}
"#;

/// `call.indirect` through a failing RIDL function pointer propagates the
/// callee's failure like a direct call does.
pub const FAILING_INDIRECT: &str = r#"module @FailingIndirect lang "test" target "aarch64-apple-darwin" stage raw

func private @might_fail(%x: i64) -> i64 fails i64 engine(runtime) {
bb0(%x: i64):
  %z = const.int i64, 0
  %bad = icmp eq %x, %z
  cond.br %bad, bb_fail(), bb_ok(%x)
bb_fail():
  %e = const.int i64, 99
  fail %e
bb_ok(%v: i64):
  return %v
}

func private @caller(%x: i64) -> i64 fails i64 engine(runtime) {
bb0(%x: i64):
  %f = func.ref @might_fail
  %r = call.indirect %f(%x) : (i64) -> i64
  return %r
}
"#;

/// A RIDL function taking a struct by address and filling an out-pointer
/// converts to the platform convention through a thunk.
pub const AGGREGATE_ADAPTER: &str = r#"module @AggregateAdapter lang "test" target "aarch64-apple-darwin" stage raw

type %Point = struct ridl { x: i64, y: i64 }
policy { copy: trivial, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none }

func private @swap(%out: *%Point, %p: *%Point) -> unit engine(runtime) abi(ridl) {
bb0(%out: *%Point, %p: *%Point):
  %px = field.addr %p, 0
  %py = field.addr %p, 1
  %x = load [trivial] %px
  %y = load [trivial] %py
  %ox = field.addr %out, 0
  %oy = field.addr %out, 1
  store [init] %y to %ox
  store [init] %x to %oy
  return
}

func module @run(%a: i64, %b: i64) -> i64 engine(runtime) {
bb0(%a: i64, %b: i64):
  %p = stack.alloc %Point
  %px = field.addr %p, 0
  %py = field.addr %p, 1
  store [init] %a to %px
  store [init] %b to %py
  %out = stack.alloc %Point
  %f = func.ref @swap
  %platform = bitcast %f to fn(*%Point, *%Point) -> unit
  call.indirect %platform(%out, %p) : (*%Point, *%Point) -> unit
  %ox = field.addr %out, 0
  %r = load [trivial] %ox
  stack.dealloc %out
  stack.dealloc %p
  return %r
}
"#;

/// An `initializer` function sets a global before the entry reads it.
pub const INITIALIZER: &str = r#"module @Initializer lang "test" target "aarch64-apple-darwin" stage raw

global @counter : i32 = 7

func private @setup() -> unit engine(runtime) initializer {
bb0:
  %p = global.addr @counter
  %n = const.int i32, 40
  store [assign] %n to %p
  return
}

func private @read() -> i32 engine(runtime) {
bb0:
  %p = global.addr @counter
  %v = load [trivial] %p
  %two = const.int i32, 2
  %r = add %v, %two
  return %r
}
"#;

/// Dynamic exclusivity (section 11.8). Two overlapping reads are fine; a modify
/// against either of them is not, and traps with `KLF-RT-0003`. `static` is
/// proved by the verifier and asks the machine for nothing.
pub const ACCESS: &str = r#"module @Access lang "test" target "x86_64-unknown-linux-gnu" stage raw

func private @two_reads() -> i64 engine(runtime) {
bb0:
  %slot = stack.alloc i64
  %seven = const.int i64, 7
  access.begin read dynamic %slot
  access.begin read dynamic %slot
  store [init] %seven to %slot
  access.end %slot
  access.end %slot
  %back = load [trivial] %slot
  stack.dealloc %slot
  return %back
}

func private @read_then_modify() -> i64 engine(runtime) {
bb0:
  %slot = stack.alloc i64
  access.begin read dynamic %slot
  access.begin modify dynamic %slot
  access.end %slot
  access.end %slot
  stack.dealloc %slot
  %z = const.int i64, 0
  return %z
}

func private @static_access_is_free() -> i64 engine(runtime) {
bb0:
  %slot = stack.alloc i64
  access.begin modify static %slot
  access.begin modify static %slot
  access.end %slot
  access.end %slot
  stack.dealloc %slot
  %z = const.int i64, 0
  return %z
}
"#;

/// Ownership on storage rather than in registers (section 11.6 pass 5,
/// section 12.8). `copy.addr` leaves a second owner so it retains,
/// `move.addr` relocates without retaining, and `destroy.addr` releases in
/// place; `assign` destroys what the destination held first. A count that does
/// not balance is reported by the runtime's exit check, so each of these also
/// proves the retain and release ran exactly once.
pub const OWNERSHIP: &str = r#"module @Ownership lang "test" target "aarch64-unknown-linux-gnu" stage raw

type %Cell = struct ridl { n: i64 }
policy { copy: none, destroy: trivial, relocate: bitwise, exclusivity: static, pairing: counted }

func private @copy_addr_retains() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted %Cell
  %a = ref.addr %r
  %field = field.addr %a, 0
  %seven = const.int i64, 7
  store [trivial] %seven to %field
  %b = stack.alloc %Cell
  copy.addr %a to %b
  %back = load [trivial] %field
  destroy.addr %a
  destroy.addr %b
  stack.dealloc %b
  return %back
}

func private @copy_addr_assign_destroys_first() -> i64 engine(runtime) {
bb0():
  %one = heap.alloc.counted %Cell
  %a = ref.addr %one
  %nine = heap.alloc.counted %Cell
  %b = ref.addr %nine
  %field = field.addr %b, 0
  %n = const.int i64, 9
  store [trivial] %n to %field
  copy.addr %a to [assign] %b
  %back = load [trivial] %field
  destroy.addr %b
  return %back
}

func private @move_addr_does_not_retain() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted %Cell
  %a = ref.addr %r
  %field = field.addr %a, 0
  %n = const.int i64, 9
  store [trivial] %n to %field
  %b = stack.alloc %Cell
  %other = const.int i64, 0
  store [trivial] %other to %b
  move.addr %b to [assign] %a
  %back = load [trivial] %field
  destroy.addr %a
  stack.dealloc %b
  return %back
}
"#;

/// `try.call` (section 12.11): the callee's error is split onto the error
/// block rather than propagated out of the caller. `relay` reaches `checked`
/// through a plain call, so the failure crosses one frame before it is caught.
pub const TRY_CALL: &str = r#"module @TryCall lang "test" target "aarch64-unknown-linux-gnu" stage raw

func private @checked(%x: i64) -> i64 fails i64 engine(runtime) {
bb0(%x: i64):
  %z = const.int i64, 0
  %bad = icmp eq %x, %z
  cond.br %bad, bb_fail(), bb_ok(%x)
bb_fail():
  %e = const.int i64, 99
  fail %e
bb_ok(%v: i64):
  return %v
}

func private @forward(%w: i64) -> i64 fails i64 engine(runtime) {
bb0(%w: i64):
  %r = call @checked(%w)
  %one = const.int i64, 1
  %s = add %r, %one
  return %s
}

func private @caller(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  try.call @checked(%x) normal bb_ok() error bb_err()
bb_ok(%out: i64):
  return %out
bb_err(%err: i64):
  %zero = const.int i64, 0
  %negated = sub %zero, %err
  return %negated
}

func private @relay(%y: i64) -> i64 engine(runtime) {
bb0(%y: i64):
  %bias = const.int i64, 1000
  try.call @forward(%y) normal bb_ok(%bias) error bb_err(%bias)
bb_ok(%out: i64, %kept: i64):
  %sum = add %out, %kept
  return %sum
bb_err(%err: i64, %held: i64):
  %diff = sub %held, %err
  return %diff
}
"#;

/// The atomics and `fence` (section 12.14): every `atomic.rmw` operator on an
/// `i64` and an `i32`, a compare-exchange that succeeds and one that fails,
/// each folded into the result so a wrong value read or written shows.
pub const ATOMICS: &str = r#"module @Atomics lang "test" target "aarch64-apple-darwin" stage raw

func private @fold(%acc: i64, %v: i64) -> i64 engine(runtime) {
bb0(%acc: i64, %v: i64):
  %k = const.int i64, 31
  %m = mul [wrap] %acc, %k
  %r = add [wrap] %m, %v
  return %r
}

func private @wide(%seed: i64) -> i64 engine(runtime) {
bb0(%seed: i64):
  %p = stack.alloc i64
  atomic.store %seed, %p seq_cst
  %three = const.int i64, 3
  %minus = const.int i64, -5
  %mask = const.int i64, 12
  %a0 = atomic.rmw add %p, %three seq_cst
  %a1 = atomic.rmw sub %p, %three acq_rel
  %a2 = atomic.rmw or %p, %mask relaxed
  %a3 = atomic.rmw and %p, %mask acquire
  %a4 = atomic.rmw xor %p, %three release
  %a5 = atomic.rmw nand %p, %mask seq_cst
  %a6 = atomic.rmw max %p, %minus seq_cst
  %a7 = atomic.rmw min %p, %minus seq_cst
  %a8 = atomic.rmw umax %p, %three seq_cst
  %a9 = atomic.rmw umin %p, %three seq_cst
  %a10 = atomic.rmw xchg %p, %seed seq_cst
  fence seq_cst
  %last = atomic.load i64, %p acquire
  %f0 = call @fold(%a0, %a1)
  %f1 = call @fold(%f0, %a2)
  %f2 = call @fold(%f1, %a3)
  %f3 = call @fold(%f2, %a4)
  %f4 = call @fold(%f3, %a5)
  %f5 = call @fold(%f4, %a6)
  %f6 = call @fold(%f5, %a7)
  %f7 = call @fold(%f6, %a8)
  %f8 = call @fold(%f7, %a9)
  %f9 = call @fold(%f8, %a10)
  %r = call @fold(%f9, %last)
  return %r
}

func private @narrow(%seed: i64) -> i64 engine(runtime) {
bb0(%seed: i64):
  %p = stack.alloc i32
  %s = trunc %seed to i32
  atomic.store %s, %p release
  %big = const.int i32, 2147483647
  %one = const.int i32, 1
  %old = atomic.rmw add %p, %big seq_cst
  %wrapped = atomic.rmw umin %p, %one seq_cst
  %signed = atomic.rmw max %p, %one seq_cst
  fence acq_rel
  %now = atomic.load i32, %p seq_cst
  %w0 = sext %old to i64
  %w1 = sext %wrapped to i64
  %w2 = sext %signed to i64
  %w3 = sext %now to i64
  %f0 = call @fold(%w0, %w1)
  %f1 = call @fold(%f0, %w2)
  %r = call @fold(%f1, %w3)
  return %r
}

func private @exchange(%seed: i64) -> i64 engine(runtime) {
bb0(%seed: i64):
  %p = stack.alloc i64
  atomic.store %seed, %p seq_cst
  %wrong = const.int i64, 99
  %new = const.int i64, 7
  (%miss, %missed) = atomic.cmpxchg %p, %wrong, %new seq_cst seq_cst
  (%hit, %done) = atomic.cmpxchg [weak] %p, %seed, %new acq_rel acquire
  %after = atomic.load i64, %p seq_cst
  %b0 = zext %missed to i64
  %b1 = zext %done to i64
  %f0 = call @fold(%miss, %b0)
  %f1 = call @fold(%f0, %hit)
  %f2 = call @fold(%f1, %b1)
  %r = call @fold(%f2, %after)
  return %r
}
"#;
