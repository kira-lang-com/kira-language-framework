//! `select`, `fma`, the raw pointer instructions, the layout queries,
//! overflow-reporting arithmetic and `call.foreign` for the parity suites. The expected results are worked out by hand, so the VM
//! is checked as well as compared.

pub const UNMANAGED: &str = r#"module @Unmanaged lang "test" target "aarch64-apple-darwin" stage raw

type %Wide = struct ridl { a: i64, b: i8 }
policy { copy: trivial, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none }

func module @picks(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %ten = const.int i64, 10
  %small = icmp slt %x, %ten
  %two = const.int i64, 2
  %doubled = mul [wrap] %x, %two
  %hundred = const.int i64, 100
  %raised = add [wrap] %x, %hundred
  %r = select %small, %doubled, %raised
  return %r
}

func module @fused(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %f = sitofp %x to f64
  %half = const.float f64, 1.5
  %quarter = const.float f64, 2.25
  %m = fma %f, %half, %quarter
  %four = const.float f64, 4.0
  %scaled = fmul %m, %four
  %r = fptosi %scaled to i64
  return %r
}

func module @raw(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %size = const.int i64, 16
  %align = const.int i64, 8
  %p = heap.alloc.raw %size, %align
  raw.store %x, %p
  %eight = const.int i64, 8
  %q = ptr.offset %p, %eight
  %seven = const.int i64, 7
  raw.store %seven, %q
  %a = raw.load i64, %p
  %b = raw.load i64, %q
  %d = ptr.diff %q, %p
  heap.free.raw %p
  %hundred = const.int i64, 100
  %ten = const.int i64, 10
  %ah = mul [wrap] %a, %hundred
  %bt = mul [wrap] %b, %ten
  %s = add [wrap] %ah, %bt
  %r = add [wrap] %s, %d
  return %r
}

func module @sizes(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %size = type.size %Wide
  %align = type.align i32
  %stride = type.stride (i32, i8)
  %hundred = const.int i64, 100
  %tenk = const.int i64, 10000
  %a = mul [wrap] %align, %hundred
  %s = mul [wrap] %stride, %tenk
  %sa = add [wrap] %size, %a
  %r = add [wrap] %sa, %s
  return %r
}

func module @borrowed(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %slot = stack.alloc i64
  store [trivial] %x to %slot
  %v = load.borrow %slot
  %p = addr.toptr %slot
  %w = raw.load i64, %p
  %r = add [wrap] %v, %w
  return %r
}

func module @unowned(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %cell = heap.alloc.counted i64
  %addr = ref.addr %cell
  store [trivial] %x to %addr
  %p = ref.unowned %cell
  %v = raw.load i64, %p
  destroy %cell
  %one = const.int i64, 1
  %r = add [wrap] %v, %one
  return %r
}

func private foreign @labs(%x: i64) -> i64 engine(native)

func module @absolute(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %zero = const.int i64, 0
  %n = sub [wrap] %zero, %x
  %r = call.foreign @labs(%n)
  return %r
}

func module @checked(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %zero = const.int i64, 0
  %big = const.int i64, 9223372036854775807
  (%s, %o) = add.ovf s %x, %big
  (%m, %p) = mul.ovf u %x, %x
  (%d, %u) = sub.ovf u %zero, %x
  %thousand = const.int i64, 1000
  %lakh = const.int i64, 100000
  %so = select %o, %thousand, %zero
  %su = select %u, %lakh, %zero
  %t = add [wrap] %so, %su
  %r = add [wrap] %t, %m
  return %r
}
"#;
