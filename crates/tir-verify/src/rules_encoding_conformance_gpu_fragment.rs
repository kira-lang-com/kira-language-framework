use super::verify_text_module;
use tir_format::parse_text_module;

fn module(body: &str) -> String {
    format!(
        "module @Fragments tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

#[test]
fn v2_fragment_control_code_verifies_text_and_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module, disassemble_text_module};

    let source = module(
        "func module @Fragments.controls() -> unit target(gpu) {\nbb0:\n  frag.demote\n  return\n}\n",
    );
    let parsed = parse_text_module(&source).expect("GPU fragment control parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "GPU fragment text verification failed: {text_errors:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("fragment CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("fragment TIRB decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "fragment binary V2 failed: {binary_errors:?}"
    );
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("fragment disassembles");
    assert_eq!(
        bytes,
        assemble_text_module(
            &parse_text_module(&canonical).expect("canonical fragment text parses")
        )
        .expect("fragment reassembles")
    );
}

#[test]
fn v2_binary_fragment_attachment_and_tile_address_result_shape() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Fragments.attachment() -> vec<4 x f32> target(gpu) {\nbb0:\n  %value = frag.load_attachment 42\n  return %value\n}\n\nfunc module @Fragments.address() -> ptr<tile> target(gpu) {\nbb0:\n  %address = frag.imageblock.addr\n  return %address\n}\n",
    );
    let parsed = parse_text_module(&source).expect("fragment result source parses");
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("fragment CODE assembles"))
            .expect("fragment TIRB decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "fragment result binary V2 failed: {binary_errors:?}"
    );
}

#[test]
fn v2_binary_rejects_cpu_fragment_invalid_attachment_and_forged_results() {
    use tir_format::{BinaryModule, BinaryOperand, BinaryTarget, assemble_text_module};

    let source = module(
        "func module @Fragments.index() -> vec<4 x f32> target(gpu) {\nbb0:\n  %result = frag.load_attachment 1\n  return %result\n}\n",
    );
    let parsed = parse_text_module(&source).expect("valid fragment source parses");
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("valid fragment CODE assembles"),
    )
    .expect("valid fragment TIRB decodes");
    let types = binary.type_table(u64::MAX).expect("TYPES decodes");
    let signatures = binary
        .signature_table(u64::MAX, &types)
        .expect("SIGNATURES decode");
    let mut code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    let body = code.functions.first_mut().expect("one function");
    let issues = super::binary_code::gpu_fragment::verify(body, Some(BinaryTarget::Cpu), "index");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("target(gpu)")),
        "CPU fragment CODE escaped V2: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE instructions")
        .first_mut()
        .expect("fragment instruction");
    first.result_count = 0;
    let issues = super::binary_code::gpu_fragment::verify(body, Some(BinaryTarget::Gpu), "index");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("result count")),
        "missing fragment SSA result escaped V2: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE instructions")
        .first_mut()
        .expect("fragment instruction");
    first.result_count = 1;
    first.modifiers = 1;
    let issues = super::binary_code::gpu_fragment::verify(body, Some(BinaryTarget::Gpu), "index");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("modifier")),
        "forged fragment modifier escaped V2: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE instructions")
        .first_mut()
        .expect("fragment instruction");
    first.modifiers = 0;
    if let BinaryOperand::Integer(index) = &mut first.operands[0] {
        index.negative = true;
    } else {
        panic!("attachment operand is not integer");
    }
    let issues = super::binary_code::gpu_fragment::verify(body, Some(BinaryTarget::Gpu), "index");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("unsigned u32")),
        "negative fragment attachment escaped V2: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("CODE instructions")
        .first_mut()
        .expect("fragment instruction");
    if let BinaryOperand::Integer(index) = &mut first.operands[0] {
        index.negative = false;
        index.magnitude = u128::from(u32::MAX) + 1;
    }
    let issues = super::binary_code::gpu_fragment::verify(body, Some(BinaryTarget::Gpu), "index");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("unsigned u32")),
        "oversized fragment attachment escaped V2: {issues:?}"
    );
}
