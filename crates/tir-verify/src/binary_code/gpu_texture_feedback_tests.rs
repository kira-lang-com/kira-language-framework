//! Feedback CODE and forged-binary structural conformance.
use super::*;
use crate::{verify_binary_module_records, verify_text_module};
use tir_diag::Severity;
use tir_format::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

const SOURCE: &str = "module @FeedbackV2 tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @FeedbackV2.sample(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lower: f32) -> f32 target(gpu) {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lower: f32):\n  %result = tex.feedback %tex, %smp, %uv offset [-8, 7] min_lod %lower [unchecked_uniformity]\n  return %result\n}\n\nfunc module @FeedbackV2.array(%tex: texture<2darray, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>, %layer: i32) -> vec<4 x f32> target(gpu) {\nbb0(%tex: texture<2darray, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>, %layer: i32):\n  %result = tex.feedback %tex, %smp, %uv, %layer\n  return %result\n}\n";

fn decoded() -> (BinaryModule, tir_format::BinarySignatureTable) {
    let parsed = parse_text_module(SOURCE).expect("feedback fixture parses");
    let bytes = assemble_text_module(&parsed).expect("feedback CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("feedback TIRB decodes");
    let types = binary.type_table(u64::MAX).expect("feedback TYPES decode");
    let signatures = binary
        .signature_table(u64::MAX, &types)
        .expect("feedback SIGNATURES decode");
    (binary, signatures)
}

#[test]
fn feedback_text_and_binary_v2_verify_and_reassemble_identically() {
    let parsed = parse_text_module(SOURCE).expect("feedback source parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "feedback text verification rejected valid CODE: {errors:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("feedback CODE assembles");
    let (binary, _) = decoded();
    let errors = verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "feedback binary verification rejected valid CODE: {errors:?}"
    );
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("feedback CODE disassembles");
    assert_eq!(
        bytes,
        assemble_text_module(&parse_text_module(&canonical).expect("canonical feedback parses"))
            .expect("feedback CODE reassembles")
    );
}

#[test]
fn feedback_binary_v2_rejects_cpu_modifiers_result_count_and_operand_shape() {
    let (binary, signatures) = decoded();
    let mut code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    let body = code.functions.first_mut().expect("feedback function");
    let errors = verify(body, Some(BinaryTarget::Cpu), "sample");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("target(gpu)")),
        "CPU feedback escaped V2: {errors:?}"
    );
    let operation = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x48d)
        .expect("feedback opcode");
    operation.result_count = 2;
    let errors = verify(body, Some(BinaryTarget::Gpu), "sample");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("one SSA result")),
        "extra feedback result escaped V2: {errors:?}"
    );
    let operation = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x48d)
        .expect("feedback opcode");
    operation.result_count = 1;
    operation.modifiers = 1 << 48;
    let errors = verify(body, Some(BinaryTarget::Gpu), "sample");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("modifiers")),
        "feedback selector escaped V2: {errors:?}"
    );
    let operation = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x48d)
        .expect("feedback opcode");
    operation.modifiers = 0;
    operation.operands = vec![
        BinaryOperand::Values(vec![0, 1]),
        BinaryOperand::Integers(Vec::new()),
    ];
    let errors = verify(body, Some(BinaryTarget::Gpu), "sample");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("operand count")),
        "feedback missing coordinates escaped V2: {errors:?}"
    );
}

#[test]
fn feedback_binary_v2_rejects_invalid_offsets_and_sparse_flag_without_residency() {
    let (binary, signatures) = decoded();
    let mut code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    let body = code.functions.first_mut().expect("feedback function");
    let operation = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x48d)
        .expect("feedback opcode");
    operation.modifiers = 1 << 35;
    let errors = verify(body, Some(BinaryTarget::Gpu), "sample");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("residency")),
        "sparse feedback without residency escaped V2: {errors:?}"
    );
    let operation = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x48d)
        .expect("feedback opcode");
    operation.modifiers = 0;
    operation.operands = vec![
        BinaryOperand::Values(vec![0, 1, 2]),
        BinaryOperand::Integers(vec![tir_format::leb::IntegerImmediate {
            negative: false,
            magnitude: 8,
        }]),
    ];
    let errors = verify(body, Some(BinaryTarget::Gpu), "sample");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("outside -8..=7")),
        "out-of-range texture offset escaped V2: {errors:?}"
    );
}
