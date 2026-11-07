use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(semantics: &str, body: &str) -> String {
    format!(
        "module @Conversion tir 26.11.1 profile app semantics {semantics}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("text parses");
    let bytes = assemble_text_module(&parsed).expect("conversion CODE assembles");
    let decoded = BinaryModule::decode(&bytes).expect("binary decodes");
    let canonical = disassemble_text_module(&decoded, u64::MAX).expect("CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("canonical CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn integer_conversions_and_float_bitcast_round_trip() {
    let body = "func module @Conversion.convert(%wide: i64, %narrow: i32) -> i64 engine(native) {\nbb0(%wide: i64, %narrow: i32):\n  %cut = trunc %wide to i32\n  %signed = sext %cut to i64\n  %unsigned = zext %narrow to i64\n  %result = add %signed, %unsigned\n  return %result\n}";
    assert!(round_trip(&module("defined", body)).contains("sext %"));
    let body = "func module @Conversion.cast(%number: i32) -> f32 engine(native) {\nbb0(%number: i32):\n  %result = bitcast %number to f32\n  return %result\n}";
    assert!(round_trip(&module("defined", body)).contains("bitcast %"));
}

#[test]
fn scalar_float_conversions_with_rounding_round_trip() {
    let body = "func module @Conversion.float(%number: f64, %integer: i32) -> f64 engine(native) {\nbb0(%number: f64, %integer: i32):\n  %narrow = fptrunc %number to f32 [rtz]\n  %wide = fpext %narrow to f64\n  %converted = sitofp %integer to f64 [rne]\n  %result = fadd %wide, %converted\n  return %result\n}";
    assert!(round_trip(&module("defined", body)).contains("[rtz]"));
    let body = "func module @Conversion.fcvt(%input: f32) -> f16 engine(native) {\nbb0(%input: f32):\n  %result = fcvt %input to f16 [rne] [satfinite]\n  return %result\n}";
    assert!(round_trip(&module("defined", body)).contains("[satfinite]"));
}

#[test]
fn saturating_and_trapping_float_to_int_round_trip() {
    for mode in ["sat", "trap"] {
        let body = format!(
            "func module @Conversion.toint(%value: f64) -> i64 engine(native) {{\nbb0(%value: f64):\n  %result = fptosi [{mode}] %value to i64\n  return %result\n}}"
        );
        let canonical = round_trip(&module("defined", &body));
        assert!(canonical.contains(&format!("fptosi [{mode}]")));
    }
}

#[test]
fn vector_conversions_preserve_lane_shapes() {
    let body = "func module @Conversion.vec(%wide: vec<4 x i64>) -> vec<4 x i32> engine(native) {\nbb0(%wide: vec<4 x i64>):\n  %result = trunc %wide to vec<4 x i32>\n  return %result\n}";
    assert!(round_trip(&module("defined", body)).contains("trunc %"));
}

#[test]
fn invalid_conversions_do_not_assemble() {
    for operation in [
        "trunc %a to i64",
        "zext %a to i16",
        "fpext %a to f32",
        "bitcast %a to i16",
        "fptosi %a to i32",
    ] {
        let body = format!(
            "func module @Conversion.bad(%a: i32) -> i32 engine(native) {{\nbb0(%a: i32):\n  %bad = {operation}\n  return %a\n}}"
        );
        let parsed = parse_text_module(&module("defined", &body)).expect("text parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "invalid operation assembled: {operation}"
        );
    }
}
