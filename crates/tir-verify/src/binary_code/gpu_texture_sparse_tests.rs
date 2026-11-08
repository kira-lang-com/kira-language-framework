//! Sparse texel reads have a second, typed i1 residency result.
use super::*;
use crate::{verify_binary_module_records, verify_text_module};
use tir_diag::Severity;
use tir_format::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

const SOURCE: &str = "module @SparseTexel tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @SparseTexel.read(%tex: texture<2d, i32, read>, %coord: vec<2 x i32>) -> i1 target(gpu) {\nbb0(%tex: texture<2d, i32, read>, %coord: vec<2 x i32>):\n  %value, %resident = tex.read %tex, %coord [sparse]\n  br bb1(%value, %resident)\nbb1(%pixel: i32, %present: i1):\n  return %present\n}\n";

fn decoded() -> (BinaryModule, tir_format::BinarySignatureTable) {
    let parsed = parse_text_module(SOURCE).expect("sparse read text parses");
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("sparse read CODE assembles"))
            .expect("sparse TIRB decodes");
    let types = binary.type_table(u64::MAX).expect("sparse TYPES decode");
    let signatures = binary
        .signature_table(u64::MAX, &types)
        .expect("sparse SIGNATURES decode");
    (binary, signatures)
}

#[test]
fn sparse_read_text_binary_and_residency_branch_verify_and_round_trip() {
    let parsed = parse_text_module(SOURCE).expect("sparse read source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "sparse text V2 rejected: {text_errors:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("sparse CODE encodes");
    let (binary, signatures) = decoded();
    let binary_errors = verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "sparse binary V2 rejected: {binary_errors:?}"
    );
    let code = binary
        .code_table(u64::MAX, &signatures)
        .expect("sparse CODE decodes");
    let read = code.functions[0].blocks[0]
        .instructions
        .as_ref()
        .expect("decoded CODE")
        .iter()
        .find(|op| op.opcode == 0x488)
        .expect("tex.read");
    assert_eq!(read.result_count, 2);
    assert_eq!(read.modifiers & SPARSE, SPARSE);
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("sparse CODE disassembles");
    assert_eq!(
        bytes,
        assemble_text_module(&parse_text_module(&canonical).expect("canonical sparse text parses"))
            .expect("sparse CODE reassembles")
    );
}

#[test]
fn sparse_read_binary_rejects_missing_residency_result_and_wrong_modifiers() {
    let (binary, signatures) = decoded();
    let mut code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    let body = code.functions.first_mut().expect("sparse function");
    let read = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x488)
        .expect("tex.read");
    read.result_count = 1;
    let errors = verify(body, Some(BinaryTarget::Gpu), "read");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("result count")),
        "sparse read missing residency escaped V2: {errors:?}"
    );
    let read = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x488)
        .expect("tex.read");
    read.result_count = 2;
    read.modifiers = 0;
    let errors = verify(body, Some(BinaryTarget::Gpu), "read");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("result count")),
        "two-result read without sparse flag escaped V2: {errors:?}"
    );
    let read = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .iter_mut()
        .find(|op| op.opcode == 0x488)
        .expect("tex.read");
    read.modifiers = SPARSE | (1 << 48);
    let errors = verify(body, Some(BinaryTarget::Gpu), "read");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("selector")),
        "sparse read with forged selector escaped V2: {errors:?}"
    );
    let errors = verify(body, Some(BinaryTarget::Cpu), "read");
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("target(gpu)")),
        "CPU sparse texture read escaped V2: {errors:?}"
    );
}
