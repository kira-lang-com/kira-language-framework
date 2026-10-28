//! `select`, `fma`, the raw pointer instructions and the layout queries on
//! the VM, against hand-worked results.

#[path = "../../klf-llvm/tests/unmanaged_corpus.rs"]
mod unmanaged_corpus;

use klf_vm::{Outcome, run_module};
use unmanaged_corpus::UNMANAGED;

fn run(entry: &str, argument: u64) -> Outcome {
    let module = klf_kir::parse_module(UNMANAGED).expect("module parses");
    run_module(&module, entry, &[argument]).expect("module lowers")
}

#[test]
fn every_unmanaged_instruction_computes_its_value() {
    let cases: [(&str, u64, u64); 12] = [
        ("picks", 3, 6),
        ("picks", 20, 120),
        ("fused", 2, 21),
        ("raw", 4, 478),
        ("sizes", 0, 80_416),
        ("borrowed", 21, 42),
        ("unowned", 41, 42),
        ("fused", 0, 9),
        ("absolute", 42, 42),
        ("checked", 0, 0),
        ("checked", 3, 101_009),
        ("checked", 1 << 32, 101_000),
    ];
    for (entry, argument, expected) in cases {
        assert_eq!(
            run(entry, argument),
            Outcome::Return(Some(expected)),
            "`{entry}({argument})`"
        );
    }
}
