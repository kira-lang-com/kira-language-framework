use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, semantics: &str) -> String {
    format!(
        "module @Projection tir 26.11.1 profile app semantics {semantics}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}
fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("address projection text parses");
    let bytes = assemble_text_module(&parsed).expect("address projection CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("address projection CODE decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("address projection disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("address projection canonical text parses"),
    )
    .expect("address projection CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn tuple_field_address_preserves_pointee_and_space() {
    let source = module(
        "func module @Projection.tuple(%base: *(i32, f64)) -> *f64 engine(native) {\nbb0(%base: *(i32, f64)):\n  %field = field.addr %base, 1\n  return %field\n}",
        "defined",
    );
    assert!(round_trip(&source).contains("field.addr %arg0, 1"));
}

#[test]
fn declared_struct_field_address_uses_types_definition() {
    let source = module(
        "type %Pair = struct ridl { left: i32, right: i64 } policy default unmanaged\n\nfunc module @Projection.struct(%base: *%Pair) -> *i64 engine(native) {\nbb0(%base: *%Pair):\n  %field = field.addr %base, 1\n  return %field\n}",
        "defined",
    );
    assert!(round_trip(&source).contains("field.addr %arg0, 1"));
}

#[test]
fn array_element_address_uses_integer_ssa_index() {
    let source = module(
        "func module @Projection.element(%base: *[4 x i32], %index: i32) -> *i32 engine(native) {\nbb0(%base: *[4 x i32], %index: i32):\n  %element = elem.addr %base, %index\n  return %element\n}",
        "defined",
    );
    assert!(round_trip(&source).contains("elem.addr %arg0, %arg1"));
}

#[test]
fn projected_addresses_can_be_loaded_across_blocks() {
    let source = module(
        "func module @Projection.load(%base: *[4 x i32], %index: i32) -> i32 engine(native) {\nbb0(%base: *[4 x i32], %index: i32):\n  %element = elem.addr %base, %index\n  br bb1(%element)\nbb1(%address: *i32):\n  %value = load [trivial] %address\n  return %value\n}",
        "defined",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("elem.addr %arg0, %arg1"));
    assert!(canonical.contains("load [trivial]"));
}

#[test]
fn unchecked_elem_address_encodes_modifier_bit_and_round_trips() {
    let source = module(
        "func module @Projection.fast(%base: *[2 x i64], %index: i64) -> *i64 engine(native) {\nbb0(%base: *[2 x i64], %index: i64):\n  %element = elem.addr %base, %index [unchecked]\n  return %element\n}",
        "permissive",
    );
    assert!(round_trip(&source).contains("elem.addr %arg0, %arg1 [unchecked]"));
}

#[test]
fn projections_reject_out_of_range_and_wrong_types() {
    let declarations =
        "type %Pair = struct ridl { left: i32, right: i64 } policy default unmanaged";
    for (params, expression) in [
        ("%base: *%Pair", "%result = field.addr %base, 2"),
        ("%base: *%Pair", "%result = field.addr %base, -1"),
        ("%base: *%Pair", "%result = field.addr %base, bogus"),
        ("%base: *i32", "%result = field.addr %base, 0"),
        ("%base: *%Pair", "%result = field.addr %missing, 0"),
        ("%base: %Pair", "%result = field.addr %base, 0"),
        (
            "%base: *[4 x i32], %index: ptr",
            "%result = elem.addr %base, %index",
        ),
        ("%base: *[4 x i32]", "%result = elem.addr %base, %missing"),
        (
            "%base: *i32, %index: i32",
            "%result = elem.addr %base, %index",
        ),
        (
            "%base: *[4 x i32], %index: i32",
            "%result = elem.addr %base, %index [bad]",
        ),
        ("%base: *[4 x i32], %index: i32", "elem.addr %base, %index"),
        (
            "%base: *[4 x i32], %index: i32",
            "%result = elem.addr %base, %index\n  %result = elem.addr %base, %index",
        ),
    ] {
        let source = module(
            &format!(
                "{declarations}\n\nfunc module @Projection.invalid({params}) -> unit engine(native) {{\nbb0({params}):\n  {expression}\n  return\n}}"
            ),
            "defined",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "bad projection assembled: {expression}"
            );
        }
    }
}
