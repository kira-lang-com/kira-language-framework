//! Vector values (section 12.5) for the parity suites: lanes built, picked,
//! shuffled, combined elementwise, compared, converted, stored, loaded and
//! passed to and returned from a private function. The expected results
//! are worked out by hand, so the VM is checked as well as compared.

pub const VECTORS: &str = r#"module @Vectors lang "test" target "aarch64-apple-darwin" stage raw

func private @scale(%v: vec<4 x i32>, %k: i32) -> vec<4 x i32> engine(runtime) {
bb0(%v: vec<4 x i32>, %k: i32):
  %s = vec.splat vec<4 x i32>, %k
  %m = mul [wrap] %v, %s
  return %m
}

func module @lanes(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %n = trunc %x to i32
  %zero = const.zero vec<4 x i32>
  %one = const.int i32, 1
  %two = const.int i32, 2
  %three = const.int i32, 3
  %a = vec.insert %zero, 0, %n
  %b = vec.insert %a, 1, %one
  %c = vec.insert %b, 2, %two
  %d = vec.insert %c, 3, %three
  %e = call @scale(%d, %two)
  %f = vec.shuffle %e, %d, [3, 2, 4, 5]
  %g = add [wrap] %e, %f
  br bb1(%g)
bb1(%h: vec<4 x i32>):
  %slot = stack.alloc vec<4 x i32>
  store [trivial] %h to %slot
  %back = load [trivial] %slot
  %l0 = vec.extract %back, 0
  %l2 = vec.extract %back, 2
  %w0 = sext %l0 to i64
  %w2 = sext %l2 to i64
  %sum = add [wrap] %w0, %w2
  return %sum
}

func module @clamped(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %n = trunc %x to i32
  %five = const.int i32, 5
  %ten = const.int i32, 10
  %start = vec.splat vec<2 x i32>, %n
  %v = vec.insert %start, 1, %five
  %limit = vec.splat vec<2 x i32>, %ten
  %below = icmp slt %v, %limit
  %kept = select %below, %v, %limit
  %wide = sext %kept to vec<2 x i64>
  %first = vec.extract %wide, 0
  %second = vec.extract %wide, 1
  %r = add [wrap] %first, %second
  return %r
}
"#;
