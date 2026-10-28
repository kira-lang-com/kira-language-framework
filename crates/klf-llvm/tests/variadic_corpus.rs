//! RIDL variadics (sections 12.14 and 29.5) for the parity suites: a
//! variadic function walks its arguments with `va.arg`, copies its list
//! with `va.copy`, and reads 32-bit, 64-bit and floating-point slots. The
//! expected results are worked out by hand, so the VM is checked as well as
//! compared.

pub const VARIADIC: &str = r#"module @Variadic lang "test" target "aarch64-apple-darwin" stage raw

func private @sum(%count: i64) -> i64 engine(runtime) {
bb0(%count: i64):
  %list = stack.alloc ptr
  va.start %list
  %zero = const.int i64, 0
  br bb1(%zero, %zero)
bb1(%i: i64, %acc: i64):
  %done = icmp eq %i, %count
  cond.br %done, bb3(), bb2()
bb2():
  %v = va.arg i64, %list
  %next_acc = add [wrap] %acc, %v
  %one = const.int i64, 1
  %next_i = add [wrap] %i, %one
  br bb1(%next_i, %next_acc)
bb3():
  va.end %list
  return %acc
}

func private @mixed(%scale: i64) -> i64 engine(runtime) {
bb0(%scale: i64):
  %list = stack.alloc ptr
  va.start %list
  %small = va.arg i32, %list
  %saved = stack.alloc ptr
  va.copy %saved, %list
  %real = va.arg f64, %list
  %again = va.arg f64, %saved
  va.end %saved
  va.end %list
  %wide = sext %small to i64
  %whole = fptosi %real to i64
  %twice = fptosi %again to i64
  %scaled = mul [wrap] %wide, %scale
  %partial = add [wrap] %scaled, %whole
  %r = add [wrap] %partial, %twice
  return %r
}

func module @summed(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %three = const.int i64, 3
  %ten = const.int i64, 10
  %r = call.variadic @sum(%three; %x, %ten, %x)
  return %r
}

func module @mixes(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %seven = const.int i32, 7
  %real = const.float f64, 2.75
  %r = call.variadic @mixed(%x; %seven, %real)
  return %r
}
"#;

/// A C import's variadic tail follows the platform (section 29.5): libc's
/// `snprintf` reads an `int`, a `long long` and a `double` from it and
/// answers how long its output would be.
pub const FOREIGN_VARIADIC: &str = r#"module @ForeignVariadic lang "test" target "aarch64-apple-darwin" stage raw

func private foreign @snprintf(%buffer: ptr, %size: i64, %format: ptr) -> i32 engine(native)

func private @formatted(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %buffer = const.null
  %size = const.int i64, 0
  (%format, %length) = const.string "%d|%lld|%.2f\0"
  %seven = const.int i32, 7
  %half = const.float f64, 2.5
  %n = call.variadic @snprintf(%buffer, %size, %format; %seven, %x, %half)
  %r = sext %n to i64
  return %r
}
"#;
