//! V2/V15 GPU barrier and memory-fence CODE conformance.
use super::verify_text_module;
use tir_format::parse_text_module;

fn module(body: &str, target: &str) -> String {
    format!(
        "module @Sync tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Sync.run() -> unit {target} {{\nbb0:\n{body}\n  return\n}}\n"
    )
}

#[test]
fn v2_v15_gpu_barrier_and_fence_verify_in_text_and_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module, disassemble_text_module};
    let source = module(
        "  gpu.barrier [subgroup] [memory: device, threadgroup]\n  gpu.memfence [system] [memory: device, texture]\n  gpu.memfence [thread] [memory: object_payload]",
        "target(gpu)",
    );
    let parsed = parse_text_module(&source).expect("GPU sync text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "GPU sync text V2/V15 rejected: {text_errors:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("GPU sync CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("GPU sync TIRB decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "GPU sync binary V2 rejected: {binary_errors:?}"
    );
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("GPU sync disassembles");
    assert_eq!(
        bytes,
        assemble_text_module(&parse_text_module(&canonical).expect("canonical GPU sync parses"))
            .expect("canonical GPU sync reassembles")
    );
}

#[test]
fn v2_binary_rejects_cpu_sync_nonzero_results_bad_scope_and_empty_mask() {
    use tir_format::{BinaryModule, BinaryOperand, BinaryTarget, assemble_text_module};
    let source = module("  gpu.barrier [memory: device]", "target(gpu)");
    let parsed = parse_text_module(&source).expect("valid GPU sync text parses");
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("valid GPU sync CODE assembles"),
    )
    .expect("GPU sync TIRB decodes");
    let types = binary.type_table(u64::MAX).expect("TYPES decodes");
    let signatures = binary
        .signature_table(u64::MAX, &types)
        .expect("SIGNATURES decode");
    let mut code = binary
        .code_table(u64::MAX, &signatures)
        .expect("CODE decodes");
    let body = code.functions.first_mut().expect("one function");
    let issues = super::binary_code::gpu_sync::verify(body, Some(BinaryTarget::Cpu), "run");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("target(gpu)")),
        "CPU synchronization was accepted: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .first_mut()
        .expect("sync operation");
    first.result_count = 1;
    let issues = super::binary_code::gpu_sync::verify(body, Some(BinaryTarget::Gpu), "run");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("zero SSA results")),
        "sync with SSA result was accepted: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .first_mut()
        .expect("sync operation");
    first.result_count = 0;
    first.modifiers = 4 << 26;
    let issues = super::binary_code::gpu_sync::verify(body, Some(BinaryTarget::Gpu), "run");
    assert!(
        issues.iter().any(|issue| issue.message.contains("scope")),
        "device-scope execution barrier was accepted: {issues:?}"
    );
    let first = body.blocks[0]
        .instructions
        .as_mut()
        .expect("decoded CODE")
        .first_mut()
        .expect("sync operation");
    first.modifiers = 0;
    first.operands[0] = BinaryOperand::Index(0);
    let issues = super::binary_code::gpu_sync::verify(body, Some(BinaryTarget::Gpu), "run");
    assert!(
        issues.iter().any(|issue| issue.message.contains("mask")),
        "empty GPU memory mask was accepted: {issues:?}"
    );
}

#[test]
fn v15_gpu_sync_text_rejects_duplicate_domains_and_unsupported_scope() {
    use tir_diag::Severity;
    for operation in [
        "gpu.barrier [device] [memory: device]",
        "gpu.memfence [unrecognized] [memory: device]",
        "gpu.barrier [memory: device, device]",
        "gpu.memfence [memory: threadgroup] [memory: device]",
    ] {
        let source = module(&format!("  {operation}"), "target(gpu)");
        let parsed = parse_text_module(&source).expect("invalid GPU sync source parses");
        let errors = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            !errors.is_empty(),
            "V15 accepted invalid GPU synchronization: {operation}"
        );
        assert!(
            tir_format::assemble_text_module(&parsed).is_err(),
            "CODE assembly accepted invalid GPU synchronization: {operation}"
        );
    }
}

#[test]
fn v15_gpu_sync_cfg_block_targets_are_not_nominal_type_references() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = "module @Sync tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Sync.cfg() -> i32 target(gpu) {\nbb0:\n  %lane = gpu.builtin lane_index : i32\n  gpu.barrier [subgroup] [memory: threadgroup]\n  br bb1(%lane)\nbb1(%forwarded: i32):\n  gpu.memfence [device] [memory: device]\n  return %forwarded\n}\n";
    let parsed = parse_text_module(source).expect("GPU sync CFG source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "V15 mistook GPU CFG label for nominal type: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("GPU sync CFG CODE assembles"))
            .expect("GPU sync CFG TIRB decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "GPU sync CFG binary V2 rejected: {binary_errors:?}"
    );
}
