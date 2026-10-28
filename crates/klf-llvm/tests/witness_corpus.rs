//! Witness calls on a concrete type (section 12.11) for the parity suites,
//! unoptimized, so the static resolution every backend gets is what runs.
//! The expected results are worked out by hand.

pub const WITNESS: &str = r#"module @Witness lang "test" target "aarch64-apple-darwin" stage raw

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
