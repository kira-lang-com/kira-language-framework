use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, target: &str) -> String {
    format!("module @TextureSamples tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n")
        .replace("_TARGET_", target)
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("texture sample source parses");
    let bytes = assemble_text_module(&parsed).expect("texture sample CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("texture sample TIRB decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("texture sample CODE disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical texture sample parses"),
    )
    .unwrap_or_else(|issue| {
        panic!("texture sample reassembles: {issue:?}\nCanonical text:\n{canonical}")
    });
    assert_eq!(bytes, rebuilt, "texture sample binary round-trip differs");
    canonical
}

#[test]
fn texture_sample_and_implicit_lod_round_trip_with_f32_coords() {
    let source = module(
        "func module @TextureSamples.sample(%tex: texture<2d, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>) -> vec<4 x f32> _TARGET_ {\nbb0(%tex: texture<2d, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>):\n  %color = tex.sample %tex, %smp, %uv\n  return %color\n}\n\nfunc module @TextureSamples.lod(%tex: texture<2d, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>) -> vec<2 x f32> _TARGET_ {\nbb0(%tex: texture<2d, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>):\n  %level = tex.lod %tex, %smp, %uv\n  return %level\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("tex.sample %arg0, %arg1, %arg2"));
    assert!(canonical.contains("tex.lod %arg0, %arg1, %arg2"));
}

#[test]
fn texture_sample_integer_offsets_and_min_lod_round_trip() {
    for (offset, expected) in [
        ("[-8, 7]", "[-8, 7]"),
        ("[+0, -1]", "[0, -1]"),
        ("[0x7, -0x8]", "[7, -8]"),
    ] {
        let source = module(
            &format!(
                "func module @TextureSamples.with_offsets(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lower: f32) -> f32 _TARGET_ {{\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lower: f32):\n  %color = tex.sample %tex, %smp, %uv offset {offset} min_lod %lower [unchecked_uniformity]\n  return %color\n}}"
            ),
            "target(gpu)",
        );
        let canonical = round_trip(&source);
        assert!(
            canonical.contains(&format!(
                "offset {expected} min_lod %arg3 [unchecked_uniformity]"
            )),
            "{canonical}"
        );
    }
}

#[test]
fn texture_sample_accepts_offset_only_and_min_lod_only() {
    for suffix in [
        " offset [1, -2]",
        " min_lod %lower",
        " [unchecked_uniformity]",
    ] {
        let source = module(
            &format!(
                "func module @TextureSamples.options(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lower: f32) -> f32 _TARGET_ {{\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lower: f32):\n  %result = tex.sample %tex, %smp, %uv{suffix}\n  return %result\n}}"
            ),
            "target(gpu)",
        );
        let canonical = round_trip(&source);
        assert!(canonical.contains("tex.sample %arg0, %arg1, %arg2"));
    }
}

#[test]
fn texture_sampling_cube_and_array_coordinates_are_typed() {
    for (dim, coords, extras, offset) in [
        ("1d", "f32", "", "offset [-8]"),
        ("1darray", "f32", ", %layer", "offset [7]"),
        ("2darray", "vec<2 x f32>", ", %layer", "offset [-1, 2]"),
        ("3d", "vec<3 x f32>", "", "offset [-1, 0, 1]"),
        ("cube", "vec<3 x f32>", "", "offset [2, -3]"),
        ("cubearray", "vec<3 x f32>", ", %layer", "offset [1, -1]"),
    ] {
        let texture = format!("texture<{dim}, f32, sample>");
        let source = module(
            &format!(
                "func module @TextureSamples.dims(%tex: {texture}, %smp: sampler, %uv: {coords}, %layer: i32) -> f32 _TARGET_ {{\nbb0(%tex: {texture}, %smp: sampler, %uv: {coords}, %layer: i32):\n  %result = tex.sample %tex, %smp, %uv{extras} {offset}\n  return %result\n}}"
            ),
            "target(gpu)",
        );
        assert!(round_trip(&source).contains("tex.sample"));
    }
}

#[test]
fn texture_lod_interns_unused_intermediate_result_types() {
    let source = module(
        "func module @TextureSamples.unused(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>) -> unit _TARGET_ {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>):\n  %level = tex.lod %tex, %smp, %uv [unchecked_uniformity]\n  %color = tex.sample %tex, %smp, %uv\n  return\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("tex.lod %arg0, %arg1, %arg2 [unchecked_uniformity]"));
}

#[test]
fn texture_sampling_rejects_wrong_types_offsets_and_modes() {
    for (ty, coords, operation) in [
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.sample %tex, %smp, %uv offset [0]",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.sample %tex, %smp, %uv offset [8, 0]",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.sample %tex, %smp, %uv offset [-9, 0]",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.sample %tex, %smp, %uv offset [0, 1, 2]",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.sample %tex, %smp, %uv min_lod %layer",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.sample %tex, %smp, %uv [sparse]",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.sample %tex, %smp, %uv min_lod %min min_lod %min",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.sample %tex, %smp, %uv offset [1, 2] offset [0, 0]",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.lod %tex, %smp, %uv offset [0, 0]",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.lod %tex, %smp, %uv min_lod %min",
        ),
        (
            "texture<2d, f32, read>",
            "vec<2 x f32>",
            "tex.sample %tex, %smp, %uv",
        ),
        (
            "texture<2d, f32, write>",
            "vec<2 x f32>",
            "tex.lod %tex, %smp, %uv",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<3 x f32>",
            "tex.sample %tex, %smp, %uv",
        ),
        (
            "texture<2d, f32, sample, ms>",
            "vec<2 x f32>",
            "tex.sample %tex, %smp, %uv",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x i32>",
            "tex.lod %tex, %smp, %uv",
        ),
    ] {
        let source = module(
            &format!(
                "func module @TextureSamples.bad(%tex: {ty}, %smp: sampler, %uv: {coords}, %layer: i32, %min: f32) -> unit _TARGET_ {{\nbb0(%tex: {ty}, %smp: sampler, %uv: {coords}, %layer: i32, %min: f32):\n  %result = {operation}\n  return\n}}"
            ),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid sampler operation encoded: {operation}"
            );
        }
    }
}

