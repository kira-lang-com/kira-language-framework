use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @VectorConstant tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("const.vec source parses");
    let bytes = assemble_text_module(&parsed).expect("const.vec CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("const.vec TIRB decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("const.vec disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical const.vec parses"))
            .expect("const.vec CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn inline_integer_vectors_round_trip_and_normalize_signed_lanes() {
    for (ty, lanes) in [
        ("vec<4 x i32>", "[1, -2, 3, 4294967295]"),
        ("vec<2 x i64>", "[-1, 18446744073709551615]"),
        ("vec<8 x i16>", "[0, 1, 2, 3, 4, 5, 6, 65535]"),
        (
            "vec<16 x i8>",
            "[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 255]",
        ),
    ] {
        let source = module(&format!(
            "func module @VectorConstant.make() -> {ty} engine(native) {{\nbb0:\n  %v = const.vec {ty}, {lanes}\n  return %v\n}}"
        ));
        assert!(round_trip(&source).contains(&format!("const.vec {ty}, [")));
    }
}

#[test]
fn integer_vector_lanes_accept_all_normative_integer_radices() {
    let source = module(
        "func module @VectorConstant.radix() -> vec<8 x i16> engine(native) {\nbb0:\n  %v = const.vec vec<8 x i16>, [0x1f, 0b1010, 0o17, 1_000, -0x10, +24, 65535, -32768]\n  return %v\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("[31, 10, 15, 1000, 65520, 24, 65535, 32768]"));
}

#[test]
fn fixed_vector_i1_masks_use_packed_little_endian_bits() {
    for (lanes, items, expected) in [
        ("4", "[true, false, 1, 0]", "[1, 0, 1, 0]"),
        ("8", "[1, 0, 1, 0, 1, 0, 1, 0]", "[1, 0, 1, 0, 1, 0, 1, 0]"),
        (
            "16",
            "[1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 0, 1, 0, 1, 0, 1]",
            "[1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 0, 1, 0, 1, 0, 1]",
        ),
    ] {
        let source = module(&format!(
            "func module @VectorConstant.mask() -> vec<{lanes} x i1> engine(native) {{\nbb0:\n  %v = const.vec vec<{lanes} x i1>, {items}\n  return %v\n}}"
        ));
        let canonical = round_trip(&source);
        assert!(
            canonical.contains(expected),
            "expected {expected}: {canonical}"
        );
    }
}

#[test]
fn large_fixed_i1_mask_uses_typed_constants_record() {
    let items = (0..32)
        .map(|i| (i % 3 == 0) as u8)
        .map(|bit| bit.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let source = module(&format!(
        "func module @VectorConstant.bigmask() -> vec<32 x i1> engine(native) {{\nbb0:\n  %v = const.vec vec<32 x i1>, [{items}]\n  return %v\n}}"
    ));
    let parsed = parse_text_module(&source).expect("large mask parses");
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("large mask assembles"))
            .expect("large mask binary decodes");
    let constants = binary
        .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
        .expect("large mask CONSTANTS section decodes");
    assert_eq!(constants.records.len(), 1);
    let canonical = round_trip(&source);
    assert!(canonical.contains(&format!("const.vec vec<32 x i1>, [{items}]")));
}

#[test]
fn invalid_fixed_vector_i1_mask_bits_are_rejected() {
    for bits in [
        "[0, 1, 2, 0]",
        "[1, -2, 0, 1]",
        "[1, 0, 1]",
        "[1, 0, 1, false, 0]",
    ] {
        let source = module(&format!(
            "func module @VectorConstant.badmask() -> unit engine(native) {{\nbb0:\n  %v = const.vec vec<4 x i1>, {bits}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid mask assembled: {bits}"
            );
        }
    }
}

#[test]
fn three_lanes_and_multiple_constants_round_trip_across_blocks() {
    let source = module(
        "func module @VectorConstant.cfg(%condition: i1) -> vec<3 x i32> engine(native) {\nbb0(%condition: i1):\n  %yes = const.vec vec<3 x i32>, [1, 2, 3]\n  br bb1(%yes)\nbb1(%forwarded: vec<3 x i32>):\n  %zero = const.vec vec<3 x i32>, [0, 0, 0]\n  %sum = add %forwarded, %zero\n  return %sum\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.vec vec<3 x i32>, [1, 2, 3]"));
    assert!(canonical.contains("const.vec vec<3 x i32>, [0, 0, 0]"));
}

