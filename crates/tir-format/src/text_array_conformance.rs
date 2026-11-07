use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Arrays tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("array text parses");
    let bytes = assemble_text_module(&parsed).expect("array CODE assembles");
    let decoded = BinaryModule::decode(&bytes).expect("array CODE decodes");
    let canonical = disassemble_text_module(&decoded, u64::MAX).expect("array CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("canonical array CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn array_new_insert_extract_round_trip() {
    let source = module(
        "func module @Arrays.run(%a: i32, %b: i32, %index: i32) -> i32 engine(native) {\nbb0(%a: i32, %b: i32, %index: i32):\n  %array = array.new [3 x i32](%a, %b, %a)\n  %replaced = array.insert %array, %index, %b\n  %result = array.extract %replaced, %index\n  return %result\n}",
    );
    let text = round_trip(&source);
    assert!(text.contains("array.new [3 x i32]"));
    assert!(text.contains("array.insert %"));
    assert!(text.contains("array.extract %"));
}

#[test]
fn array_values_are_forwarded_through_basic_blocks() {
    let source = module(
        "func module @Arrays.run(%a: f32, %index: i32) -> f32 engine(native) {\nbb0(%a: f32, %index: i32):\n  %array = array.new [2 x f32](%a, %a)\n  br bb1(%array, %index)\nbb1(%passed: [2 x f32], %i: i32):\n  %result = array.extract %passed, %i\n  return %result\n}",
    );
    let text = round_trip(&source);
    assert!(text.contains("array.new [2 x f32]"));
}

#[test]
fn array_constructor_rejects_mismatched_counts_and_element_types() {
    for (params, expression) in [
        ("%a: i32", "array.new [2 x i32](%a)"),
        ("%a: i32", "array.new [2 x f32](%a, %a)"),
        ("%a: i32", "array.new [2 x i32](%a, %missing)"),
    ] {
        let source = module(&format!(
            "func module @Arrays.bad({params}) -> i32 engine(native) {{\nbb0({params}):\n  %invalid = {expression}\n  return %a\n}}"
        ));
        let parsed = parse_text_module(&source).expect("syntax parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "invalid array encoded: {expression}"
        );
    }
}
