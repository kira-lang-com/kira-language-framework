use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, target: &str) -> String {
    format!("module @Textures tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n").replace("_TARGET_", target)
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("texture source parses");
    let bytes = assemble_text_module(&parsed).expect("texture CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("texture TIRB decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("texture CODE disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical texture source parses"),
    )
    .expect("texture CODE reassembles");
    assert_eq!(bytes, rebuilt, "texture binary round-trip changed bytes");
    canonical
}

#[test]
fn texture_all_seven_query_fields_have_canonical_selector_codes() {
    for field in [
        "width", "height", "depth", "size", "layers", "levels", "samples",
    ] {
        let result = if field == "size" {
            "vec<2 x i32>"
        } else {
            "i32"
        };
        let source = module(
            &format!(
                "func module @Textures.query(%tex: texture<2d, vec<4 x f32>, read>) -> {result} _TARGET_ {{\nbb0(%tex: texture<2d, vec<4 x f32>, read>):\n  %value = tex.query %tex, {field}\n  return %value\n}}"
            ),
            "target(gpu)",
        );
        assert!(round_trip(&source).contains(&format!("tex.query %arg0, {field}")));
    }
}

#[test]
fn texture_query_size_uses_dimension_dependent_scalar_and_vector_types() {
    for (dim, result) in [
        ("1d", "i32"),
        ("1darray", "vec<2 x i32>"),
        ("2d", "vec<2 x i32>"),
        ("cube", "vec<2 x i32>"),
        ("2darray", "vec<3 x i32>"),
        ("3d", "vec<3 x i32>"),
        ("cubearray", "vec<3 x i32>"),
    ] {
        let texture = format!("texture<{dim}, i32, read>");
        let source = module(
            &format!(
                "func module @Textures.query(%tex: {texture}) -> {result} _TARGET_ {{\nbb0(%tex: {texture}):\n  %value = tex.query %tex, size\n  return %value\n}}"
            ),
            "target(gpu)",
        );
        let canonical = round_trip(&source);
        assert!(canonical.contains("tex.query %arg0, size"));
    }
}

#[test]
fn texture_queries_intern_unused_intermediate_scalar_and_vector_results() {
    let source = module(
        "func module @Textures.unused(%tex: texture<3d, vec<4 x f32>, read>) -> unit _TARGET_ {\nbb0(%tex: texture<3d, vec<4 x f32>, read>):\n  %size = tex.query %tex, size\n  %width = tex.query %tex, width\n  return\n}\n\nfunc module @Textures.second(%tex: texture<2d, vec<4 x f32>, read>) -> unit _TARGET_ {\nbb0(%tex: texture<2d, vec<4 x f32>, read>):\n  %size = tex.query %tex, size\n  return\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("tex.query %arg0, width"));
    assert_eq!(canonical.matches("tex.query %arg0, size").count(), 2);
}

#[test]
fn texture_raw_read_and_write_use_typed_texel_coordinates() {
    for (texture, coord, result) in [
        ("texture<1d, i32, read>", "i32", "i32"),
        (
            "texture<2d, vec<4 x f32>, read>",
            "vec<2 x i32>",
            "vec<4 x f32>",
        ),
        ("texture<3d, f32, readwrite>", "vec<3 x i32>", "f32"),
    ] {
        let source = module(
            &format!(
                "func module @Textures.read(%tex: {texture}, %coord: {coord}) -> {result} _TARGET_ {{\nbb0(%tex: {texture}, %coord: {coord}):\n  %value = tex.read %tex, %coord\n  return %value\n}}"
            ),
            "target(gpu)",
        );
        assert!(round_trip(&source).contains("tex.read %arg0, %arg1"));
    }
    let source = module(
        "func module @Textures.write(%tex: texture<2d, i32, write>, %coord: vec<2 x i32>, %data: i32) -> unit _TARGET_ {\nbb0(%tex: texture<2d, i32, write>, %coord: vec<2 x i32>, %data: i32):\n  tex.write %tex, %coord, %data\n  return\n}\n",
        "target(gpu)",
    );
    assert!(round_trip(&source).contains("tex.write %arg0, %arg1, %arg2"));
}

#[test]
fn texture_array_layer_and_multisample_index_round_trip() {
    for (tex, extra, result) in [
        ("texture<2darray, i32, read>", "%layer", "i32"),
        ("texture<2d, i32, read, ms>", "%layer", "i32"),
        ("texture<2darray, i32, read, ms>", "%layer, %sample", "i32"),
    ] {
        let source = module(
            &format!(
                "func module @Textures.read(%tex: {tex}, %coords: vec<2 x i32>, %layer: i32, %sample: i32) -> {result} _TARGET_ {{\nbb0(%tex: {tex}, %coords: vec<2 x i32>, %layer: i32, %sample: i32):\n  %value = tex.read %tex, %coords, {extra}\n  return %value\n}}"
            ),
            "target(gpu)",
        );
        assert!(round_trip(&source).contains("tex.read"));
    }
}

#[test]
fn texture_rejects_unknown_field_wrong_coordinates_access_and_extra_operands() {
    for (ty, body) in [
        ("texture<2d, i32, read>", "%value = tex.query %tex, invalid"),
        (
            "texture<2d, i32, read>",
            "%value = tex.query %tex, size, extra",
        ),
        (
            "texture<2d, i32, read>",
            "%value = tex.query %tex, width [sparse]",
        ),
        ("texture<2d, i32, read>", "%value = tex.read %tex, %layer"),
        ("texture<2d, i32, write>", "%value = tex.read %tex, %coords"),
        ("texture<2d, i32, read>", "tex.write %tex, %coords, %layer"),
        (
            "texture<2d, i32, write>",
            "tex.write %tex, %coords, %coords",
        ),
        (
            "texture<2d, i32, read>",
            "%value = tex.read %tex, %coords, %layer",
        ),
        ("texture<2d, i32, read>", "tex.read %tex, %coords"),
        (
            "texture<2d, i32, write>",
            "%value = tex.write %tex, %coords, %layer",
        ),
    ] {
        let source = module(
            &format!(
                "func module @Textures.bad(%tex: {ty}, %coords: vec<2 x i32>, %layer: i32) -> unit _TARGET_ {{\nbb0(%tex: {ty}, %coords: vec<2 x i32>, %layer: i32):\n  {body}\n  return\n}}"
            ),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid texture CODE accepted: {body}"
            );
        }
    }
}

