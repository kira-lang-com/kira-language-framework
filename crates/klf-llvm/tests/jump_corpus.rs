//! `setjmp` and `longjmp` (section 12.14) for the parity suites: a
//! function saves its place, calls down two frames, and the deepest one
//! jumps back with its argument. A value made before the `setjmp` and
//! needed only after the jump must still be there, though the block that
//! uses it comes first and the path to the jump no longer needs it. The expected results are
//! worked out by hand.

pub const JUMPS: &str = r#"module @Jumps lang "test" target "aarch64-apple-darwin" stage raw

func module @deepest(%buffer: ptr, %n: i64) -> i64 engine(runtime) {
bb0(%buffer: ptr, %n: i64):
  %value = trunc %n to i32
  longjmp %buffer, %value
  unreachable
}

func module @middle(%buffer: ptr, %n: i64) -> i64 engine(runtime) {
bb0(%buffer: ptr, %n: i64):
  %one = const.int i64, 1
  %more = add [wrap] %n, %one
  %r = call @deepest(%buffer, %more)
  return %r
}

func module @caught(%n: i64) -> i64 engine(runtime) returns_twice {
bb0(%n: i64):
  %slot = stack.alloc [64 x i64]
  %buffer = addr.toptr %slot
  %base = const.int i64, 100
  %r = setjmp %buffer
  %zero = const.int i32, 0
  %first = icmp eq %r, %zero
  cond.br %first, bb2(), bb1()
bb1():
  %wide = sext %r to i64
  %sum = add [wrap] %wide, %base
  return %sum
bb2():
  %seven = const.int i64, 7
  %scratch = mul [wrap] %n, %seven
  %r2 = call @middle(%buffer, %scratch)
  return %r2
}
"#;
