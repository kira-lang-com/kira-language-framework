//! Witness calls on a concrete type (section 12.11) on the VM with no
//! optimization, against hand-worked results.

#[path = "../../klf-llvm/tests/witness_corpus.rs"]
mod witness_corpus;

use klf_vm::{Outcome, run_module};
use witness_corpus::WITNESS;

#[test]
fn a_concrete_witness_call_reaches_its_slot() {
    let module = klf_kir::parse_module(WITNESS).expect("module parses");
    // twice(20) + size(20) = 40 + 8.
    assert_eq!(
        run_module(&module, "describe", &[20]).expect("module lowers"),
        Outcome::Return(Some(48))
    );
}
