use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @FP tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(text: &str) -> String {
    let parsed = parse_text_module(text).expect("FP text parses");
    let bytes = assemble_text_module(&parsed).expect("FP CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("FP CODE decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("FP CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical FP text parses"))
            .expect("FP CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn fclass_frexp_ldexp_and_fp_status_round_trip() {
    let text = module(
        "func module @FP.scalar(%a: f32, %exponent: i32) -> f32 engine(native) {\nbb0(%a: f32, %exponent: i32):\n  %class = fclass %a\n  %parts = frexp %a\n  %new = ldexp %a, %exponent\n  %flags = fp.flags.read\n  fp.flags.clear\n  return %new\n}",
    );
    let canonical = round_trip(&text);
    for expected in [
        " = fclass %",
        " = frexp %",
        " = ldexp %",
        " = fp.flags.read",
        "fp.flags.clear",
    ] {
        assert!(canonical.contains(expected), "missing {expected}");
    }
}

#[test]
fn class_result_and_frexp_tuple_can_be_used_by_later_instructions() {
    let text = module(
        "func module @FP.classify(%a: f64) -> i32 engine(native) {\nbb0(%a: f64):\n  %pair = frexp %a\n  %mantissa = tuple.extract %pair, 0\n  %code = fclass %mantissa\n  return %code\n}",
    );
    let canonical = round_trip(&text);
    assert!(canonical.contains("tuple.extract %"));
    assert!(canonical.contains("fclass %"));
}

#[test]
fn fp_flags_read_in_later_block_round_trips() {
    let text = module(
        "func module @FP.flags() -> i32 engine(native) {\nbb0:\n  fp.flags.clear\n  br bb1()\nbb1:\n  %flags = fp.flags.read\n  return %flags\n}",
    );
    assert!(round_trip(&text).contains("fp.flags.read"));
}

#[test]
fn invalid_fp_misc_operand_types_and_result_count_are_rejected() {
    for (params, line) in [
        ("%a: i32", "%result = fclass %a"),
        ("%a: f32", "%result = ldexp %a, %a"),
        ("%a: f32", "%result = frexp %unknown"),
        ("", "fp.flags.read"),
        ("", "%value = fp.flags.clear"),
        ("", "%value = fp.flags.read %unknown"),
    ] {
        let source = module(&format!(
            "func module @FP.bad({params}) -> unit engine(native) {{\nbb0({params}):\n  {line}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid FP instruction encoded: {line}"
            );
        }
    }
}
