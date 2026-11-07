use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Specialize tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("specialization source parses");
    let bytes = assemble_text_module(&parsed).expect("specialization CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("specialization binary decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("specialization CODE disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical specialization source parses"),
    )
    .expect("specialization reassembles");
    assert_eq!(bytes, rebuilt, "specialization binary round-trip differs");
    canonical
}

#[test]
fn scalar_integer_and_boolean_gpu_specializations_round_trip() {
    for (id, ty, literal) in [
        ("1", "i1", "true"),
        ("0", "i1", "false"),
        ("0xff", "i5", "-1"),
        ("4294967295", "i64", "18446744073709551615"),
        ("0b1010", "i16", "0o77"),
        ("1234", "i32", "-12"),
    ] {
        let source = module(&format!(
            "func module @Specialize.scalar() -> {ty} target(gpu) {{\nbb0:\n  %constant = gpu.spec_const {id}, {ty}, {literal}\n  return %constant\n}}"
        ));
        assert!(round_trip(&source).contains(&format!("gpu.spec_const ")));
    }
}

#[test]
fn scalar_ieee_gpu_specializations_round_trip() {
    for (ty, literal) in [
        ("f16", "1.5"),
        ("bf16", "-2.0"),
        ("f32", "4.0"),
        ("f64", "1.5"),
    ] {
        let source = module(&format!(
            "func module @Specialize.float() -> {ty} target(gpu) {{\nbb0:\n  %constant = gpu.spec_const 1, {ty}, {literal}\n  return %constant\n}}"
        ));
        let canonical = round_trip(&source);
        assert!(canonical.contains(&format!("gpu.spec_const 1, {ty},")));
    }
}

#[test]
fn fixed_vector_gpu_specializations_round_trip() {
    for (ty, literal) in [
        ("vec<2 x i1>", "[true, false]"),
        ("vec<3 x i16>", "[-1, 0x1f, 2]"),
        ("vec<4 x f32>", "[1.5, 2.0, -1.0, 4.0]"),
        ("vec<3 x f64>", "[1.5, 2.0, 3.0]"),
        ("vec<4 x bf16>", "[0.0, 1.5, 2.0, 4.0]"),
    ] {
        let source = module(&format!(
            "func module @Specialize.vector() -> {ty} target(gpu) {{\nbb0:\n  %constant = gpu.spec_const 42, {ty}, {literal}\n  return %constant\n}}"
        ));
        assert!(round_trip(&source).contains(&format!("gpu.spec_const 42, {ty}, [")));
    }
}

#[test]
fn specialization_defaults_share_constants_without_cross_index_corruption() {
    let source = module(
        "global module @Specialize.seed: i32 = 7\n\nfunc module @Specialize.mixed() -> vec<3 x f32> target(gpu) {\nbb0:\n  %first = gpu.spec_const 1, i32, 42\n  %float = const.float f32, 1.5\n  %vector = const.vec vec<8 x i32>, [1, 2, 3, 4, 5, 6, 7, 8]\n  %result = gpu.spec_const 2, vec<3 x f32>, [1.5, 2.0, 3.0]\n  return %result\n}\n\nfunc module @Specialize.second() -> f16 target(gpu) {\nbb0:\n  %result = gpu.spec_const 0xabc, f16, -2.0\n  return %result\n}\n",
    );
    let parsed = parse_text_module(&source).expect("mixed specialization source parses");
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("mixed specialization CODE assembles"),
    )
    .expect("mixed binary decodes");
    let constants = binary
        .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
        .expect("mixed CONSTANTS section decodes");
    assert_eq!(
        constants.records.len(),
        6,
        "global, float, vector and three GPU defaults"
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("gpu.spec_const 1, i32, 42"));
    assert!(canonical.contains("gpu.spec_const 2, vec<3 x f32>, ["));
    assert!(canonical.contains("gpu.spec_const 2748, f16,"));
}

#[test]
fn gpu_specializations_reject_invalid_id_types_and_defaults() {
    for (id, ty, literal, target) in [
        ("-1", "i32", "1", "target(gpu)"),
        ("4294967296", "i32", "1", "target(gpu)"),
        ("1", "i1", "1", "target(gpu)"),
        ("1", "i1", "yes", "target(gpu)"),
        ("1", "i3", "8", "target(gpu)"),
        ("1", "i3", "-5", "target(gpu)"),
        ("1", "i65", "4", "target(gpu)"),
        ("1", "f128", "1.5", "target(gpu)"),
        ("1", "f32", "0.1", "target(gpu)"),
        ("1", "vec<4 x i32>", "[1, 2, 3]", "target(gpu)"),
        (
            "1",
            "vec<8 x i32>",
            "[1, 2, 3, 4, 5, 6, 7, 8]",
            "target(gpu)",
        ),
        ("1", "vec<2 x i1>", "[1, 0]", "target(gpu)"),
        ("1", "vec<2 x i32>", "[1, garbage]", "target(gpu)"),
        ("1", "i32", "1", "engine(native)"),
    ] {
        let source = module(&format!(
            "func module @Specialize.invalid() -> unit {target} {{\nbb0:\n  %default = gpu.spec_const {id}, {ty}, {literal}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid GPU specialization accepted: {id}, {ty}, {literal}, {target}"
            );
        }
    }
}

#[test]
fn specialization_defaults_feed_cfg_and_scalar_arithmetic() {
    let source = module(
        "func module @Specialize.cfg() -> i32 target(gpu) {\nbb0:\n  %first = gpu.spec_const 0x10, i32, 7\n  br bb1(%first)\nbb1(%value: i32):\n  %second = gpu.spec_const 0x20, i32, 9\n  %sum = add %value, %second\n  return %sum\n}\n",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("gpu.spec_const 16, i32, 7"));
    assert!(canonical.contains("gpu.spec_const 32, i32, 9"));
}
