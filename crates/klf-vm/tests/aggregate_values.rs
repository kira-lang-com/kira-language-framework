//! Aggregate values (section 12.6) on the VM, against hand-worked results.

#[path = "../../klf-llvm/tests/aggregates_corpus.rs"]
mod aggregates_corpus;

use aggregates_corpus::AGGREGATES;
use klf_vm::{Outcome, run_module};

fn run(entry: &str, argument: u64) -> Outcome {
    let module = klf_kir::parse_module(AGGREGATES).expect("module parses");
    run_module(&module, entry, &[argument]).expect("module lowers")
}

#[test]
fn every_aggregate_instruction_computes_its_value() {
    let cases: [(&str, u64, u64); 10] = [
        ("pairs", 7, 703),
        ("scaled", 4, 12_015),
        ("tuples", 5, 42),
        ("arrays", 8, 95),
        ("shapes", 0, 0),
        ("shapes", 6, 24),
        ("fibonacci", 0, 0),
        ("fibonacci", 10, 55),
        ("fibonacci", 50, 12_586_269_025),
        ("stored", 4, 5),
    ];
    for (entry, argument, expected) in cases {
        assert_eq!(
            run(entry, argument),
            Outcome::Return(Some(expected)),
            "`{entry}({argument})`"
        );
    }
}

