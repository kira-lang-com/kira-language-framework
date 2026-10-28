use klf_driver::{Backend, run};
use klf_vm::Outcome;

/// `main` registers `ending`, then returns or traps by its argument. `ending`
/// traps with a code naming the `reason * 1000 + detail` it was given.
const MODULE: &str = r#"module @RuntimeExit lang "klfkira" target "aarch64-apple-darwin" stage canonical

func private foreign @klfsvc_runtime_at_exit(%function: fn(i64, i64) -> unit) -> i64 engine(native)

func module @ending(%reason: i64, %detail: i64) -> unit engine(native) {
entry(%reason: i64, %detail: i64):
  %thousand = const.int i64, 1000
  %scaled = mul [wrap] s %reason, %thousand
  %code = add [wrap] s %scaled, %detail
  switch.int %code, [7: returned(), 1042: trapped()], default other()
returned():
  trap KLF-RT-0201
trapped():
  trap KLF-RT-0202
other():
  trap KLF-RT-0203
}

func module @main(%trapping: i64) -> i64 engine(native) {
entry(%trapping: i64):
  %ending = func.ref @ending
  %previous = call.foreign @klfsvc_runtime_at_exit(%ending)
  %zero = const.int i64, 0
  %traps = icmp ne %trapping, %zero
  cond.br %traps, trapped(), returned()
returned():
  %seven = const.int i64, 7
  return %seven
trapped():
  trap KLF-RT-0042
}
"#;

#[test]
fn the_exit_function_sees_how_the_program_ended() {
    let module = klf_kir::parse_module(MODULE).expect("module parses");
    for backend in [Backend::Vm, Backend::Native] {
        assert_eq!(
            run(&module, "main", &[0], backend),
            Ok(Outcome::Trap("KLF-RT-0201".to_owned())),
            "{backend:?} return"
        );
        assert_eq!(
            run(&module, "main", &[1], backend),
            Ok(Outcome::Trap("KLF-RT-0202".to_owned())),
            "{backend:?} trap"
        );
    }
}