#[test]
fn large_integer_vectors_use_constants_table_and_round_trip() {
    for (ty, elements) in [
        ("vec<8 x i32>", "[0, 1, 2, 3, 4, -1, 6, 7]"),
        ("vec<3 x i64>", "[-1, 0, 18446744073709551615]"),
        (
            "vec<32 x i8>",
            "[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 255]",
        ),
    ] {
        let source = module(&format!(
            "func module @VectorConstant.large() -> {ty} engine(native) {{\nbb0:\n  %large = const.vec {ty}, {elements}\n  return %large\n}}"
        ));
        let parsed = parse_text_module(&source).expect("large vector source parses");
        let bytes = assemble_text_module(&parsed).expect("large vector CODE assembles");
        let binary = BinaryModule::decode(&bytes).expect("large vector binary decodes");
        let table = binary
            .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
            .expect("large vector CONSTANTS section decodes");
        assert_eq!(table.records.len(), 1, "large vector must use CONSTANTS");
        assert!(round_trip(&source).contains(&format!("const.vec {ty}, [")));
    }
}

#[test]
fn multiple_large_vectors_share_constants_section_and_keep_indices() {
    let source = module(
        "func module @VectorConstant.first() -> vec<8 x i32> engine(native) {\nbb0:\n  %a = const.vec vec<8 x i32>, [1, 2, 3, 4, 5, 6, 7, 8]\n  return %a\n}\n\nfunc module @VectorConstant.second() -> vec<8 x i32> engine(native) {\nbb0:\n  %a = const.vec vec<8 x i32>, [0, 0, 0, 0, 0, 0, 0, 0]\n  %b = const.vec vec<8 x i32>, [2, 3, 4, 5, 6, 7, 8, 9]\n  %sum = add %a, %b\n  return %sum\n}\n",
    );
    let parsed = parse_text_module(&source).expect("large vector source parses");
    let bytes = assemble_text_module(&parsed).expect("large vector CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("large vector binary decodes");
    assert_eq!(
        binary
            .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
            .expect("CONSTANTS decodes")
            .records
            .len(),
        3
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("[1, 2, 3, 4, 5, 6, 7, 8]"));
    assert!(canonical.contains("[0, 0, 0, 0, 0, 0, 0, 0]"));
    assert!(canonical.contains("[2, 3, 4, 5, 6, 7, 8, 9]"));
}

#[test]
fn global_float_and_large_vector_constants_preserve_cross_section_indexes() {
    let source = module(
        "global module @VectorConstant.seed: i64 = 13\n\nfunc module @VectorConstant.mixed() -> vec<8 x i32> engine(native) {\nbb0:\n  %a = const.vec vec<8 x i32>, [1, 2, 3, 4, 5, 6, 7, 8]\n  %f = const.float f32, 1.5\n  %b = const.vec vec<8 x i32>, [8, 7, 6, 5, 4, 3, 2, 1]\n  %sum = add %a, %b\n  return %sum\n}\n\nfunc module @VectorConstant.other() -> f64 engine(native) {\nbb0:\n  %v = const.float f64, 2.0\n  return %v\n}\n",
    );
    let parsed = parse_text_module(&source).expect("mixed CONSTANTS text parses");
    let bytes = assemble_text_module(&parsed).expect("mixed CONSTANTS CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("mixed CONSTANTS binary decodes");
    let table = binary
        .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
        .expect("mixed CONSTANTS section decodes");
    assert_eq!(table.records.len(), 5, "global, two floats and two vectors");
    let canonical = round_trip(&source);
    assert_eq!(canonical.matches("const.vec vec<8 x i32>").count(), 2);
    assert_eq!(canonical.matches("const.float f32,").count(), 1);
    assert_eq!(canonical.matches("const.float f64,").count(), 1);
    assert!(canonical.contains("@VectorConstant.seed"));
}

#[test]
fn ieee_floating_vector_lanes_round_trip_inline_and_constants_backed() {
    for (ty, lanes, constant_count) in [
        ("vec<4 x f16>", "[1.5, -2.0, 0.0, 4.0]", 0),
        ("vec<4 x bf16>", "[1.5, -2.0, 0.0, 4.0]", 0),
        ("vec<4 x f32>", "[1.5, -2.0, 0.0, 4.0]", 0),
        ("vec<2 x f64>", "[1.5, -2.0]", 0),
        ("vec<3 x f64>", "[1.5, -2.0, 0.0]", 1),
        ("vec<2 x f128>", "[1.5, -2.0]", 1),
    ] {
        let source = module(&format!(
            "func module @VectorConstant.float_vector() -> {ty} engine(native) {{\nbb0:\n  %v = const.vec {ty}, {lanes}\n  return %v\n}}"
        ));
        let parsed = parse_text_module(&source).expect("float const.vec syntax parses");
        let binary = BinaryModule::decode(
            &assemble_text_module(&parsed)
                .unwrap_or_else(|error| panic!("{ty} float const.vec encoding failed: {error:?}")),
        )
        .expect("float const.vec TIRB decodes");
        let constants = binary
            .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
            .expect("float vector CONSTANTS decodes");
        assert_eq!(
            constants.records.len(),
            constant_count,
            "CONSTANTS form for {ty}"
        );
        assert!(round_trip(&source).contains(&format!("const.vec {ty}, [")));
    }
}

#[test]
fn large_ieee_float_vector_and_integer_vector_keep_constants_indices() {
    let source = module(
        "func module @VectorConstant.float_large() -> vec<3 x f64> engine(native) {\nbb0:\n  %a = const.vec vec<3 x f64>, [1.5, -2.0, 4.0]\n  return %a\n}\n\nfunc module @VectorConstant.int_large() -> vec<8 x i32> engine(native) {\nbb0:\n  %b = const.vec vec<8 x i32>, [1, 2, 3, 4, 5, 6, 7, 8]\n  return %b\n}\n",
    );
    let parsed = parse_text_module(&source).expect("mixed vector type source parses");
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("mixed float and integer vectors encode"),
    )
    .expect("mixed vector TIRB decodes");
    assert_eq!(
        binary
            .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
            .expect("mixed CONSTANTS decodes")
            .records
            .len(),
        2
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.vec vec<3 x f64>"));
    assert!(canonical.contains("const.vec vec<8 x i32>"));
}

#[test]
fn floating_vector_lanes_reject_inexact_and_malformed_literals() {
    for (ty, lanes) in [
        ("vec<2 x f32>", "[0.1, 2.0]"),
        ("vec<2 x bf16>", "[0.1, 2.0]"),
        ("vec<2 x f16>", "[65536.0, 1.0]"),
        ("vec<2 x f64>", "[1.0, nonsense]"),
        ("vec<2 x f128>", "[1.0]"),
        ("vec<1 x f128>", "[1.0]"),
    ] {
        let source = module(&format!(
            "func module @VectorConstant.invalid_float() -> unit engine(native) {{\nbb0:\n  %bad = const.vec {ty}, {lanes}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid IEEE lanes accepted: {ty} {lanes}"
            );
        }
    }
}

#[test]
fn inline_const_vec_rejects_wrong_arity_width_storage_and_lane_range() {
    for (ty, lanes) in [
        ("vec<4 x i32>", "[1, 2, 3]"),
        ("vec<4 x i32>", "[1, 2, 3, 4, 5]"),
        ("vec<2 x i8>", "[256, 0]"),
        ("vec<2 x i8>", "[-129, 0]"),
        ("vec<2 x i128>", "[1, 2]"),
        ("i32", "[1]"),
        ("vec<2 x i32>", "[1, garbage]"),
        ("vec<2 x i32>", "[1, 2"),
    ] {
        let source = module(&format!(
            "func module @VectorConstant.invalid() -> unit engine(native) {{\nbb0:\n  %v = const.vec {ty}, {lanes}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid const.vec accepted: {ty} {lanes}"
            );
        }
    }
}
