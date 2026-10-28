//! What `klf_kir::legalize` hands the backends still verifies: indirect
//! branches as switches, RIDL variadics as argument areas, vectors as
//! lanes, handlerless unwinding as traps and concrete witness calls as
//! direct calls.

#[path = "../../klf-llvm/tests/indirect_corpus.rs"]
mod indirect_corpus;
#[path = "../../klf-llvm/tests/unwind_corpus.rs"]
mod unwind_corpus;
#[allow(dead_code)]
#[path = "../../klf-llvm/tests/variadic_corpus.rs"]
mod variadic_corpus;
#[path = "../../klf-llvm/tests/vector_corpus.rs"]
mod vector_corpus;
#[path = "../../klf-llvm/tests/witness_corpus.rs"]
mod witness_corpus;

#[test]
fn legalized_modules_verify() {
    for source in [
        indirect_corpus::INDIRECT,
        unwind_corpus::UNWIND,
        variadic_corpus::VARIADIC,
        vector_corpus::VECTORS,
        witness_corpus::WITNESS,
    ] {
        let module = klf_kir::parse_module(source).expect("module parses");
        let diagnostics = klf_kir_verify::verify_module(&module);
        assert!(diagnostics.is_empty(), "{diagnostics:?}\n{module}");
        let legal = klf_kir::legalize(&module).expect("something to rewrite");
        let diagnostics = klf_kir_verify::verify_module(&legal);
        assert!(diagnostics.is_empty(), "{diagnostics:?}\n{legal}");
        let text = legal.to_string();
        for gone in [
            "block.addr",
            "br.indirect",
            "va.",
            "call.variadic",
            "vec.",
            "invoke ",
            "landingpad",
            "resume %",
            "throw %",
            "call.witness",
        ] {
            assert!(!text.contains(gone), "{gone} left in\n{text}");
        }
    }
}
