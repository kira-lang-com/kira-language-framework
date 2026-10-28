use klf_driver::{Backend, run_with_source_into_session};
use klf_services::{DiagnosticSpec, Session};
use klf_vm::Outcome;

const MODULE: &str = r#"module @Diagnostics lang "klfkira" target "aarch64-apple-darwin" stage canonical

func private foreign @klfsvc_diag_error(%code: i64, %start: i64, %end: i64) -> unit engine(native)

func module @main() -> i64 engine(native) {
entry():
  %code = const.int i64, 2001
  %start = const.int i64, 0
  %end = const.int i64, 1
  call.foreign @klfsvc_diag_error(%code, %start, %end)
  %result = const.int i64, 7
  return %result
}
"#;

#[test]
fn driver_resolves_runtime_diagnostics_into_session() {
    let module = klf_kir::parse_module(MODULE).expect("module parses");
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    let mut session = Session::new(target);
    let file = session.add_file("input.klfd", "A");
    session.register_diagnostic(DiagnosticSpec::new(
        2001,
        klf_diag::Severity::Error,
        "invalid token",
        "the token is not valid here",
    ));

    let outcome =
        run_with_source_into_session(&module, "main", &[], "A", Backend::Vm, &mut session, file)
            .expect("module runs");
    assert_eq!(outcome, Outcome::Return(Some(7)));
    assert_eq!(session.runtime_diagnostics().len(), 1);
    assert_eq!(session.runtime_diagnostics()[0].span.start, 0);
}
