//! Aggregate values (section 12.6) for the parity suites: every aggregate
//! instruction on structs, tuples, arrays and enums held as SSA values, a
//! struct passed to and returned from a function, and a struct carried
//! around a loop. The expected results are worked out by hand, so the VM is
//! checked as well as compared.

pub const AGGREGATES: &str = r#"module @Aggregates lang "test" target "aarch64-apple-darwin" stage raw

type %Pair = struct ridl { a: i64, b: i64 }
policy { copy: none, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none }

type %Shape = enum ridl { Empty, Square(i64), Rect(i64, i64) }
policy { copy: none, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none }

func module @pairs(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %k = const.int i64, 3
  %p = struct.new %Pair(%x, %k)
  %a = struct.extract %p, 0
  %b = struct.extract %p, 1
  %t = const.int i64, 100
  %m = mul [wrap] %a, %t
  %r = add [wrap] %m, %b
  return %r
}

func module @scale(%v: borrow %Pair, %by: i64) -> %Pair engine(runtime) {
bb0(%v: borrow %Pair, %by: i64):
  %a = struct.extract %v, 0
  %b = struct.extract %v, 1
  %sa = mul [wrap] %a, %by
  %sb = mul [wrap] %b, %by
  %r = struct.new %Pair(%sa, %sb)
  return %r
}

func module @scaled(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %one = const.int i64, 1
  %y = add [wrap] %x, %one
  %p = struct.new %Pair(%x, %y)
  %three = const.int i64, 3
  %q = call @scale(%p, %three)
  (%qa, %qb) = struct.destructure %q
  %t = const.int i64, 1000
  %m = mul [wrap] %qa, %t
  %r = add [wrap] %m, %qb
  return %r
}

func module @tuples(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %two = const.int i64, 2
  %y = mul [wrap] %x, %two
  %seven = const.int i64, 7
  %t = tuple.new(%x, %y, %seven)
  %last = tuple.extract %t, 2
  (%a, %b, %c) = tuple.destructure %t
  %ab = sub [wrap] %b, %a
  %abc = mul [wrap] %ab, %c
  %r = add [wrap] %abc, %last
  return %r
}

func module @arrays(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %one = const.int i64, 1
  %y = add [wrap] %x, %one
  %five = const.int i64, 5
  %arr = array.new i64(%x, %y, %five)
  %slot = stack.alloc [3 x i64]
  store [init] %arr to %slot
  %i = const.int i64, 2
  %e2 = elem.addr %slot, %i
  %v2 = load [trivial] %e2
  %j = const.int i64, 1
  %e1 = elem.addr %slot, %j
  %v1 = load [trivial] %e1
  %t = const.int i64, 10
  %m = mul [wrap] %v1, %t
  %r = add [wrap] %m, %v2
  return %r
}

func module @shapes(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %zero = const.int i64, 0
  %is_zero = icmp eq %x, %zero
  cond.br %is_zero, bb_square(), bb_rect()
bb_square():
  %s = enum.new %Shape, 1(%x)
  br bb_area(%s)
bb_rect():
  %four = const.int i64, 4
  %r = enum.new %Shape, 2(%x, %four)
  br bb_area(%r)
bb_area(%shape: %Shape):
  switch.enum %shape, [0: bb_empty(), 1: bb_one(), 2: bb_two()]
bb_empty():
  return %zero
bb_one():
  %side = enum.take %shape, 1
  %sq = mul [wrap] %side, %side
  return %sq
bb_two():
  (%w, %h) = enum.take %shape, 2
  %area = mul [wrap] %w, %h
  return %area
}

func module @fibonacci(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  %zero = const.int i64, 0
  %one = const.int i64, 1
  %start = struct.new %Pair(%zero, %one)
  br bb_loop(%start, %zero)
bb_loop(%p: %Pair, %i: i64):
  %more = icmp ult %i, %n
  cond.br %more, bb_step(), bb_done()
bb_step():
  %a = struct.extract %p, 0
  %b = struct.extract %p, 1
  %s = add [wrap] %a, %b
  %next = struct.new %Pair(%b, %s)
  %i2 = add [wrap] %i, %one
  br bb_loop(%next, %i2)
bb_done():
  %r = struct.extract %p, 0
  return %r
}

func module @stored(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %nine = const.int i64, 9
  %p = struct.new %Pair(%nine, %x)
  %slot = stack.alloc %Pair
  store [init] %p to %slot
  %q = load [trivial] %slot
  %c = copy %q
  %ca = struct.extract %c, 0
  %qb = struct.extract %q, 1
  %r = sub [wrap] %ca, %qb
  return %r
}
"#;
