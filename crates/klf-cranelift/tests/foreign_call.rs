//! `call.foreign` (section 26.3): a KIR program that calls a platform-C libc
//! symbol lowers to a native object, links against libc, and returns the C
//! function's real result.

mod support;

use klf_vm::Outcome;
use support::cranelift_run;

const SERVICE: &str = r#"module @IoDemo lang "c" target "aarch64-apple-darwin" stage canonical

func private foreign @klfsvc_io_put(%c: i32) -> i32 engine(native)

func module @main() -> i64 engine(native) {
entry():
  %h = const.int i32, 72
  %r1 = call.foreign @klfsvc_io_put(%h)
  %i = const.int i32, 73
  %r2 = call.foreign @klfsvc_io_put(%i)
  %z = const.int i64, 0
  return %z
}
"#;

#[test]
fn native_calls_a_kernel_service_symbol() {
    // The native object calls `klfsvc_io_put`, linked from klf-native-rt: the
    // service runs in the compiled binary and prints its output (`IO 72`,
    // `IO 73`) alongside the program's `RET 0`.
    let stdout = support::cranelift_stdout(SERVICE, "main", &[]);
    assert!(stdout.contains("IO 72"), "service wrote 'H': {stdout:?}");
    assert!(stdout.contains("IO 73"), "service wrote 'I': {stdout:?}");
    assert!(stdout.contains("RET 0"), "program returned 0: {stdout:?}");
}

const FOREIGN_AS_RIDL: &str = r#"module @ForeignRidl lang "c" target "aarch64-apple-darwin" stage canonical

func private foreign @labs(%x: i64) -> i64 engine(native)

func private @apply(%f: fn(i64) -> i64 abi(ridl), %x: i64) -> i64 engine(native) abi(ridl) {
entry(%f: fn(i64) -> i64 abi(ridl), %x: i64):
  %r = call.indirect %f(%x) : (i64) -> i64
  return %r
}

func module @main() -> i64 engine(native) {
entry():
  %f = func.ref @labs
  %ridl = bitcast %f to fn(i64) -> i64 abi(ridl)
  %n = const.int i64, -42
  %r = call @apply(%ridl, %n)
  return %r
}
"#;

#[test]
fn a_foreign_function_converts_to_ridl_through_a_thunk() {
    let module = klf_kir_passes::monomorphize(
        &klf_kir::parse_module(FOREIGN_AS_RIDL).expect("module parses"),
    );
    assert!(
        klf_kir_verify::verify_module(&module).is_empty(),
        "the foreign adapter verifies"
    );
    assert!(
        module.to_string().contains("call.foreign @labs"),
        "the thunk calls the foreign symbol:\n{module}"
    );
    assert_eq!(
        cranelift_run(FOREIGN_AS_RIDL, "main", &[]),
        Outcome::Return(Some(42))
    );
}
