//! RIDL variadics (sections 12.14 and 29.5) on the VM, against hand-worked
//! results.

#[path = "../../klf-llvm/tests/variadic_corpus.rs"]
mod variadic_corpus;

use klf_vm::{Outcome, run_module};
use variadic_corpus::{FOREIGN_VARIADIC, VARIADIC};

#[test]
fn a_variadic_function_reads_its_arguments_slot_by_slot() {
    let module = klf_kir::parse_module(VARIADIC).expect("module parses");
    // summed: x + 10 + x. mixes: 7 * x + 2 + 2.
    for (entry, argument, expected) in [
        ("summed", 4, 18),
        ("summed", 0, 10),
        ("mixes", 3, 25),
        ("mixes", 10, 74),
    ] {
        assert_eq!(
            run_module(&module, entry, &[argument]).expect("module lowers"),
            Outcome::Return(Some(expected)),
            "{entry}({argument})"
        );
    }
}

#[test]
fn a_c_imports_variadic_tail_is_placed_as_c_reads_it() {
    let module = klf_kir::parse_module(FOREIGN_VARIADIC).expect("module parses");
    // "7|123456789012|2.50" and "7|-5|2.50"
    for (argument, length) in [(123_456_789_012_u64, 19), ((-5_i64).cast_unsigned(), 9)] {
        assert_eq!(
            run_module(&module, "formatted", &[argument]).expect("module lowers"),
            Outcome::Return(Some(length)),
        );
    }
}
