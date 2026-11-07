//! Additional fixed-vector CODE conformance round-trip cases.
use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

#[test]
fn integer_vector_arithmetic_comparison_and_select_round_trip() {
    for lanes in [2, 3, 4, 8] {
        let ty = format!("vec<{lanes} x i32>");
        let source = format!(
            "module @Vector tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Vector.compute(%a: {ty}, %b: {ty}) -> {ty} engine(native) {{\nbb0(%a: {ty}, %b: {ty}):\n  %sum = add %a, %b\n  %difference = sub %sum, %b\n  %mask = icmp slt %sum, %b\n  %result = select %mask, %difference, %a\n  return %result\n}}\n"
        );
        let parsed = parse_text_module(&source).expect("vector integer text parses");
        let bytes = assemble_text_module(&parsed).expect("integer-vector CODE assembles");
        let binary = BinaryModule::decode(&bytes).expect("binary decodes");
        let canonical =
            disassemble_text_module(&binary, u64::MAX).expect("integer-vector CODE disassembles");
        assert!(canonical.contains("icmp slt"));
        let rebuilt = assemble_text_module(&parse_text_module(&canonical).expect("text reparses"))
            .expect("canonical vector CODE reassembles");
        assert_eq!(bytes, rebuilt);
    }
}

#[test]
fn integer_vector_select_rejects_wrong_mask_shape() {
    let source = "module @Vector tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Vector.compute(%a: vec<4 x i32>, %mask: vec<2 x i1>) -> vec<4 x i32> engine(native) {\nbb0(%a: vec<4 x i32>, %mask: vec<2 x i1>):\n  %result = select %mask, %a, %a\n  return %result\n}\n";
    let parsed = parse_text_module(source).expect("text parses");
    assert!(
        assemble_text_module(&parsed).is_err(),
        "invalid vector mask was assembled"
    );
}
