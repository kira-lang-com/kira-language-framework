//! Block addresses and indirect branches (section 12.13) on the VM,
//! against hand-worked results.

#[path = "../../klf-llvm/tests/indirect_corpus.rs"]
mod indirect_corpus;

use indirect_corpus::INDIRECT;
use klf_vm::{Outcome, run_module};

#[test]
fn an_indirect_branch_reaches_the_addressed_block() {
    let module = klf_kir::parse_module(INDIRECT).expect("module parses");
    for (argument, expected) in [(0, 10), (1, 20), (5, 35)] {
        assert_eq!(
            run_module(&module, "dispatch", &[argument]).expect("module lowers"),
            Outcome::Return(Some(expected)),
            "dispatch({argument})"
        );
    }
}