#[test]
fn texture_query_read_write_are_gpu_only() {
    for body in [
        "%v = tex.query %tex, width",
        "%v = tex.read %tex, %coords",
        "tex.write %tex, %coords, %data",
    ] {
        let source = module(
            &format!(
                "func module @Textures.cpu(%tex: texture<2d, i32, readwrite>, %coords: vec<2 x i32>, %data: i32) -> unit _TARGET_ {{\nbb0(%tex: texture<2d, i32, readwrite>, %coords: vec<2 x i32>, %data: i32):\n  {body}\n  return\n}}"
            ),
            "engine(native)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "CPU texture CODE accepted: {body}"
            );
        }
    }
}

#[test]
fn sparse_texture_read_returns_value_and_i1_residency_across_cfg() {
    let source = module(
        "func module @Textures.sparse(%tex: texture<2d, vec<4 x f32>, read>, %coords: vec<2 x i32>) -> i1 _TARGET_ {\nbb0(%tex: texture<2d, vec<4 x f32>, read>, %coords: vec<2 x i32>):\n  %value, %resident = tex.read %tex, %coords [sparse]\n  br bb1(%value, %resident)\nbb1(%pixels: vec<4 x f32>, %seen: i1):\n  return %seen\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(
        canonical.contains("= tex.read %arg0, %arg1 [sparse]"),
        "{canonical}"
    );
    assert!(canonical.contains("%b1arg1: i1"), "{canonical}");
}

#[test]
fn sparse_texture_read_interns_residency_even_if_unused_and_handles_sample_index() {
    for (ty, extra) in [
        ("texture<2d, i32, read>", ""),
        ("texture<2darray, i32, read>", ", %layer"),
        ("texture<2d, i32, read, ms>", ", %sample"),
        ("texture<2darray, i32, read, ms>", ", %layer, %sample"),
    ] {
        let source = module(
            &format!(
                "func module @Textures.sparse_unused(%tex: {ty}, %coords: vec<2 x i32>, %layer: i32, %sample: i32) -> i32 _TARGET_ {{\nbb0(%tex: {ty}, %coords: vec<2 x i32>, %layer: i32, %sample: i32):\n  %value, %resident = tex.read %tex, %coords{extra} [sparse]\n  return %value\n}}"
            ),
            "target(gpu)",
        );
        assert!(round_trip(&source).contains("[sparse]"));
    }
}

#[test]
fn sparse_texture_read_requires_two_distinct_ssa_results_and_trailing_flag() {
    for operation in [
        "%value = tex.read %tex, %coords [sparse]",
        "%value, %res = tex.read %tex, %coords",
        "%value, %value = tex.read %tex, %coords [sparse]",
        "%value, %res = tex.read %tex, %coords [sparse] [sparse]",
        "%value, %res = tex.read %tex, %coords [invalid]",
        "%value, %res, %other = tex.read %tex, %coords [sparse]",
        "%value, %res = tex.read %tex, %missing [sparse]",
        "%value, %res = tex.query %tex, width [sparse]",
        "%value, %res = tex.write %tex, %coords [sparse]",
    ] {
        let source = module(
            &format!(
                "func module @Textures.invalid_sparse(%tex: texture<2d, i32, readwrite>, %coords: vec<2 x i32>) -> unit _TARGET_ {{\nbb0(%tex: texture<2d, i32, readwrite>, %coords: vec<2 x i32>):\n  {operation}\n  return\n}}"
            ),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid sparse texture text accepted: {operation}"
            );
        }
    }
}
