//! Block addresses and indirect branches (section 12.13) for the parity
//! suites: a block address picked by a value, carried through a block
//! parameter and branched to. The expected results are worked out by hand,
//! so the VM is checked as well as compared.

pub const INDIRECT: &str = r#"module @Indirect lang "test" target "aarch64-apple-darwin" stage raw

func module @dispatch(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %zero = const.int i64, 0
  %one = const.int i64, 1
  %small = icmp eq %x, %zero
  %middle = icmp eq %x, %one
  %to_small = block.addr bb2
  %to_middle = block.addr bb3
  %to_large = block.addr bb4
  %picked = select %middle, %to_middle, %to_large
  %target = select %small, %to_small, %picked
  br bb1(%target)
bb1(%address: ptr):
  br.indirect %address, [bb2, bb3, bb4, bb3]
bb2():
  %a = const.int i64, 10
  return %a
bb3():
  %b = const.int i64, 20
  return %b
bb4():
  %c = const.int i64, 30
  %r = add [wrap] %c, %x
  return %r
}
"#;