#[test]
fn texture_sampling_is_gpu_only() {
    for operation in ["tex.sample %tex, %smp, %uv", "tex.lod %tex, %smp, %uv"] {
        let source = module(
            &format!(
                "func module @TextureSamples.cpu(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>) -> unit _TARGET_ {{\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>):\n  %result = {operation}\n  return\n}}"
            ),
            "engine(native)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "CPU texture sampling encoded: {operation}"
            );
        }
    }
}

#[test]
fn explicit_level_and_bias_sampling_round_trip_with_optional_offsets_and_min_lod() {
    let source = module(
        "func module @TextureSamples.explicit(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %level: f32, %lower: f32) -> f32 _TARGET_ {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %level: f32, %lower: f32):\n  %a = tex.sample_level %tex, %smp, %uv, lod %level offset [-8, 7] min_lod %lower\n  %b = tex.sample_bias %tex, %smp, %uv, bias %level offset [1, -3] min_lod %lower [unchecked_uniformity]\n  return %b\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(
        canonical.contains(
            "tex.sample_level %arg0, %arg1, %arg2, lod %arg3 offset [-8, 7] min_lod %arg4"
        )
    );
    assert!(canonical.contains("tex.sample_bias %arg0, %arg1, %arg2, bias %arg3 offset [1, -3] min_lod %arg4 [unchecked_uniformity]"));
}

#[test]
fn sample_level_and_bias_accept_minimal_forms_and_array_layers() {
    for (opcode, extra) in [("tex.sample_level", "lod"), ("tex.sample_bias", "bias")] {
        let tex = "texture<cube, f32, sample>";
        let source = module(
            &format!(
                "func module @TextureSamples.minimal(%tex: {tex}, %smp: sampler, %uv: vec<3 x f32>, %level: f32) -> f32 _TARGET_ {{\nbb0(%tex: {tex}, %smp: sampler, %uv: vec<3 x f32>, %level: f32):\n  %result = {opcode} %tex, %smp, %uv, {extra} %level\n  return %result\n}}"
            ),
            "target(gpu)",
        );
        assert!(round_trip(&source).contains(opcode));
        let tex = "texture<2darray, f32, sample>";
        let source = module(
            &format!(
                "func module @TextureSamples.array(%tex: {tex}, %smp: sampler, %uv: vec<2 x f32>, %layer: i32, %level: f32) -> f32 _TARGET_ {{\nbb0(%tex: {tex}, %smp: sampler, %uv: vec<2 x f32>, %layer: i32, %level: f32):\n  %result = {opcode} %tex, %smp, %uv, %layer, {extra} %level offset [-1, 2]\n  return %result\n}}"
            ),
            "target(gpu)",
        );
        assert!(round_trip(&source).contains("offset [-1, 2]"));
    }
}

