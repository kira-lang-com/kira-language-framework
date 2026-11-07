use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @VectorGather tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}
fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("gather/scatter text parses");
    let bytes = assemble_text_module(&parsed).expect("gather/scatter CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("gather/scatter binary decodes");
    let text =
        disassemble_text_module(&binary, u64::MAX).expect("gather/scatter CODE disassembles");
    let rebuilt = assemble_text_module(&parse_text_module(&text).expect("canonical text parses"))
        .expect("gather/scatter CODE reassembles");
    assert_eq!(bytes, rebuilt);
    text
}

#[test]
fn pointer_vector_gather_and_scatter_round_trip() {
    let source = module(
        "func module @VectorGather.copy(%ptrs: vec<4 x ptr>) -> vec<4 x i32> engine(native) {\nbb0(%ptrs: vec<4 x ptr>):\n  %result = vec.gather vec<4 x i32>, %ptrs\n  vec.scatter %result, %ptrs\n  return %result\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("vec.gather vec<4 x i32>, %arg0"));
    assert!(canonical.contains("vec.scatter %v1, %arg0"));
}

#[test]
fn masked_pointer_vector_gather_and_scatter_round_trip() {
    let source = module(
        "func module @VectorGather.masked(%ptrs: vec<4 x ptr>, %mask: vec<4 x i1>, %old: vec<4 x f32>) -> vec<4 x f32> engine(native) {\nbb0(%ptrs: vec<4 x ptr>, %mask: vec<4 x i1>, %old: vec<4 x f32>):\n  %result = vec.gather vec<4 x f32>, %ptrs [mask %mask, passthru %old]\n  vec.scatter %result, %ptrs [mask %mask]\n  return %result\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("vec.gather vec<4 x f32>, %arg0 [mask %arg1, passthru %arg2]"));
    assert!(canonical.contains("vec.scatter %v3, %arg0 [mask %arg1]"));
}

#[test]
fn base_scaled_offsets_gather_and_scatter_round_trip() {
    let source = module(
        "func module @VectorGather.base(%base: ptr, %offsets: vec<4 x i32>, %mask: vec<4 x i1>, %old: vec<4 x i64>) -> vec<4 x i64> engine(native) {\nbb0(%base: ptr, %offsets: vec<4 x i32>, %mask: vec<4 x i1>, %old: vec<4 x i64>):\n  %result = vec.gather vec<4 x i64>, %base, %offsets, 8 [mask %mask, passthru %old]\n  vec.scatter %result, %base, %offsets, 8 [mask %mask]\n  return %result\n}",
    );
    let canonical = round_trip(&source);
    assert!(
        canonical.contains("vec.gather vec<4 x i64>, %arg0, %arg1, 8 [mask %arg2, passthru %arg3]")
    );
    assert!(canonical.contains("vec.scatter %v4, %arg0, %arg1, 8 [mask %arg2]"));
}

#[test]
fn unmasked_scaled_offsets_round_trip_across_block_edge() {
    let source = module(
        "func module @VectorGather.edge(%base: ptr, %offsets: vec<2 x i32>) -> vec<2 x i16> engine(native) {\nbb0(%base: ptr, %offsets: vec<2 x i32>):\n  %found = vec.gather vec<2 x i16>, %base, %offsets, 2\n  br bb1(%found)\nbb1(%value: vec<2 x i16>):\n  vec.scatter %value, %base, %offsets, 2\n  return %value\n}",
    );
    let text = round_trip(&source);
    assert!(text.contains("vec.gather vec<2 x i16>"));
    assert!(text.contains("vec.scatter"));
}

#[test]
fn gather_and_scatter_reject_incompatible_addresses_and_masks() {
    for (params, instruction) in [
        (
            "%ptrs: vec<2 x ptr>",
            "%out = vec.gather vec<4 x i32>, %ptrs",
        ),
        (
            "%ptrs: vec<4 x i64>",
            "%out = vec.gather vec<4 x i32>, %ptrs",
        ),
        (
            "%ptrs: vec<4 x ptr>, %mask: vec<2 x i1>, %old: vec<4 x i32>",
            "%out = vec.gather vec<4 x i32>, %ptrs [mask %mask, passthru %old]",
        ),
        (
            "%ptrs: vec<4 x ptr>, %mask: vec<4 x i1>",
            "%out = vec.gather vec<4 x i32>, %ptrs [mask %mask]",
        ),
        ("%ptrs: vec<4 x ptr>", "vec.scatter %ptrs, %ptrs"),
        (
            "%base: ptr, %offsets: vec<2 x i32>",
            "%out = vec.gather vec<4 x i32>, %base, %offsets, 4",
        ),
        (
            "%base: ptr, %offsets: vec<4 x f32>",
            "%out = vec.gather vec<4 x i32>, %base, %offsets, 4",
        ),
        (
            "%base: ptr, %offsets: vec<4 x i32>",
            "%out = vec.gather vec<4 x i32>, %base, %offsets, -1",
        ),
        (
            "%base: ptr, %offsets: vec<4 x i32>",
            "vec.scatter %offsets, %base, %offsets, 4 [mask %offsets]",
        ),
        (
            "%ptrs: vec<4 x ptr>",
            "vec.scatter %ptrs, %ptrs [mask %missing]",
        ),
    ] {
        let body = format!(
            "func module @VectorGather.invalid({params}) -> unit engine(native) {{\nbb0({params}):\n  {instruction}\n  return\n}}"
        );
        let source = module(&body);
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid gather/scatter assembled: {instruction}"
            );
        }
    }
}
