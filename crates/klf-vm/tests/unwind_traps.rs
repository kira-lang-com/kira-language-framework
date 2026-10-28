//! Unwinding with no handler (section 12.14) on the VM, against
//! hand-worked results.

#[path = "../../klf-llvm/tests/unwind_corpus.rs"]
mod unwind_corpus;

use klf_vm::{Outcome, run_module};
use unwind_corpus::UNWIND;

#[test]
fn an_uncaught_throw_traps_at_the_boundary() {
    let module = klf_kir::parse_module(UNWIND).expect("module parses");
    assert_eq!(
        run_module(&module, "guarded", &[0]).expect("module lowers"),
        Outcome::Return(Some(7))
    );
    assert_eq!(
        run_module(&module, "guarded", &[1]).expect("module lowers"),
        Outcome::Trap("KLF-RT-0020".to_owned())
    );
}

#[test]
fn a_catch_clause_is_left_for_the_backend_to_refuse() {
    let source = UNWIND.replace(
        "func module @guarded(%x: i64) -> i64 engine(runtime) {\nbb0(%x: i64):\n",
        "func module @guarded(%x: i64) -> i64 engine(runtime) {\nbb0(%x: i64):\n  %typeinfo = inttoptr %x to ptr\n",
    )
    .replace("landingpad cleanup", "landingpad catch %typeinfo");
    let module = klf_kir::parse_module(&source).expect("module parses");
    assert!(
        klf_kir::unwinding_without_handlers(&module)
            .is_some_and(|legal| legal.to_string().contains("landingpad catch"))
    );
    assert!(run_module(&module, "guarded", &[0]).is_err());
}