#[test]
fn sample_level_and_bias_reject_wrong_lod_bias_types_and_modifiers() {
    for operation in [
        "tex.sample_level %tex, %smp, %uv",
        "tex.sample_bias %tex, %smp, %uv",
        "tex.sample_level %tex, %smp, %uv, lod %layer",
        "tex.sample_bias %tex, %smp, %uv, bias %layer",
        "tex.sample_level %tex, %smp, %uv, bias %level",
        "tex.sample_bias %tex, %smp, %uv, lod %level",
        "tex.sample_level %tex, %smp, %uv, lod %level [unchecked_uniformity]",
        "tex.sample_level %tex, %smp, %uv, lod %level [sparse]",
        "tex.sample_bias %tex, %smp, %uv, bias %level [sparse]",
        "tex.sample_bias %tex, %smp, %uv, bias %level offset [1]",
        "tex.sample_level %tex, %smp, %uv, lod %level min_lod %layer",
        "tex.sample_bias %tex, %smp, %uv, bias %level min_lod %layer",
        "tex.sample_level %tex, %smp, %uv, lod %missing",
        "tex.sample_bias %tex, %smp, %uv, bias %missing",
    ] {
        let source = module(
            &format!(
                "func module @TextureSamples.invalid_modes(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %level: f32, %layer: i32) -> unit _TARGET_ {{\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %level: f32, %layer: i32):\n  %result = {operation}\n  return\n}}"
            ),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid explicit sampling CODE accepted: {operation}"
            );
        }
    }
}

#[test]
fn texture_sample_grad_round_trips_two_typed_gradients_offsets_and_min_lod() {
    for (dim, coordinates, offsets) in [
        ("1d", "f32", "[-8]"),
        ("2d", "vec<2 x f32>", "[-8, 7]"),
        ("3d", "vec<3 x f32>", "[1, -2, 3]"),
        ("cube", "vec<3 x f32>", "[1, -2]"),
    ] {
        let source = module(
            &format!(
                "func module @TextureSamples.grad(%tex: texture<{dim}, f32, sample>, %smp: sampler, %uv: {coordinates}, %dx: {coordinates}, %dy: {coordinates}, %lower: f32) -> f32 _TARGET_ {{\nbb0(%tex: texture<{dim}, f32, sample>, %smp: sampler, %uv: {coordinates}, %dx: {coordinates}, %dy: {coordinates}, %lower: f32):\n  %result = tex.sample_grad %tex, %smp, %uv grad %dx, %dy offset {offsets} min_lod %lower\n  return %result\n}}"
            ),
            "target(gpu)",
        );
        let canonical = round_trip(&source);
        assert!(canonical.contains(&format!("tex.sample_grad %arg0, %arg1, %arg2 grad %arg3, %arg4 offset {offsets} min_lod %arg5")), "{canonical}");
    }
}

#[test]
fn texture_sample_grad_round_trips_minimal_and_array_forms() {
    let tex = "texture<2darray, f32, sample>";
    let source = module(
        &format!(
            "func module @TextureSamples.gradient_array(%tex: {tex}, %smp: sampler, %uv: vec<2 x f32>, %layer: i32, %dx: vec<2 x f32>, %dy: vec<2 x f32>) -> f32 _TARGET_ {{\nbb0(%tex: {tex}, %smp: sampler, %uv: vec<2 x f32>, %layer: i32, %dx: vec<2 x f32>, %dy: vec<2 x f32>):\n  %value = tex.sample_grad %tex, %smp, %uv, %layer grad %dx, %dy\n  return %value\n}}"
        ),
        "target(gpu)",
    );
    assert!(
        round_trip(&source)
            .contains("tex.sample_grad %arg0, %arg1, %arg2, %arg3 grad %arg4, %arg5")
    );
}

