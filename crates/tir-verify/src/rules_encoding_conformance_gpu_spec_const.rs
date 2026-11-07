//! V2/V15 specialization ID/type/CONSTANTS conformance.
use super::verify_text_module;
use tir_format::parse_text_module;

#[test]
fn v2_v15_gpu_specialization_defaults_verify_scalar_vector_and_cfg() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module, disassemble_text_module};

    let source = "module @Spec tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Spec.scalar() -> i32 target(gpu) {\nbb0:\n  %first = gpu.spec_const 0x10, i32, -12\n  br bb1(%first)\nbb1(%forwarded: i32):\n  %second = gpu.spec_const 0x20, i32, 23\n  %sum = add %forwarded, %second\n  return %sum\n}\n\nfunc module @Spec.vector() -> vec<3 x f32> target(gpu) {\nbb0:\n  %result = gpu.spec_const 3, vec<3 x f32>, [1.5, 2.0, 3.0]\n  return %result\n}\n\nfunc module @Spec.boolean() -> i1 target(gpu) {\nbb0:\n  %result = gpu.spec_const 4, i1, true\n  return %result\n}\n";
    let parsed = parse_text_module(source).expect("GPU specialization source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "GPU specialization text V2/V15 rejected: {text_errors:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("GPU specialization CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("GPU specialization TIRB decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "GPU specialization binary V2 rejected: {binary_errors:?}"
    );
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("GPU specialization disassembles");
    assert_eq!(
        bytes,
        assemble_text_module(
            &parse_text_module(&canonical).expect("canonical GPU specialization parses")
        )
        .expect("canonical GPU specialization reassembles")
    );
}

#[test]
fn v2_binary_rejects_cpu_specialization_wrong_id_missing_or_wrong_default() {
    use tir_format::{
        BinaryConstantValue, BinaryModule, BinaryOperand, BinaryTarget, assemble_text_module,
    };
    let source = "module @Spec tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Spec.default() -> i32 target(gpu) {\nbb0:\n  %value = gpu.spec_const 1, i32, 42\n  return %value\n}\n";
    let parsed = parse_text_module(source).expect("valid specialization source parses");
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("valid specialization CODE assembles"),
    )
    .expect("valid TIRB decodes");
    let types = binary.type_table(u64::MAX).expect("TYPES decodes");
    let signatures = binary
        .signature_table(u64::MAX, &types)
        .expect("SIGNATURES decode");
    let constants = binary
        .constant_table(u64::MAX, &types)
        .expect("CONSTANTS decode");
    let mut code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    let body = code.functions.first_mut().expect("one function");
    let issues = super::binary_code::gpu_spec_const::verify(
        body,
        Some(BinaryTarget::Cpu),
        &types,
        Some(&constants),
        "default",
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("target(gpu)")),
        "CPU specialization escaped V2: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .first_mut()
        .expect("spec operation");
    first.operands[0] = BinaryOperand::Index(u64::from(u32::MAX) + 1);
    let issues = super::binary_code::gpu_spec_const::verify(
        body,
        Some(BinaryTarget::Gpu),
        &types,
        Some(&constants),
        "default",
    );
    assert!(
        issues.iter().any(|issue| issue.message.contains("ID")),
        "oversized ID escaped V2: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .first_mut()
        .expect("spec operation");
    first.operands[0] = BinaryOperand::Index(1);
    first.operands[2] = BinaryOperand::Index(u64::MAX);
    let issues = super::binary_code::gpu_spec_const::verify(
        body,
        Some(BinaryTarget::Gpu),
        &types,
        Some(&constants),
        "default",
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("CONSTANTS index")),
        "missing CONSTANTS record escaped V2: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .first_mut()
        .expect("spec operation");
    first.operands[2] = BinaryOperand::Index(0);
    first.operands[1] = BinaryOperand::Type(u64::MAX);
    let issues = super::binary_code::gpu_spec_const::verify(
        body,
        Some(BinaryTarget::Gpu),
        &types,
        Some(&constants),
        "default",
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("CONSTANTS type")),
        "wrong specialization result type escaped V2: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .first_mut()
        .expect("spec operation");
    first.operands[1] = BinaryOperand::Type(constants.records[0].ty);
    let mut wrong = constants.clone();
    wrong.records[0].value = BinaryConstantValue::Float(vec![0, 0, 0, 0]);
    let issues = super::binary_code::gpu_spec_const::verify(
        body,
        Some(BinaryTarget::Gpu),
        &types,
        Some(&wrong),
        "default",
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("invalid typed default")),
        "incorrect CONSTANTS kind escaped V2: {issues:?}"
    );
}

#[test]
fn v15_specialization_defaults_reject_inexact_and_invalid_text() {
    use tir_diag::Severity;
    use tir_format::assemble_text_module;
    for (ty, literal) in [
        ("i1", "1"),
        ("i5", "32"),
        ("f32", "0.1"),
        ("vec<3 x i32>", "[1, 2]"),
        ("vec<4 x f32>", "[1.5, 2.0, 0.1, 4.0]"),
    ] {
        let source = format!(
            "module @Spec tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Spec.invalid() -> unit target(gpu) {{\nbb0:\n  %result = gpu.spec_const 2, {ty}, {literal}\n  return\n}}\n"
        );
        let parsed = parse_text_module(&source).expect("specialization-invalid source parses");
        let errors = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            !errors.is_empty(),
            "V15 accepted invalid specialization {ty}={literal}"
        );
        assert!(
            assemble_text_module(&parsed).is_err(),
            "assembler accepted invalid specialization {ty}={literal}"
        );
    }
}
