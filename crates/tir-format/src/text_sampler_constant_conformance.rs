use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, target: &str) -> String {
    format!(
        "module @Samplers tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Samplers.test() -> sampler {target} {{\nbb0:\n{body}\n}}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("sampler source parses");
    let bytes = assemble_text_module(&parsed).expect("sampler CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("sampler TIRB decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("sampler disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical sampler text parses"),
    )
    .expect("sampler CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn sampler_defaults_encode_23_byte_descriptor_and_round_trip() {
    let source = module(
        "  %sampler = const.sampler {}\n  return %sampler",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.sampler {}"));
}

#[test]
fn sampler_all_fields_in_unordered_text_canonicalize_and_round_trip() {
    let source = module(
        "  %sampler = const.sampler { lod_bias: -2.0, compare: greater_equal, address_w: mirror_clamp_to_edge, min: linear, reduction: max, border: opaque_white, mag: linear, mip: linear, normalized_coordinates: false, lod_max: 8.0, lod_min: 1.5, max_anisotropy: 16, address_u: repeat, address_v: clamp_to_border }\n  return %sampler",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("min: linear, mag: linear, mip: linear"));
    assert!(canonical.contains("address_u: repeat, address_v: clamp_to_border"));
    assert!(canonical.contains("max_anisotropy: 16"));
    assert!(canonical.contains("normalized_coordinates: false"));
}

#[test]
fn sampler_rejects_cpu_target_and_invalid_fields() {
    for (fields, target) in [
        ("{}", "engine(native)"),
        ("{ min: linear, min: nearest }", "target(gpu)"),
        ("{ mag: cubic }", "target(gpu)"),
        ("{ address_u: outside }", "target(gpu)"),
        ("{ max_anisotropy: 0 }", "target(gpu)"),
        ("{ max_anisotropy: 17 }", "target(gpu)"),
        ("{ normalized_coordinates: yes }", "target(gpu)"),
        ("{ lod_min: 0.1 }", "target(gpu)"),
        ("{ compare: disabled }", "target(gpu)"),
        ("{ missing: linear }", "target(gpu)"),
        ("{ min: linear, }", "target(gpu)"),
        ("{ min: linear,, mag: linear }", "target(gpu)"),
    ] {
        let source = module(
            &format!("  %sampler = const.sampler {fields}\n  return %sampler"),
            target,
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid sampler assembled: {fields} {target}"
            );
        }
    }
}