#[test]
fn texture_sample_grad_rejects_wrong_gradient_shapes_and_missing_operands() {
    for operation in [
        "tex.sample_grad %tex, %smp, %uv",
        "tex.sample_grad %tex, %smp, %uv grad %dx",
        "tex.sample_grad %tex, %smp, %uv grad %dx, %missing",
        "tex.sample_grad %tex, %smp, %uv grad %dx, %wrong",
        "tex.sample_grad %tex, %smp, %uv grad %wrong, %dy",
        "tex.sample_grad %tex, %smp, %uv grad %dx, %dy offset [-8]",
        "tex.sample_grad %tex, %smp, %uv grad %dx, %dy [unchecked_uniformity]",
        "tex.sample_grad %tex, %smp, %uv grad %dx, %dy min_lod %layer",
        "tex.sample_grad %tex, %smp, %uv grad %dx, %dy [sparse]",
        "tex.sample_grad %tex, %smp, %uv, %dx, %dy",
    ] {
        let source = module(
            &format!(
                "func module @TextureSamples.invalid_grad(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %dx: vec<2 x f32>, %dy: vec<2 x f32>, %wrong: f32, %layer: i32) -> unit _TARGET_ {{\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %dx: vec<2 x f32>, %dy: vec<2 x f32>, %wrong: f32, %layer: i32):\n  %result = {operation}\n  return\n}}"
            ),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid gradient texture CODE accepted: {operation}"
            );
        }
    }
}

#[test]
fn comparison_sampling_round_trips_f32_reference_and_explicit_lod() {
    let source = module(
        "func module @TextureSamples.compare(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %ref: f32, %level: f32, %lower: f32) -> f32 _TARGET_ {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %ref: f32, %level: f32, %lower: f32):\n  %a = tex.sample_compare %tex, %smp, %uv compare %ref offset [-8, 7] min_lod %lower [unchecked_uniformity]\n  %b = tex.sample_compare_level %tex, %smp, %uv compare %ref, lod %level offset [2, -3] min_lod %lower\n  return %b\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("tex.sample_compare %arg0, %arg1, %arg2 compare %arg3 offset [-8, 7] min_lod %arg5 [unchecked_uniformity]"));
    assert!(canonical.contains("tex.sample_compare_level %arg0, %arg1, %arg2 compare %arg3, lod %arg4 offset [2, -3] min_lod %arg5"));
}

#[test]
fn comparison_sampling_minimal_and_array_forms_round_trip() {
    for (dim, coord, extra) in [
        ("1d", "f32", ""),
        ("2d", "vec<2 x f32>", ""),
        ("2darray", "vec<2 x f32>", ", %layer"),
        ("cube", "vec<3 x f32>", ""),
        ("cubearray", "vec<3 x f32>", ", %layer"),
    ] {
        let tex = format!("texture<{dim}, f32, sample>");
        let source = module(
            &format!(
                "func module @TextureSamples.compare_dims(%tex: {tex}, %smp: sampler, %uv: {coord}, %layer: i32, %ref: f32, %level: f32) -> f32 _TARGET_ {{\nbb0(%tex: {tex}, %smp: sampler, %uv: {coord}, %layer: i32, %ref: f32, %level: f32):\n  %a = tex.sample_compare %tex, %smp, %uv{extra} compare %ref\n  %b = tex.sample_compare_level %tex, %smp, %uv{extra} compare %ref, lod %level\n  return %b\n}}"
            ),
            "target(gpu)",
        );
        let canonical = round_trip(&source);
        assert!(canonical.contains("tex.sample_compare %arg0"));
        assert!(canonical.contains("tex.sample_compare_level %arg0"));
    }
}

#[test]
fn comparison_sampling_rejects_non_f32_texture_reference_or_lod() {
    for (texture, reference, operation) in [
        (
            "texture<2d, i32, sample>",
            "f32",
            "tex.sample_compare %tex, %smp, %uv compare %ref",
        ),
        (
            "texture<2d, f32, sample>",
            "i32",
            "tex.sample_compare %tex, %smp, %uv compare %ref",
        ),
        (
            "texture<2d, f32, sample>",
            "i32",
            "tex.sample_compare_level %tex, %smp, %uv compare %ref, lod %level",
        ),
        (
            "texture<2d, f32, sample>",
            "f32",
            "tex.sample_compare_level %tex, %smp, %uv compare %ref, lod %layer",
        ),
        (
            "texture<2d, f32, sample>",
            "f32",
            "tex.sample_compare_level %tex, %smp, %uv compare %ref",
        ),
        (
            "texture<2d, f32, sample>",
            "f32",
            "tex.sample_compare %tex, %smp, %uv",
        ),
        (
            "texture<2d, f32, sample>",
            "f32",
            "tex.sample_compare_level %tex, %smp, %uv compare %ref, lod %level [unchecked_uniformity]",
        ),
        (
            "texture<2d, f32, sample>",
            "f32",
            "tex.sample_compare %tex, %smp, %uv compare %ref [sparse]",
        ),
        (
            "texture<2d, f32, sample>",
            "f32",
            "tex.sample_compare_level %tex, %smp, %uv compare %ref, lod %level offset [1]",
        ),
        (
            "texture<2d, f32, sample>",
            "f32",
            "tex.sample_compare_level %tex, %smp, %uv compare %ref, lod %missing",
        ),
    ] {
        let source = module(
            &format!(
                "func module @TextureSamples.invalid_compare(%tex: {texture}, %smp: sampler, %uv: vec<2 x f32>, %ref: {reference}, %level: f32, %layer: i32) -> unit _TARGET_ {{\nbb0(%tex: {texture}, %smp: sampler, %uv: vec<2 x f32>, %ref: {reference}, %level: f32, %layer: i32):\n  %result = {operation}\n  return\n}}"
            ),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid comparison sampling CODE accepted: {operation}"
            );
        }
    }
}

