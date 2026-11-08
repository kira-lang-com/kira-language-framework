//! Binary V2 conformance for gather selectors and comparison sampling.
use super::*;
use crate::{verify_binary_module_records, verify_text_module};
use tir_diag::Severity;
use tir_format::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

const SOURCE: &str = "module @GatherV2 tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @GatherV2.gather(%tex: texture<2d, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>) -> vec<4 x f32> target(gpu) {\nbb0(%tex: texture<2d, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>):\n  %value = tex.gather %tex, %smp, %uv component 3 offset [-8, 7] [unchecked_uniformity]\n  return %value\n}\n\nfunc module @GatherV2.compare(%tex: texture<2darray, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %layer: i32, %ref: f32) -> vec<4 x f32> target(gpu) {\nbb0(%tex: texture<2darray, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %layer: i32, %ref: f32):\n  %value = tex.gather_compare %tex, %smp, %uv, %layer, %ref\n  return %value\n}\n";

fn decoded() -> (BinaryModule, tir_format::BinarySignatureTable) {
    let parsed = parse_text_module(SOURCE).expect("gather V2 fixture parses");
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("gather CODE assembles"))
            .expect("gather TIRB decodes");
    let types = binary.type_table(u64::MAX).expect("TYPES decode");
    let signatures = binary
        .signature_table(u64::MAX, &types)
        .expect("SIGNATURES decode");
    (binary, signatures)
}

#[test]
fn gather_text_and_binary_v2_verify_with_byte_identical_round_trip() {
    let parsed = parse_text_module(SOURCE).expect("gather text source parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "valid gather text V2 rejected: {errors:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("gather CODE assembles");
    let (binary, _) = decoded();
    let errors = verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "valid gather binary V2 rejected: {errors:?}"
    );
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("gather CODE disassembles");
    assert_eq!(
        bytes,
        assemble_text_module(&parse_text_module(&canonical).expect("canonical gather text parses"))
            .expect("gather CODE reassembles")
    );
}

#[test]
fn gather_binary_v2_rejects_forged_selector_results_and_operand_counts() {
    let (binary, signatures) = decoded();
    let mut code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    let gather = code.functions.first_mut().expect("gather function");
    let operation = gather.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x486)
        .expect("gather opcode");
    operation.modifiers = 4 << 48;
    let issues = verify(gather, Some(BinaryTarget::Gpu), "gather");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("selector")),
        "selector 4 escaped V2: {issues:?}"
    );
    let operation = gather.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x486)
        .expect("gather opcode");
    operation.modifiers = 0;
    operation.result_count = 2;
    let issues = verify(gather, Some(BinaryTarget::Gpu), "gather");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("one SSA result")),
        "gather two results escaped V2: {issues:?}"
    );
    let compare = code
        .functions
        .get_mut(1)
        .expect("gather comparison function");
    let operation = compare.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x487)
        .expect("gather compare opcode");
    operation.modifiers = 1 << 48;
    let issues = verify(compare, Some(BinaryTarget::Gpu), "compare");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("modifiers")),
        "gather comparison accepted selector bits: {issues:?}"
    );
    let operation = compare.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x487)
        .expect("gather compare opcode");
    operation.modifiers = 0;
    operation.operands = vec![
        BinaryOperand::Values(vec![0, 1]),
        BinaryOperand::Integers(Vec::new()),
    ];
    let issues = verify(compare, Some(BinaryTarget::Gpu), "compare");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("operand count")),
        "gather comparison missing reference escaped V2: {issues:?}"
    );
}
