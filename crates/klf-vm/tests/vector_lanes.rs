//! Vector values (section 12.5) on the VM, against hand-worked results.

#[path = "../../klf-llvm/tests/vector_corpus.rs"]
mod vector_corpus;

use klf_vm::{Outcome, run_module};
use vector_corpus::VECTORS;

#[test]
fn every_vector_instruction_computes_its_lanes() {
    let module = klf_kir::parse_module(VECTORS).expect("module parses");
    // lanes: e = [2n, 2, 4, 6], f = [6, 4, n, 1], g = e + f; g0 + g2 = 3n + 10.
    // clamped: [min(n, 10), 5] summed.
    let cases: [(&str, u64, u64); 4] = [
        ("lanes", 4, 22),
        ("lanes", 10, 40),
        ("clamped", 3, 8),
        ("clamped", 20, 15),
    ];
    for (entry, argument, expected) in cases {
        assert_eq!(
            run_module(&module, entry, &[argument]).expect("module lowers"),
            Outcome::Return(Some(expected)),
            "{entry}({argument})"
        );
    }
}