#[test]
fn texture_gather_all_four_component_selectors_round_trip() {
    for selector in 0..=3 {
        let component = if selector == 0 {
            String::new()
        } else {
            format!(" component {selector}")
        };
        let source = module(
            &format!(
                "func module @TextureSamples.gather(%tex: texture<2d, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>) -> vec<4 x f32> _TARGET_ {{\nbb0(%tex: texture<2d, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>):\n  %value = tex.gather %tex, %smp, %uv{component}\n  return %value\n}}"
            ),
            "target(gpu)",
        );
        let canonical = round_trip(&source);
        assert!(
            canonical.contains(&format!("tex.gather %arg0, %arg1, %arg2{component}")),
            "{canonical}"
        );
    }
}

#[test]
fn texture_gather_scalar_and_intermediate_vector_result_types_round_trip() {
    let source = module(
        "func module @TextureSamples.gather_int(%tex: texture<2d, i32, sample>, %smp: sampler, %uv: vec<2 x f32>) -> unit _TARGET_ {\nbb0(%tex: texture<2d, i32, sample>, %smp: sampler, %uv: vec<2 x f32>):\n  %value = tex.gather %tex, %smp, %uv component 2 offset [-8, 7]\n  return\n}\n\nfunc module @TextureSamples.gather_float(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>) -> vec<4 x f32> _TARGET_ {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>):\n  %value = tex.gather %tex, %smp, %uv component 3\n  return %value\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("tex.gather %arg0, %arg1, %arg2 component 2 offset [-8, 7]"));
    assert!(canonical.contains("tex.gather %arg0, %arg1, %arg2 component 3"));
}

#[test]
fn texture_gather_compare_f32_reference_and_array_layers_round_trip() {
    let source = module(
        "func module @TextureSamples.compare_gather(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %ref: f32, %min: f32) -> vec<4 x f32> _TARGET_ {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %ref: f32, %min: f32):\n  %value = tex.gather_compare %tex, %smp, %uv, %ref offset [1, -2] min_lod %min [unchecked_uniformity]\n  return %value\n}\n\nfunc module @TextureSamples.compare_gather_array(%tex: texture<2darray, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %layer: i32, %ref: f32) -> vec<4 x f32> _TARGET_ {\nbb0(%tex: texture<2darray, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %layer: i32, %ref: f32):\n  %value = tex.gather_compare %tex, %smp, %uv, %layer, %ref\n  return %value\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("tex.gather_compare %arg0, %arg1, %arg2, %arg3 offset [1, -2] min_lod %arg4 [unchecked_uniformity]"));
    assert!(canonical.contains("tex.gather_compare %arg0, %arg1, %arg2, %arg3, %arg4"));
}

#[test]
fn texture_gather_rejects_invalid_selector_or_comparison_reference() {
    for (component, reference, operation) in [
        ("f32", "f32", "tex.gather %tex, %smp, %uv component 4"),
        ("f32", "f32", "tex.gather %tex, %smp, %uv component -1"),
        ("f32", "f32", "tex.gather %tex, %smp, %uv component banana"),
        (
            "f32",
            "f32",
            "tex.gather %tex, %smp, %uv component 1 component 2",
        ),
        ("f32", "f32", "tex.gather %tex, %smp, %uv offset [1]"),
        (
            "f32",
            "f32",
            "tex.gather %tex, %smp, %uv component 2 [sparse]",
        ),
        ("i32", "f32", "tex.gather_compare %tex, %smp, %uv, %ref"),
        ("f32", "i32", "tex.gather_compare %tex, %smp, %uv, %ref"),
        ("f32", "f32", "tex.gather_compare %tex, %smp, %uv"),
        (
            "f32",
            "f32",
            "tex.gather_compare %tex, %smp, %uv, %ref component 2",
        ),
        (
            "f32",
            "f32",
            "tex.gather_compare %tex, %smp, %uv, %ref, %ref",
        ),
    ] {
        let texture = format!("texture<2d, {component}, sample>");
        let source = module(
            &format!(
                "func module @TextureSamples.bad_gather(%tex: {texture}, %smp: sampler, %uv: vec<2 x f32>, %ref: {reference}) -> unit _TARGET_ {{\nbb0(%tex: {texture}, %smp: sampler, %uv: vec<2 x f32>, %ref: {reference}):\n  %value = {operation}\n  return\n}}"
            ),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid gather CODE accepted: {operation}"
            );
        }
    }
}

#[test]
fn sampler_feedback_round_trips_offset_min_lod_and_array_layers() {
    let source = module(
        "func module @TextureSamples.feedback(%tex: texture<2d, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>, %min: f32) -> vec<4 x f32> _TARGET_ {\nbb0(%tex: texture<2d, vec<4 x f32>, sample>, %smp: sampler, %uv: vec<2 x f32>, %min: f32):\n  %result = tex.feedback %tex, %smp, %uv offset [-8, 7] min_lod %min [unchecked_uniformity]\n  return %result\n}\n\nfunc module @TextureSamples.array_feedback(%tex: texture<2darray, i32, sample>, %smp: sampler, %uv: vec<2 x f32>, %layer: i32) -> i32 _TARGET_ {\nbb0(%tex: texture<2darray, i32, sample>, %smp: sampler, %uv: vec<2 x f32>, %layer: i32):\n  %result = tex.feedback %tex, %smp, %uv, %layer\n  return %result\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains(
        "tex.feedback %arg0, %arg1, %arg2 offset [-8, 7] min_lod %arg3 [unchecked_uniformity]"
    ));
    assert!(canonical.contains("tex.feedback %arg0, %arg1, %arg2, %arg3"));
}

