use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Tuple tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("tuple text parses");
    let bytes = assemble_text_module(&parsed).expect("tuple CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("tuple binary decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("tuple CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("canonical tuple CODE assembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn tuple_new_and_extract_scalar_fields_round_trip() {
    let source = module(
        "func module @Tuple.run(%a: i32, %b: f32) -> i32 engine(native) {\nbb0(%a: i32, %b: f32):\n  %combined = tuple.new (%a, %b)\n  %first = tuple.extract %combined, 0\n  return %first\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("tuple.new (%"));
    assert!(canonical.contains("tuple.extract %"));
}

#[test]
fn tuple_new_can_be_returned_and_forwarded() {
    let source = module(
        "func module @Tuple.run(%a: i32, %b: f32) -> (i32, f32) engine(native) {\nbb0(%a: i32, %b: f32):\n  %combined = tuple.new (%a, %b)\n  br bb1(%combined)\nbb1(%forwarded: (i32, f32)):\n  return %forwarded\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("tuple.new"));
}

#[test]
fn tuple_rejects_invalid_field_index_and_nontrivial_element() {
    for body in [
        "func module @Tuple.bad(%a: i32) -> i32 engine(native) {\nbb0(%a: i32):\n  %t = tuple.new (%a)\n  %bad = tuple.extract %t, 2\n  return %bad\n}",
        "func module @Tuple.bad(%a: i32) -> i32 engine(native) {\nbb0(%a: i32):\n  %t = tuple.new (%unbound)\n  %bad = tuple.extract %t, 0\n  return %bad\n}",
    ] {
        let parsed = parse_text_module(&module(body)).expect("tuple input parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "invalid tuple assembled: {body}"
        );
    }
}
