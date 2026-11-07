use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, target: &str) -> String {
    format!(
        "module @Sync tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Sync.run() -> unit {target} {{\nbb0:\n{body}\n  return\n}}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("GPU synchronization source parses");
    let bytes = assemble_text_module(&parsed).expect("GPU synchronization CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("GPU synchronization binary decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("GPU synchronization disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical synchronization parses"),
    )
    .expect("canonical synchronization reassembles");
    assert_eq!(
        bytes, rebuilt,
        "synchronization must round-trip byte-identically"
    );
    canonical
}

#[test]
fn barrier_and_memfence_defaults_encode_full_domain_mask() {
    let source = module("  gpu.barrier\n  gpu.memfence", "target(gpu)");
    let canonical = round_trip(&source);
    assert!(canonical.contains("    gpu.barrier\n"));
    assert!(canonical.contains("    gpu.memfence\n"));
}

#[test]
fn gpu_sync_scope_and_memory_domain_permutations_canonicalize() {
    let source = module(
        "  gpu.memfence [system] [memory: object_payload, device, texture]\n  gpu.barrier [threadgroup] [memory: texture, device]\n  gpu.memfence [thread] [memory: threadgroup]\n  gpu.barrier [subgroup] [memory: device, object_payload]\n  gpu.memfence [device] [memory: device, threadgroup]\n  gpu.memfence [subgroup] [memory: texture]",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("gpu.memfence [system] [memory: device, texture, object_payload]"));
    assert!(canonical.contains("gpu.barrier [threadgroup] [memory: device, texture]"));
    assert!(canonical.contains("gpu.memfence [thread] [memory: threadgroup]"));
    assert!(canonical.contains("gpu.barrier [subgroup] [memory: device, object_payload]"));
    assert!(canonical.contains("gpu.memfence [device] [memory: device, threadgroup]"));
    assert!(canonical.contains("gpu.memfence [subgroup] [memory: texture]"));
}

#[test]
fn unchecked_uniformity_barrier_modifier_round_trips() {
    let source = module(
        "  gpu.barrier [subgroup] [memory: device] [unchecked_uniformity]",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("gpu.barrier [subgroup] [memory: device] [unchecked_uniformity]"));
}

#[test]
fn gpu_sync_rejects_invalid_scopes_clauses_targets_and_result_count() {
    for (operation, target) in [
        ("gpu.barrier", "engine(native)"),
        ("gpu.memfence", "engine(native)"),
        ("gpu.barrier [device]", "target(gpu)"),
        ("gpu.barrier [thread]", "target(gpu)"),
        ("gpu.barrier [system]", "target(gpu)"),
        ("gpu.memfence [nope]", "target(gpu)"),
        ("gpu.barrier [subgroup] [threadgroup]", "target(gpu)"),
        ("gpu.memfence [device] [system]", "target(gpu)"),
        ("gpu.barrier [memory: device, device]", "target(gpu)"),
        (
            "gpu.memfence [memory: device] [memory: texture]",
            "target(gpu)",
        ),
        ("gpu.barrier [memory: ]", "target(gpu)"),
        ("gpu.memfence [memory: device,]", "target(gpu)"),
        ("gpu.memfence [memory: constant]", "target(gpu)"),
        ("gpu.memfence [memory: ray_payload]", "target(gpu)"),
        ("gpu.barrier [memory: node_record]", "target(gpu)"),
        ("gpu.memfence [unchecked_uniformity]", "target(gpu)"),
        (
            "gpu.barrier [unchecked_uniformity] [unchecked_uniformity]",
            "target(gpu)",
        ),
        ("gpu.memfence bogus", "target(gpu)"),
        ("%value = gpu.barrier", "target(gpu)"),
        ("%value = gpu.memfence", "target(gpu)"),
    ] {
        let source = module(&format!("  {operation}"), target);
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid GPU sync encoded: {operation} / {target}"
            );
        }
    }
}

#[test]
fn mixed_gpu_builtin_sync_and_control_flow_round_trip() {
    let source = "module @Sync tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Sync.mixed() -> i32 target(gpu) {\nbb0:\n  %lane = gpu.builtin lane_index : i32\n  gpu.memfence [threadgroup] [memory: device]\n  br bb1(%lane)\nbb1(%forwarded: i32):\n  gpu.barrier [subgroup] [memory: threadgroup]\n  return %forwarded\n}\n";
    let canonical = round_trip(source);
    assert!(canonical.contains("gpu.builtin lane_index : i32"));
    assert!(canonical.contains("gpu.memfence [threadgroup] [memory: device]"));
    assert!(canonical.contains("gpu.barrier [subgroup] [memory: threadgroup]"));
}