#[test]
fn sampler_feedback_rejects_malformed_shapes_sparse_and_cpu() {
    for (texture, coordinates, operation, target) in [
        (
            "texture<2d, f32, read>",
            "vec<2 x f32>",
            "tex.feedback %tex, %smp, %uv",
            "target(gpu)",
        ),
        (
            "texture<2d, f32, sample, ms>",
            "vec<2 x f32>",
            "tex.feedback %tex, %smp, %uv",
            "target(gpu)",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<3 x f32>",
            "tex.feedback %tex, %smp, %uv",
            "target(gpu)",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.feedback %tex, %smp, %uv offset [-9, 7]",
            "target(gpu)",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.feedback %tex, %smp, %uv offset [1]",
            "target(gpu)",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.feedback %tex, %smp, %uv min_lod %layer",
            "target(gpu)",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.feedback %tex, %smp, %uv [sparse]",
            "target(gpu)",
        ),
        (
            "texture<2d, f32, sample>",
            "vec<2 x f32>",
            "tex.feedback %tex, %smp, %uv",
            "engine(native)",
        ),
    ] {
        let source = module(
            &format!(
                "func module @TextureSamples.bad_feedback(%tex: {texture}, %smp: sampler, %uv: {coordinates}, %layer: i32) -> unit _TARGET_ {{\nbb0(%tex: {texture}, %smp: sampler, %uv: {coordinates}, %layer: i32):\n  %result = {operation}\n  return\n}}"
            ),
            target,
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid sampler feedback encoded: {operation} ({target})"
            );
        }
    }
}
