//! V2 GPU builtin CODE and text name/type consistency.
use super::verify_text_module;
use tir_format::parse_text_module;

#[test]
fn v2_gpu_builtins_are_checked_in_text_and_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module, disassemble_text_module};

    let source = "module @Builtins tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Builtins.mask() -> i32 target(gpu) {\nbb0:\n  %first = gpu.builtin lane_index : i32\n  %second = gpu.builtin subgroup_size : i32\n  %sum = add %first, %second\n  return %sum\n}\n\nfunc module @Builtins.position() -> vec<3 x i32> target(gpu) {\nbb0:\n  %position = gpu.builtin thread_position_in_grid : vec<3 x i32>\n  return %position\n}\n";
    let parsed = parse_text_module(source).expect("GPU builtin text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "GPU builtin text V2 rejected: {text_errors:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("GPU builtin CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("GPU builtin TIRB decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "GPU builtin binary V2 rejected: {binary_errors:?}"
    );
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("GPU builtin disassembles");
    assert_eq!(
        bytes,
        assemble_text_module(&parse_text_module(&canonical).expect("canonical GPU builtin parses"))
            .expect("canonical GPU builtin reassembles")
    );
}

#[test]
fn v2_binary_rejects_forged_gpu_builtin_type_and_cpu_target() {
    use tir_format::{BinaryModule, BinaryOperand, BinaryTarget, assemble_text_module};

    let source = "module @Builtins tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Builtins.value() -> i32 target(gpu) {\nbb0:\n  %value = gpu.builtin lane_index : i32\n  return %value\n}\n";
    let parsed = parse_text_module(source).expect("valid builtin source parses");
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("valid builtin assembles"))
            .expect("valid builtin TIRB decodes");
    let types = binary.type_table(u64::MAX).expect("TYPES decodes");
    let signatures = binary
        .signature_table(u64::MAX, &types)
        .expect("SIGNATURES decode");
    let mut code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    let strings = binary.string_table(u64::MAX).expect("STRINGS decode");
    let body = code.functions.first_mut().expect("one function");
    let issues = super::binary_code::gpu_builtin::verify(
        body,
        Some(BinaryTarget::Cpu),
        &types,
        Some(&strings),
        "value",
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("target(gpu)")),
        "CPU-targeted builtins escaped binary V2: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded instructions")
        .first_mut()
        .expect("builtin instruction");
    first.operands[1] = BinaryOperand::Type(u64::MAX);
    let issues = super::binary_code::gpu_builtin::verify(
        body,
        Some(BinaryTarget::Gpu),
        &types,
        Some(&strings),
        "value",
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("disagree")),
        "forged GPU builtin result type escaped binary V2: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded instructions")
        .first_mut()
        .expect("builtin instruction");
    first.operands[0] = BinaryOperand::Index(u64::MAX);
    let issues = super::binary_code::gpu_builtin::verify(
        body,
        Some(BinaryTarget::Gpu),
        &types,
        Some(&strings),
        "value",
    );
    assert!(
        issues.iter().any(|issue| issue.message.contains("STRINGS")),
        "invalid builtin string index escaped binary V2: {issues:?}"
    );
}

#[test]
fn v2_gpu_builtin_mismatched_text_declarations_are_rejected() {
    use tir_diag::Severity;
    for (builtin, ty) in [
        ("lane_index", "i1"),
        ("position", "vec<3 x f32>"),
        ("invalid_gpu_builtin", "i32"),
    ] {
        let source = format!(
            "module @Builtins tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Builtins.invalid() -> unit target(gpu) {{\nbb0:\n  %value = gpu.builtin {builtin} : {ty}\n  return\n}}\n"
        );
        let parsed = parse_text_module(&source).expect("mismatched builtin source parses");
        let diagnostics = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            !diagnostics.is_empty(),
            "mismatched {builtin} : {ty} passed text V2"
        );
        assert!(
            tir_format::assemble_text_module(&parsed).is_err(),
            "mismatched {builtin} : {ty} assembled into CODE"
        );
    }
}
