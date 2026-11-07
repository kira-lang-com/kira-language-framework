use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, params: &str, returns: &str) -> String {
    format!(
        "module @Vectors tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Vectors.compute({params}) -> {returns} engine(native) {{\nbb0({params}):\n{body}\n}}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("text parses");
    let bytes = assemble_text_module(&parsed).expect("vector CODE assembles");
    let decoded = BinaryModule::decode(&bytes).expect("vector binary decodes");
    let canonical = disassemble_text_module(&decoded, u64::MAX).expect("vector CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("vector CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn splat_reverse_extract_insert_and_vscale_round_trip() {
    let body = "  %vec = vec.splat %scalar to vec<4 x i32>\n  %reverse = vec.reverse %vec\n  %replaced = vec.insert %reverse, %index, %scalar\n  %extracted = vec.extract %replaced, %index\n  %step = vec.vscale\n  return %extracted";
    let canonical = round_trip(&module(body, "%scalar: i32, %index: i32", "i32"));
    assert!(canonical.contains("vec.splat"));
    assert!(canonical.contains("vec.reverse"));
    assert!(canonical.contains("vec.vscale"));
}

#[test]
fn step_concat_and_split_round_trip() {
    let body = "  %step = vec.step vec<4 x i32>\n  %double = vec.concat %step, %step\n  %back = vec.split %double [hi]\n  return %back";
    let canonical = round_trip(&module(body, "", "vec<4 x i32>"));
    assert!(canonical.contains("vec.concat"));
    assert!(canonical.contains("vec.split"));
}

#[test]
fn zip_unzip_and_mask_queries_round_trip() {
    let body = "  %interleave = vec.zip %a, %b [hi]\n  %restore = vec.unzip %interleave, %a [odd]\n  return %restore";
    let canonical = round_trip(&module(
        body,
        "%a: vec<4 x f32>, %b: vec<4 x f32>",
        "vec<4 x f32>",
    ));
    assert!(canonical.contains("vec.zip"));
    let mask = "  %count = vec.mask.count %mask\n  %first = vec.mask.first %mask\n  %result = add %count, %first\n  return %result";
    let canonical = round_trip(&module(mask, "%mask: vec<8 x i1>", "i64"));
    assert!(canonical.contains("vec.mask.count"));
}

#[test]
fn wrong_vector_element_type_is_rejected() {
    let body = "  %bad = vec.splat %scalar to vec<4 x f32>\n  return %bad";
    let source = module(body, "%scalar: i32", "vec<4 x f32>");
    let parsed = parse_text_module(&source).expect("text parses");
    assert!(assemble_text_module(&parsed).is_err());
}

#[test]
fn scalable_active_mask_can_be_queried_and_round_trips() {
    let body = "  %mask = vec.mask.active %start, %limit\n  %count = vec.mask.count %mask\n  %first = vec.mask.first %mask\n  %sum = add %count, %first\n  return %sum";
    let text = round_trip(&module(body, "%start: i64, %limit: i64", "i64"));
    assert!(text.contains("vec.mask.active %arg0, %arg1"));
    assert!(text.contains("vec.mask.count %v2"));
    assert!(text.contains("vec.mask.first %v2"));
}

#[test]
fn scalable_active_mask_can_be_returned_and_forwarded() {
    let source = "module @Masks tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Masks.make(%start: i32, %limit: i64) -> svec<i1> engine(native) {\nbb0(%start: i32, %limit: i64):\n  %active = vec.mask.active %start, %limit\n  br bb1(%active)\nbb1(%forwarded: svec<i1>):\n  return %forwarded\n}\n";
    let text = round_trip(source);
    assert!(text.contains("vec.mask.active %arg0, %arg1"));
    assert!(text.contains("svec<i1>"));
}

#[test]
fn scalable_active_mask_rejects_wrong_arguments_and_arity() {
    for (params, body) in [
        (
            "%start: f32, %limit: i64",
            "  %mask = vec.mask.active %start, %limit\n  return",
        ),
        (
            "%start: i64, %limit: ptr",
            "  %mask = vec.mask.active %start, %limit\n  return",
        ),
        ("%start: i64", "  %mask = vec.mask.active %start\n  return"),
        (
            "%start: i64, %limit: i64",
            "  %mask = vec.mask.active %start, %missing\n  return",
        ),
        (
            "%start: i64, %limit: i64",
            "  vec.mask.active %start, %limit\n  return",
        ),
        (
            "%value: svec<i32>",
            "  %count = vec.mask.count %value\n  return",
        ),
    ] {
        let source = module(body, params, "unit");
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid scalable mask accepted: {body}"
            );
        }
    }
}

#[test]
fn compress_expand_and_slide_round_trip() {
    let body = "  %compressed = vec.compress %a, %mask, %b\n  %expanded = vec.expand %compressed, %mask, %a\n  %shifted = vec.slide %expanded, %b, %index [down]\n  return %shifted";
    let canonical = round_trip(&module(
        body,
        "%a: vec<4 x i32>, %b: vec<4 x i32>, %mask: vec<4 x i1>, %index: i32",
        "vec<4 x i32>",
    ));
    for opcode in ["vec.compress", "vec.expand", "vec.slide"] {
        assert!(canonical.contains(opcode), "missing {opcode}");
    }
    assert!(canonical.contains("vec.slide %v5, %arg1, %arg3 [down]"));
}

#[test]
fn byte_table_returns_shape_of_integer_indices() {
    let body = "  %result = vec.table %bytes, %indexes\n  return %result";
    let canonical = round_trip(&module(
        body,
        "%bytes: vec<8 x i8>, %indexes: vec<4 x i32>",
        "vec<4 x i8>",
    ));
    assert!(canonical.contains("vec.table %arg0, %arg1"));
}

#[test]
fn compress_slide_and_table_reject_incompatible_shapes() {
    for (params, instruction) in [
        (
            "%a: vec<4 x i32>, %mask: vec<2 x i1>",
            "%out = vec.compress %a, %mask, %a",
        ),
        (
            "%a: vec<4 x i32>, %mask: vec<4 x i1>, %b: vec<2 x i32>",
            "%out = vec.expand %a, %mask, %b",
        ),
        (
            "%a: vec<4 x i32>, %b: vec<2 x i32>, %n: i32",
            "%out = vec.slide %a, %b, %n",
        ),
        (
            "%a: vec<4 x i32>, %n: vec<4 x i32>",
            "%out = vec.slide %a, %a, %n",
        ),
        (
            "%a: vec<4 x i32>, %n: i32",
            "%out = vec.slide %a, %a, %n [left]",
        ),
        (
            "%t: vec<4 x i32>, %i: vec<4 x i32>",
            "%out = vec.table %t, %i",
        ),
        (
            "%t: vec<4 x i8>, %i: vec<4 x f32>",
            "%out = vec.table %t, %i",
        ),
    ] {
        let source = module(&format!("  {instruction}\n  return"), params, "unit");
        let parsed = parse_text_module(&source).expect("invalid vector syntax parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "invalid vector opcode assembled: {instruction}"
        );
    }
}

#[test]
fn integer_dot_product_kinds_round_trip_with_four_to_one_lane_groups() {
    for kind in ["sdot", "udot", "usdot"] {
        let body = format!("  %result = vec.dot {kind} %acc, %left, %right\n  return %result");
        let canonical = round_trip(&module(
            &body,
            "%acc: vec<4 x i32>, %left: vec<16 x i8>, %right: vec<16 x i8>",
            "vec<4 x i32>",
        ));
        assert!(canonical.contains(&format!("vec.dot {kind} %arg0, %arg1, %arg2")));
    }
}

#[test]
fn bf16_dot_product_uses_two_lanes_per_f32_accumulator() {
    let body = "  %result = vec.dot bfdot %acc, %left, %right\n  return %result";
    let canonical = round_trip(&module(
        body,
        "%acc: vec<4 x f32>, %left: vec<8 x bf16>, %right: vec<8 x bf16>",
        "vec<4 x f32>",
    ));
    assert!(canonical.contains("vec.dot bfdot %arg0, %arg1, %arg2"));
}

#[test]
fn dot_product_rejects_wrong_kinds_lanes_and_element_types() {
    for (params, instruction) in [
        (
            "%acc: vec<4 x i32>, %left: vec<8 x i8>, %right: vec<8 x i8>",
            "%out = vec.dot sdot %acc, %left, %right",
        ),
        (
            "%acc: vec<4 x i64>, %left: vec<16 x i8>, %right: vec<16 x i8>",
            "%out = vec.dot udot %acc, %left, %right",
        ),
        (
            "%acc: vec<4 x f32>, %left: vec<16 x bf16>, %right: vec<16 x bf16>",
            "%out = vec.dot bfdot %acc, %left, %right",
        ),
        (
            "%acc: vec<4 x i32>, %left: vec<16 x i8>, %right: vec<16 x i16>",
            "%out = vec.dot usdot %acc, %left, %right",
        ),
        (
            "%acc: vec<4 x i32>, %left: vec<16 x i8>, %right: vec<16 x i8>",
            "%out = vec.dot unknown %acc, %left, %right",
        ),
    ] {
        let source = module(&format!("  {instruction}\n  return"), params, "unit");
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "bad vec.dot encoded: {instruction}"
            );
        }
    }
}
