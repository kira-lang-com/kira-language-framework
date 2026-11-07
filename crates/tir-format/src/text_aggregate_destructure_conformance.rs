use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Destructure tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}
fn round_trip(source: &str) -> String {
    let text = parse_text_module(source).expect("aggregate destructure text parses");
    let bytes = assemble_text_module(&text).expect("aggregate destructure encodes");
    let binary = BinaryModule::decode(&bytes).expect("aggregate destructure TIRB decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("aggregate destructure disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical destructure parses"))
            .expect("aggregate destructure reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn tuple_destructure_into_two_numeric_fields_round_trips() {
    let source = module(
        "func module @Destructure.run(%t: (i32, f64)) -> f64 engine(native) {\nbb0(%t: (i32, f64)):\n  %x, %y = tuple.destructure %t\n  return %y\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("%v1, %v2 = tuple.destructure %arg0"));
    assert!(canonical.contains("return %v2"));
}

#[test]
fn struct_destructure_returns_declared_fields_in_order() {
    let source = module(
        "type %Triple = struct ridl { x: i8, y: i32, z: ptr } policy default unmanaged\n\nfunc module @Destructure.run(%s: %Triple) -> ptr engine(native) {\nbb0(%s: %Triple):\n  %a, %b, %c = struct.destructure %s\n  return %c\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("%v1, %v2, %v3 = struct.destructure %arg0"));
    assert!(canonical.contains("return %v3"));
}

#[test]
fn destructure_results_feed_arithmetic_and_branch_parameters() {
    let source = module(
        "type %Pair = struct ridl { x: i32, y: i32 } policy default unmanaged\n\nfunc module @Destructure.sum(%p: %Pair) -> i32 engine(native) {\nbb0(%p: %Pair):\n  %x, %y = struct.destructure %p\n  %sum = add %x, %y\n  br bb1(%sum)\nbb1(%result: i32):\n  return %result\n}\n\nfunc module @Destructure.tuple(%p: (i64, i64)) -> i64 engine(native) {\nbb0(%p: (i64, i64)):\n  %first, %second = tuple.destructure %p\n  %sum = add %first, %second\n  return %sum\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("struct.destructure"));
    assert!(canonical.contains("tuple.destructure"));
}

#[test]
fn destructure_rejects_arity_duplicates_and_type_mismatches() {
    let declarations = "type %Triple = struct ridl { x: i32, y: i64, z: ptr } policy default unmanaged\ntype %Enum = enum ridl { A, B } policy default unmanaged";
    for (parameters, line) in [
        ("%x: (i32, i64)", "%a = tuple.destructure %x"),
        ("%x: (i32, i64)", "%a, %b, %c = tuple.destructure %x"),
        ("%x: (i32, i64)", "%a, %a = tuple.destructure %x"),
        ("%x: (i32, i64)", "%x, %b = tuple.destructure %x"),
        ("%x: (i32, i64)", "tuple.destructure %x"),
        ("%x: (i32, i64)", "%a, %b = tuple.destructure %missing"),
        ("%x: %Triple", "%a, %b = struct.destructure %x"),
        ("%x: %Triple", "%a, %b, %c, %d = struct.destructure %x"),
        ("%x: %Enum", "%a, %b = struct.destructure %x"),
        ("%x: i32", "%a, %b = tuple.destructure %x"),
        ("%x: (i32, i64)", "%a, %b = struct.destructure %x"),
    ] {
        let source = module(&format!(
            "{declarations}\n\nfunc module @Destructure.bad({parameters}) -> unit engine(native) {{\nbb0({parameters}):\n  {line}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "bad destructure encoded: {line}"
            );
        }
    }
}

#[test]
fn destructure_rejects_owned_fields_until_ownership_lowering_exists() {
    let source = module(
        "type %Managed = struct ridl { ref_value: ref i32, number: i32 } policy default unmanaged\n\nfunc module @Destructure.bad(%value: %Managed) -> i32 engine(native) {\nbb0(%value: %Managed):\n  %a, %b = struct.destructure %value\n  return %b\n}",
    );
    let parsed = parse_text_module(&source).expect("managed struct syntax parses");
    assert!(
        assemble_text_module(&parsed).is_err(),
        "managed destructuring must fail closed"
    );
}
