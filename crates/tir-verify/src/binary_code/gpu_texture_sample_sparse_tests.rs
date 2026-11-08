//! Sparse sampling produces a typed pixel/sample result and an i1 residency result.
use super::*;
use crate::{verify_binary_module_records, verify_text_module};
use tir_diag::Severity;
use tir_format::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

const SOURCE: &str = "module @SparseSamplingV2 tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @SparseSamplingV2.sample(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %min: f32) -> i1 target(gpu) {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %min: f32):\n  %pixel, %residency = tex.sample %tex, %smp, %uv offset [-8, 7] min_lod %min [unchecked_uniformity] [sparse]\n  br bb1(%pixel, %residency)\nbb1(%value: f32, %present: i1):\n  return %present\n}\n\nfunc module @SparseSamplingV2.gather(%tex: texture<2d, i32, sample>, %smp: sampler, %uv: vec<2 x f32>) -> i1 target(gpu) {\nbb0(%tex: texture<2d, i32, sample>, %smp: sampler, %uv: vec<2 x f32>):\n  %components, %resident = tex.gather %tex, %smp, %uv component 3 [sparse]\n  return %resident\n}\n\nfunc module @SparseSamplingV2.feedback(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>) -> i1 target(gpu) {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>):\n  %pixel, %resident = tex.feedback %tex, %smp, %uv [sparse]\n  return %resident\n}\n";

fn decoded() -> (BinaryModule, tir_format::BinarySignatureTable) {
    let parsed = parse_text_module(SOURCE).expect("sparse sampled texture text parses");
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("sparse sampled CODE assembles"),
    )
    .expect("sparse TIRB decodes");
    let types = binary.type_table(u64::MAX).expect("sparse TYPES decode");
    let signatures = binary
        .signature_table(u64::MAX, &types)
        .expect("sparse SIGNATURES decode");
    (binary, signatures)
}

#[test]
fn sparse_sample_gather_feedback_binary_v2_and_residency_cfg_round_trip() {
    let parsed = parse_text_module(SOURCE).expect("sparse source parses");
    let text_issues = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_issues.is_empty(),
        "sparse sample text V2 rejected: {text_issues:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("sparse CODE assembles");
    let (binary, signatures) = decoded();
    let errors = verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(errors.is_empty(), "sparse binary V2 rejected: {errors:?}");
    let code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    for function in &code.functions {
        let sparse = function.blocks[0]
            .instructions
            .as_ref()
            .expect("CODE instructions")
            .iter()
            .find(|op| matches!(op.opcode, 0x480 | 0x486 | 0x48d))
            .expect("sparse texture operation");
        assert_eq!(sparse.result_count, 2);
        assert_ne!(sparse.modifiers & SPARSE, 0);
    }
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("CODE disassembles");
    assert_eq!(
        bytes,
        assemble_text_module(
            &parse_text_module(&canonical).expect("canonical sparse sampling parses")
        )
        .expect("sparse sampling reassembles")
    );
}

#[test]
fn sparse_sample_binary_v2_rejects_missing_residency_and_forged_non_sparse_lod() {
    let (binary, signatures) = decoded();
    let mut code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    let body = code.functions.first_mut().expect("sample function");
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x480)
        .expect("tex.sample");
    first.result_count = 1;
    let errors = verify(body, Some(BinaryTarget::Gpu), "sample");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("residency")),
        "sparse sample missing second result escaped V2: {errors:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x480)
        .expect("tex.sample");
    first.result_count = 2;
    first.modifiers &= !SPARSE;
    let errors = verify(body, Some(BinaryTarget::Gpu), "sample");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("residency")),
        "non-sparse sample with two results escaped V2: {errors:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x480)
        .expect("tex.sample");
    first.modifiers = SPARSE;
    first.opcode = 0x48c;
    let errors = verify(body, Some(BinaryTarget::Gpu), "sample");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("modifiers")),
        "sparse tex.lod escaped V2: {errors:?}"
    );
}

#[test]
fn sparse_gather_selector_and_cpu_forgery_fail_binary_v2() {
    let (binary, signatures) = decoded();
    let mut code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    let body = code.functions.get_mut(1).expect("gather function");
    let errors = verify(body, Some(BinaryTarget::Cpu), "gather");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("target(gpu)")),
        "CPU sparse gather escaped V2: {errors:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x486)
        .expect("tex.gather");
    first.modifiers = SPARSE | (4 << 48);
    let errors = verify(body, Some(BinaryTarget::Gpu), "gather");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("selector")),
        "sparse gather selector 4 escaped V2: {errors:?}"
    );
}
