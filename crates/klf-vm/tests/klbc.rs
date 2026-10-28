use klf_kir::parse_module;
use klf_vm::{Outcome, decode, encode, lower_module, run};

const MODULE: &str = r#"module @Roundtrip lang "test" target "aarch64-apple-darwin" stage raw

func private @add(%a: i64, %b: i64) -> i64 engine(runtime) {
bb0(%a: i64, %b: i64):
  %s = add %a, %b
  return %s
}

func private @main() -> i64 engine(runtime) {
bb0():
  %slot = stack.alloc i64
  %x = const.int i64, 40
  store [trivial] %x to %slot
  %y = load [trivial] %slot
  %two = const.int i64, 2
  %sum = call @add(%y, %two)
  stack.dealloc %slot
  return %sum
}
"#;

const BRANCH_MODULE: &str = r#"module @BranchRoundtrip lang "test" target "aarch64-apple-darwin" stage raw

func private @main(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %zero = const.int i64, 0
  %ten = const.int i64, 10
  %twenty = const.int i64, 20
  %is_zero = icmp eq %x, %zero
  cond.br %is_zero, bb1(%ten), bb2(%twenty)
bb1(%result: i64):
  return %result
bb2(%result: i64):
  return %result
}
"#;

#[test]
fn klbc_round_trips_and_runs() {
    let module = parse_module(MODULE).unwrap();
    let program = lower_module(&module).unwrap();

    let image = encode(&program);
    assert_eq!(&image[0..4], b"KLBC");

    let decoded = decode(&image).unwrap();
    assert_eq!(program, decoded);

    let index = decoded.function_index("main").unwrap();
    assert_eq!(run(&decoded, index, &[]), Outcome::Return(Some(42)));
}

#[test]
fn klbc_round_trips_branch_sites_and_executes() {
    let module = parse_module(BRANCH_MODULE).unwrap();
    let program = lower_module(&module).unwrap();

    let image = encode(&program);
    assert_eq!(u16::from_le_bytes([image[6], image[7]]), 1);
    let decoded = decode(&image).unwrap();
    assert_eq!(program, decoded);

    let index = decoded.function_index("main").unwrap();
    assert_eq!(run(&decoded, index, &[0]), Outcome::Return(Some(10)));
    assert_eq!(run(&decoded, index, &[5]), Outcome::Return(Some(20)));
}

#[test]
fn decode_rejects_bad_magic() {
    assert!(decode(b"not a klbc image at all").is_err());
}
