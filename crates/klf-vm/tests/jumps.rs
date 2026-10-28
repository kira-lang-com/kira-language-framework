//! `setjmp` and `longjmp` (section 12.14) on the VM, against hand-worked
//! results.

#[path = "../../klf-llvm/tests/jump_corpus.rs"]
mod jump_corpus;

use jump_corpus::JUMPS;
use klf_vm::{Outcome, run_module};

#[test]
fn a_long_jump_returns_through_the_set_jump() {
    let module = klf_kir::parse_module(JUMPS).expect("module parses");
    // 3 * 7 + 1 = 22 comes back as the second answer of `setjmp`.
    assert_eq!(
        run_module(&module, "caught", &[3]).expect("module lowers"),
        Outcome::Return(Some(122))
    );
    // -1 * 7 + 1 = -6 jumps back as -6.
    assert_eq!(
        run_module(&module, "caught", &[u64::MAX]).expect("module lowers"),
        Outcome::Return(Some(94))
    );
}

#[test]
fn a_long_jump_of_zero_answers_one() {
    let source = JUMPS.replace("%more = add [wrap] %n, %one", "%more = sub [wrap] %n, %n");
    let module = klf_kir::parse_module(&source).expect("module parses");
    assert_eq!(
        run_module(&module, "caught", &[5]).expect("module lowers"),
        Outcome::Return(Some(101))
    );
}
