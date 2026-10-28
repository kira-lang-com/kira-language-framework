//! Unwinding with no handler (section 12.14) for the parity suites: a
//! callee that throws under one argument, reached through `invoke` with a
//! cleanup pad that resumes. Nothing can catch, so the throw ends the run
//! with `KLF-RT-0020` and the pad never runs; the other argument returns
//! through the normal edge. The expected results are worked out by hand.

pub const UNWIND: &str = r#"module @Unwind lang "test" target "aarch64-apple-darwin" stage raw

func module @thrower(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %zero = const.int i64, 0
  %calm = icmp eq %x, %zero
  cond.br %calm, bb1(), bb2()
bb1():
  %five = const.int i64, 5
  return %five
bb2():
  %box = inttoptr %x to ptr
  throw %box
}

func module @guarded(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  invoke @thrower(%x) to bb1() unwind bb2()
bb1():
  %seven = const.int i64, 7
  return %seven
bb2():
  (%exception, %selector) = landingpad cleanup
  resume %exception
}
"#;
