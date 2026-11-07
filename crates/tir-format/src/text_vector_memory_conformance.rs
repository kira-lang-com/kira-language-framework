use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @VectorMemory tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("vector memory text parses");
    let bytes = assemble_text_module(&parsed).expect("vector memory CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("vector memory binary decodes");
    let text = disassemble_text_module(&binary, u64::MAX).expect("vector memory disassembles");
    let rebuilt = assemble_text_module(&parse_text_module(&text).expect("canonical text parses"))
        .expect("vector memory CODE reassembles");
    assert_eq!(bytes, rebuilt);
    text
}

#[test]
fn unmasked_vector_load_and_store_round_trip() {
    let source = module(
        "func module @VectorMemory.copy(%source: ptr, %dest: ptr) -> vec<4 x i32> engine(native) {\nbb0(%source: ptr, %dest: ptr):\n  %v = vec.load vec<4 x i32>, %source [align 16]\n  vec.store %v, %dest [align 16]\n  return %v\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("vec.load vec<4 x i32>, %arg0 [align 16]"));
    assert!(canonical.contains("vec.store %v2, %arg1 [align 16]"));
}

#[test]
fn masked_vector_load_passthru_and_store_round_trip() {
    let source = module(
        "func module @VectorMemory.masked(%ptr: ptr, %mask: vec<4 x i1>, %old: vec<4 x f32>) -> vec<4 x f32> engine(native) {\nbb0(%ptr: ptr, %mask: vec<4 x i1>, %old: vec<4 x f32>):\n  %loaded = vec.load vec<4 x f32>, %ptr [align 16] [mask %mask, passthru %old]\n  vec.store %loaded, %ptr [mask %mask]\n  return %loaded\n}",
    );
    let canonical = round_trip(&source);
    assert!(
        canonical.contains("vec.load vec<4 x f32>, %arg0 [align 16] [mask %arg1, passthru %arg2]")
    );
    assert!(canonical.contains("vec.store %v3, %arg0 [mask %arg1]"));
}

#[test]
fn vector_memory_may_flow_across_blocks_without_losing_alignment() {
    let source = module(
        "func module @VectorMemory.cfg(%ptr: ptr, %mask: vec<2 x i1>, %old: vec<2 x i64>) -> unit engine(native) {\nbb0(%ptr: ptr, %mask: vec<2 x i1>, %old: vec<2 x i64>):\n  %v = vec.load vec<2 x i64>, %ptr [mask %mask, passthru %old]\n  br bb1(%v)\nbb1(%value: vec<2 x i64>):\n  vec.store %value, %ptr [align 8]\n  return\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("vec.load vec<2 x i64>"));
    assert!(canonical.contains("vec.store %"));
}

#[test]
fn vector_memory_rejects_misalignment_mismatched_masks_and_nonvectors() {
    for (parameters, code) in [
        ("%p: ptr", "%v = vec.load vec<4 x i32>, %p [align 3]"),
        ("%p: ptr", "%v = vec.load vec<4 x i32>, %p [align 0]"),
        ("%p: ptr", "%v = vec.load i32, %p"),
        (
            "%p: ptr, %mask: vec<2 x i1>, %old: vec<4 x i32>",
            "%v = vec.load vec<4 x i32>, %p [mask %mask, passthru %old]",
        ),
        (
            "%p: ptr, %mask: vec<4 x i1>",
            "%v = vec.load vec<4 x i32>, %p [mask %mask]",
        ),
        (
            "%p: ptr, %value: i32, %mask: vec<4 x i1>",
            "vec.store %value, %p [mask %mask]",
        ),
        (
            "%p: ptr, %v: vec<4 x i32>, %mask: vec<2 x i1>",
            "vec.store %v, %p [mask %mask]",
        ),
        ("%p: ptr, %v: vec<4 x i32>", "%result = vec.store %v, %p"),
        (
            "%p: ptr, %v: vec<4 x i32>",
            "vec.store %v, %p [align 8] [align 16]",
        ),
        ("%v: vec<4 x i32>", "%result = vec.load vec<4 x i32>, %v"),
    ] {
        let body = format!(
            "func module @VectorMemory.invalid({parameters}) -> unit engine(native) {{\nbb0({parameters}):\n  {code}\n  return\n}}"
        );
        let source = module(&body);
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid vector memory accepted: {code}"
            );
        }
    }
}
