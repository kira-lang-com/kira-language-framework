//! Every sparse-capable sampled-texture opcode must return value plus i1 residency.
use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn source(operations: &[(&str, &str)]) -> String {
    let mut text = "module @SparseSamples tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n".to_owned();
    for (index, (_, operation)) in operations.iter().enumerate() {
        text.push_str(&format!(
            "func module @SparseSamples.f{index}(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lod: f32, %dx: vec<2 x f32>, %dy: vec<2 x f32>, %reference: f32) -> i1 target(gpu) {{\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lod: f32, %dx: vec<2 x f32>, %dy: vec<2 x f32>, %reference: f32):\n  %value, %resident = {operation}\n  return %resident\n}}\n\n"
        ));
    }
    text
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("sparse sample source parses");
    let bytes = assemble_text_module(&parsed).expect("sparse sample CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("sparse sample TIRB decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("sparse sample CODE disassembles");
    assert_eq!(
        bytes,
        assemble_text_module(&parse_text_module(&canonical).expect("canonical sparse text parses"))
            .expect("sparse text reassembles")
    );
    canonical
}

#[test]
fn every_sparse_sampling_comparison_gather_and_feedback_opcode_round_trips() {
    let cases = [
        (
            "tex.sample",
            "tex.sample %tex, %smp, %uv [unchecked_uniformity] [sparse]",
        ),
        (
            "tex.sample_level",
            "tex.sample_level %tex, %smp, %uv, lod %lod [sparse]",
        ),
        (
            "tex.sample_grad",
            "tex.sample_grad %tex, %smp, %uv grad %dx, %dy [sparse]",
        ),
        (
            "tex.sample_bias",
            "tex.sample_bias %tex, %smp, %uv, bias %lod [sparse]",
        ),
        (
            "tex.sample_compare",
            "tex.sample_compare %tex, %smp, %uv compare %reference [sparse]",
        ),
        (
            "tex.sample_compare_level",
            "tex.sample_compare_level %tex, %smp, %uv compare %reference, lod %lod [sparse]",
        ),
        (
            "tex.gather",
            "tex.gather %tex, %smp, %uv component 2 [sparse]",
        ),
        (
            "tex.gather_compare",
            "tex.gather_compare %tex, %smp, %uv, %reference [sparse]",
        ),
        ("tex.feedback", "tex.feedback %tex, %smp, %uv [sparse]"),
    ];
    let canonical = round_trip(&source(&cases));
    for (name, _) in &cases {
        assert!(
            canonical.contains(&format!("= {name} %")),
            "sparse {name} missing: {canonical}"
        );
    }
    assert_eq!(canonical.matches("[sparse]").count(), cases.len());
    assert_eq!(canonical.matches("-> i1 target(gpu)").count(), cases.len());
}

#[test]
fn sparse_sampling_preserves_offset_min_lod_and_gather_selector() {
    let cases = [
        (
            "sample",
            "tex.sample %tex, %smp, %uv offset [-8, 7] min_lod %lod [unchecked_uniformity] [sparse]",
        ),
        (
            "grad",
            "tex.sample_grad %tex, %smp, %uv grad %dx, %dy offset [-1, 3] min_lod %lod [sparse]",
        ),
        (
            "gather",
            "tex.gather %tex, %smp, %uv component 3 offset [2, -3] min_lod %lod [sparse]",
        ),
        (
            "compare",
            "tex.sample_compare_level %tex, %smp, %uv compare %reference, lod %lod offset [1, 2] [sparse]",
        ),
        (
            "feedback",
            "tex.feedback %tex, %smp, %uv offset [-3, 2] [unchecked_uniformity] [sparse]",
        ),
    ];
    let canonical = round_trip(&source(&cases));
    assert!(
        canonical.contains("component 3 offset [2, -3] min_lod"),
        "{canonical}"
    );
    assert_eq!(canonical.matches("[sparse]").count(), cases.len());
}

#[test]
fn sparse_sampling_rejects_bad_result_arity_and_unsupported_lod() {
    for operation in [
        "%value = tex.sample %tex, %smp, %uv [sparse]",
        "%value, %res = tex.sample %tex, %smp, %uv",
        "%value, %value = tex.sample %tex, %smp, %uv [sparse]",
        "%value, %res, %extra = tex.sample %tex, %smp, %uv [sparse]",
        "%value, %res = tex.sample %tex, %smp, %uv [sparse] [sparse]",
        "%value, %res = tex.sample %tex, %smp, %uv [sparse] [unchecked_uniformity]",
        "%value, %res = tex.lod %tex, %smp, %uv [sparse]",
        "%value, %res = tex.sample_level %tex, %smp, %uv, lod %lod [unchecked_uniformity] [sparse]",
        "%value, %res = tex.sample %tex, %smp, %missing [sparse]",
    ] {
        let prefix = "func module @SparseSamples.invalid(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lod: f32) -> unit target(gpu) {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lod: f32):\n";
        let full = format!(
            "module @SparseSamples tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{prefix}  {operation}\n  return\n}}\n"
        );
        if let Ok(parsed) = parse_text_module(&full) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid sparse sample accepted: {operation}"
            );
        }
    }
}
