use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(function: &str) -> String {
    format!(
        "module @Floats tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{function}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("text parses");
    let bytes = assemble_text_module(&parsed).expect("floating-point constants assemble");
    let binary = BinaryModule::decode(&bytes).expect("canonical binary decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("binary disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("canonical text reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn two_float_constants_feed_arithmetic_in_one_block() {
    let source = module(
        "func module @Floats.compute() -> f32 engine(native) {\nbb0:\n  %a = const.float f32, 1.5\n  %b = const.float f32, 2.25\n  %sum = fadd %a, %b\n  return %sum\n}",
    );
    let canonical = round_trip(&source);
    assert_eq!(canonical.matches("const.float f32").count(), 2);
    assert!(canonical.contains("fadd"));
}

#[test]
fn float_constants_in_multiple_functions_and_blocks_keep_indexes() {
    let source = module(
        "func module @Floats.first() -> f64 engine(native) {\nbb0:\n  %base = const.float f64, 1.5\n  br bb1(%base)\nbb1(%value: f64):\n  %extra = const.float f64, 2.0\n  %result = fmul %value, %extra\n  return %result\n}\n\nfunc module @Floats.second() -> f32 engine(native) {\nbb0:\n  %answer = const.float f32, 8.0\n  %other = const.float f32, 0.5\n  %result = fsub %answer, %other\n  return %result\n}",
    );
    let canonical = round_trip(&source);
    assert_eq!(canonical.matches("const.float f64").count(), 2);
    assert_eq!(canonical.matches("const.float f32").count(), 2);
}

#[test]
fn float_constant_and_arithmetic_operand_types_must_match() {
    let source = module(
        "func module @Floats.bad() -> f32 engine(native) {\nbb0:\n  %left = const.float f32, 1.0\n  %right = const.float f64, 2.0\n  %sum = fadd %left, %right\n  return %sum\n}",
    );
    let parsed = parse_text_module(&source).expect("text syntax parses");
    assert!(assemble_text_module(&parsed).is_err());
}

#[test]
fn float_constants_reject_inexact_literals() {
    let source = module(
        "func module @Floats.bad() -> f32 engine(native) {\nbb0:\n  %value = const.float f32, 0.1\n  %result = fadd %value, %value\n  return %result\n}",
    );
    let parsed = parse_text_module(&source).expect("text syntax parses");
    assert!(assemble_text_module(&parsed).is_err());
}
