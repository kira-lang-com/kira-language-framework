//! V2 text and binary acceptance of a GPU-only inline sampler descriptor.
use super::verify_text_module;
use tir_format::parse_text_module;

#[test]
fn v2_gpu_sampler_descriptor_is_typed_in_code_and_round_trips() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module, disassemble_text_module};

    let source = "module @Samplers tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Samplers.make() -> sampler target(gpu) {\nbb0:\n  %sampler = const.sampler { min: linear, mag: linear, max_anisotropy: 4, lod_min: 1.5, lod_max: 4.0, address_u: repeat }\n  return %sampler\n}\n";
    let parsed = parse_text_module(source).expect("GPU sampler source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "GPU sampler text V2 rejected: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("GPU sampler CODE assembles"))
            .expect("GPU sampler TIRB decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "GPU sampler binary V2 rejected: {binary_errors:?}"
    );
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("GPU sampler disassembles");
    assert!(canonical.contains("const.sampler {"));
}

#[test]
fn cpu_sampler_constant_is_rejected_before_code_encoding() {
    let source = "module @Samplers tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Samplers.bad() -> sampler engine(native) {\nbb0:\n  %sampler = const.sampler {}\n  return %sampler\n}\n";
    let parsed = parse_text_module(source).expect("CPU sampler text parses");
    assert!(tir_format::assemble_text_module(&parsed).is_err());
}
