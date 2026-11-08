use super::verify_text_module;
use tir_format::parse_text_module;

fn source() -> String {
    "module @GpuPrintf tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @GpuPrintf.print() -> i32 target(gpu) {\nbb0:\n  %first = gpu.spec_const 2, i32, -7\n  gpu.printf \"first=%d\", %first\n  br bb1(%first)\nbb1(%arg: i32):\n  gpu.printf \"value=%d\", %arg\n  return %arg\n}\n".to_owned()
}

#[test]
fn v2_gpu_printf_text_binary_and_cfg_round_trip() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module, disassemble_text_module};

    let parsed = parse_text_module(&source()).expect("GPU printf parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "GPU printf V2/V15 text rejected: {errors:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("GPU printf CODE encodes");
    let binary = BinaryModule::decode(&bytes).expect("GPU printf TIRB decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "GPU printf binary V2 rejected: {errors:?}"
    );
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("GPU printf disassembles");
    assert_eq!(
        bytes,
        assemble_text_module(&parse_text_module(&canonical).expect("canonical GPU printf parses"))
            .expect("GPU printf reassembles")
    );
}

#[test]
fn v2_gpu_printf_binary_rejects_cpu_modified_results_and_bad_format_index() {
    use tir_format::{BinaryModule, BinaryOperand, BinaryTarget, assemble_text_module};

    let parsed = parse_text_module(&source()).expect("valid GPU printf parses");
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("valid GPU printf assembles"))
            .expect("valid TIRB decodes");
    let types = binary.type_table(u64::MAX).expect("TYPES decodes");
    let signatures = binary
        .signature_table(u64::MAX, &types)
        .expect("SIGNATURES decode");
    let strings = binary.string_table(u64::MAX).expect("STRINGS decode");
    let mut code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    let body = code.functions.first_mut().expect("one function");
    let errors = super::binary_code::gpu_printf::verify(
        body,
        Some(BinaryTarget::Cpu),
        Some(&strings),
        "print",
    );
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("target(gpu)")),
        "CPU gpu.printf escaped V2: {errors:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE decoded")
        .iter_mut()
        .find(|op| op.opcode == 0x40a)
        .expect("gpu.printf operation");
    first.result_count = 1;
    let errors = super::binary_code::gpu_printf::verify(
        body,
        Some(BinaryTarget::Gpu),
        Some(&strings),
        "print",
    );
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("zero modifiers")),
        "forged printf results escaped V2: {errors:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE decoded")
        .iter_mut()
        .find(|op| op.opcode == 0x40a)
        .expect("gpu.printf operation");
    first.result_count = 0;
    first.modifiers = 1;
    let errors = super::binary_code::gpu_printf::verify(
        body,
        Some(BinaryTarget::Gpu),
        Some(&strings),
        "print",
    );
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("zero modifiers")),
        "forged printf modifier escaped V2: {errors:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE decoded")
        .iter_mut()
        .find(|op| op.opcode == 0x40a)
        .expect("gpu.printf operation");
    first.modifiers = 0;
    first.operands[0] = BinaryOperand::Index(u64::MAX);
    let errors = super::binary_code::gpu_printf::verify(
        body,
        Some(BinaryTarget::Gpu),
        Some(&strings),
        "print",
    );
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("STRINGS index")),
        "invalid printf format index escaped V2: {errors:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE decoded")
        .iter_mut()
        .find(|op| op.opcode == 0x40a)
        .expect("gpu.printf operation");
    first.operands[0] = BinaryOperand::Value(0);
    let errors = super::binary_code::gpu_printf::verify(
        body,
        Some(BinaryTarget::Gpu),
        Some(&strings),
        "print",
    );
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("counted SSA")),
        "wrong printf operand shape escaped V2: {errors:?}"
    );
}

#[test]
fn v2_gpu_printf_requires_gpu_target_and_defined_ssa_arguments() {
    let invalid = "module @GpuPrintf tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @GpuPrintf.cpu() -> unit engine(native) {\nbb0:\n  gpu.printf \"not valid in CPU\"\n  return\n}\n";
    let parsed = parse_text_module(invalid).expect("CPU printf text parses");
    assert!(tir_format::assemble_text_module(&parsed).is_err());
    let invalid = "module @GpuPrintf tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @GpuPrintf.bad() -> unit target(gpu) {\nbb0:\n  gpu.printf \"bad\", %not_defined\n  return\n}\n";
    let parsed = parse_text_module(invalid).expect("undefined printf operand parses");
    assert!(tir_format::assemble_text_module(&parsed).is_err());
}
