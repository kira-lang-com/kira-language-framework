use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Bytes tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(body: &str) -> String {
    let source = module(body);
    let parsed = parse_text_module(&source).expect("TIR text parses");
    let encoded = assemble_text_module(&parsed).expect("const.bytes CODE assembles");
    let binary = BinaryModule::decode(&encoded).expect("binary decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("binary disassembles");
    let rebuilt = assemble_text_module(&parse_text_module(&canonical).expect("text reparses"))
        .expect("CODE reassembles");
    assert_eq!(encoded, rebuilt);
    canonical
}

#[test]
fn byte_constants_feed_integer_and_float_arithmetic() {
    let source = r#"func module @Bytes.int(%a: i32) -> i32 engine(native) {
bb0(%a: i32):
  %little_endian = const.bytes i32, b"\x01\x00\x00\x00"
  %sum = add %little_endian, %a
  return %sum
}

func module @Bytes.float(%a: f32) -> f32 engine(native) {
bb0(%a: f32):
  %one = const.bytes f32, b"\x00\x00\x80\x3F"
  %product = fmul %a, %one
  return %product
}"#;
    let canonical = round_trip(source);
    assert!(canonical.contains("b\"\\x01\\x00\\x00\\x00\""));
    assert!(canonical.contains("b\"\\x00\\x00\\x80\\x3F\""));
}

#[test]
fn byte_constants_in_different_blocks_round_trip() {
    let source = r#"func module @Bytes.chain(%a: i64) -> i64 engine(native) {
bb0(%a: i64):
  br bb1(%a)
bb1(%v: i64):
  %mask = const.bytes i64, b"\x00\x00\x00\x00\x00\x00\x00\x80"
  %result = xor %v, %mask
  return %result
}"#;
    let canonical = round_trip(source);
    assert!(canonical.contains("const.bytes i64"));
}

#[test]
fn incorrect_byte_lengths_and_subbyte_types_are_rejected() {
    for expression in [
        r#"const.bytes i32, b"\x01\x02""#,
        r#"const.bytes f32, b"\x00\x00\x00""#,
        r#"const.bytes i7, b"\x7F""#,
    ] {
        let body = format!(
            "func module @Bytes.invalid() -> i32 engine(native) {{\nbb0:\n  %bad = {expression}\n  %answer = const.int i32, 0\n  return %answer\n}}"
        );
        let parsed = parse_text_module(&module(&body)).expect("TIR text parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "bad byte constant assembled: {expression}"
        );
    }
}
