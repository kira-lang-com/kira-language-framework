use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(source: &str) -> String {
    format!(
        "module @Nominal tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{source}\n"
    )
}
fn round_trip(source: &str) -> String {
    let text = parse_text_module(source).expect("nominal constructor source parses");
    let bytes = assemble_text_module(&text).expect("nominal constructor CODE encodes");
    let binary = BinaryModule::decode(&bytes).expect("nominal CODE decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("nominal CODE formats");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("nominal CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn trivial_struct_new_and_insert_round_trip() {
    let source = module(
        "type %Record = struct ridl { first: i32, second: i64 } policy default unmanaged\n\nfunc module @Nominal.make(%x: i32, %y: i64) -> %Record engine(native) {\nbb0(%x: i32, %y: i64):\n  %new = struct.new %Record(%x, %y)\n  %replaced = struct.insert %new, 0, %x\n  return %replaced\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("struct.new %Record(%arg0, %arg1)"));
    assert!(canonical.contains("struct.insert %v2, 0, %arg0"));
}

#[test]
fn enum_new_with_and_without_payload_round_trips() {
    let source = module(
        "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\n\nfunc module @Nominal.number(%x: i32) -> %Choice engine(native) {\nbb0(%x: i32):\n  %choice = enum.new %Choice, Number(%x)\n  return %choice\n}\n\nfunc module @Nominal.none() -> %Choice engine(native) {\nbb0:\n  %choice = enum.new %Choice, None()\n  return %choice\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("enum.new %Choice, Number(%arg0)"));
    assert!(canonical.contains("enum.new %Choice, None()"));
}

#[test]
fn enum_tag_of_ridl_enum_returns_i8_and_round_trips() {
    let source = module(
        "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\n\nfunc module @Nominal.tag(%choice: %Choice) -> i8 engine(native) {\nbb0(%choice: %Choice):\n  %tag = enum.tag %choice\n  return %tag\n}\n\nfunc module @Nominal.created(%x: i32) -> i8 engine(native) {\nbb0(%x: i32):\n  %created = enum.new %Choice, Number(%x)\n  %tag = enum.tag %created\n  return %tag\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("enum.tag %arg0"));
    assert!(canonical.contains("enum.tag %v1"));
}

#[test]
fn enum_tag_rejects_non_enum_unknown_values_and_duplicate_results() {
    let definitions = "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\ntype %Pair = struct ridl { value: i32 } policy default unmanaged\ntype %CChoice = enum c { None, Number } policy default unmanaged";
    for (parameter, statement) in [
        ("%value: %Pair", "%tag = enum.tag %value"),
        ("%value: %Choice", "%tag = enum.tag %missing"),
        ("%value: %Choice", "enum.tag %value"),
        (
            "%value: %Choice",
            "%tag = enum.tag %value\n  %tag = enum.tag %value",
        ),
    ] {
        let source = module(&format!(
            "{definitions}\n\nfunc module @Nominal.invalid({parameter}) -> i8 engine(native) {{\nbb0({parameter}):\n  {statement}\n  %zero = const.int i8, 0\n  return %zero\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid enum.tag accepted: {statement}"
            );
        }
    }
}

#[test]
fn enum_tag_c_and_packed_layouts_use_correct_integer_widths() {
    let source = module(
        "type %CChoice = enum c { None, Value } policy default unmanaged\ntype %Packed = enum packed(1) { None, Value(i64) } policy default unmanaged\n\nfunc module @Nominal.c_tag(%value: %CChoice) -> i32 engine(native) {\nbb0(%value: %CChoice):\n  %tag = enum.tag %value\n  return %tag\n}\n\nfunc module @Nominal.packed_tag(%value: %Packed) -> i8 engine(native) {\nbb0(%value: %Packed):\n  %tag = enum.tag %value\n  return %tag\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("enum.tag %arg0"));
    assert!(canonical.contains("-> i32 engine(native)"));
    assert!(canonical.contains("-> i8 engine(native)"));
}

#[test]
fn enum_tag_wide_ridl_uses_i16_for_257_variants() {
    let variants = (0..257)
        .map(|index| format!("V{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = module(&format!(
        "type %Wide = enum ridl {{ {variants} }} policy default unmanaged\n\nfunc module @Nominal.wide_tag(%choice: %Wide) -> i16 engine(native) {{\nbb0(%choice: %Wide):\n  %tag = enum.tag %choice\n  return %tag\n}}",
    ));
    let canonical = round_trip(&source);
    assert!(canonical.contains("-> i16 engine(native)"));
    assert!(canonical.contains("enum.tag %arg0"));
}

#[test]
fn enum_tag_rejects_wrong_width_for_c_and_ridl() {
    for (layout, return_ty) in [("c", "i8"), ("ridl", "i32"), ("packed(2)", "i16")] {
        let source = module(&format!(
            "type %Wrong = enum {layout} {{ Empty, Value }} policy default unmanaged\n\nfunc module @Nominal.bad(%value: %Wrong) -> {return_ty} engine(native) {{\nbb0(%value: %Wrong):\n  %tag = enum.tag %value\n  return %tag\n}}"
        ));
        let parsed = parse_text_module(&source).expect("enum width syntax parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "incorrect enum tag width accepted: {layout} -> {return_ty}"
        );
    }
}

#[test]
fn c_enum_with_payload_is_not_a_valid_c_layout() {
    let source = module("type %Invalid = enum c { None, Value(i32) } policy default unmanaged");
    let parsed = parse_text_module(&source).expect("C enum source parses");
    assert!(
        assemble_text_module(&parsed).is_err(),
        "C enum payload requires non-C layout"
    );
}

#[test]
fn nominal_struct_can_be_forwarded_through_block_parameters() {
    let source = module(
        "type %Pair = struct ridl { left: i32, right: i32 } policy default unmanaged\n\nfunc module @Nominal.pair(%x: i32) -> %Pair engine(native) {\nbb0(%x: i32):\n  %pair = struct.new %Pair(%x, %x)\n  br bb1(%pair)\nbb1(%value: %Pair):\n  %updated = struct.insert %value, 1, %x\n  return %updated\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("struct.new %Pair("));
    assert!(canonical.contains("struct.insert %"));
}

#[test]
fn struct_extract_resolves_declared_field_type_and_round_trips() {
    let source = module(
        "type %Record = struct ridl { number: i32, ratio: f32, address: ptr } policy default unmanaged\n\nfunc module @Nominal.number(%record: %Record) -> i32 engine(native) {\nbb0(%record: %Record):\n  %value = struct.extract %record, 0\n  return %value\n}\n\nfunc module @Nominal.ratio(%record: %Record) -> f32 engine(native) {\nbb0(%record: %Record):\n  %value = struct.extract %record, 1\n  return %value\n}\n\nfunc module @Nominal.pointer(%record: %Record) -> ptr engine(native) {\nbb0(%record: %Record):\n  %value = struct.extract %record, 2\n  return %value\n}",
    );
    let canonical = round_trip(&source);
    for field in 0..3 {
        assert!(canonical.contains(&format!("struct.extract %arg0, {field}")));
    }
}

#[test]
fn struct_new_and_extract_can_chain_through_block_parameters() {
    let source = module(
        "type %Pair = struct ridl { left: i64, right: i64 } policy default unmanaged\n\nfunc module @Nominal.chained(%x: i64) -> i64 engine(native) {\nbb0(%x: i64):\n  %pair = struct.new %Pair(%x, %x)\n  br bb1(%pair)\nbb1(%forwarded: %Pair):\n  %right = struct.extract %forwarded, 1\n  return %right\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("struct.new %Pair(%arg0, %arg0)"));
    assert!(canonical.contains("struct.extract %"));
}

#[test]
fn struct_extract_rejects_wrong_field_types_invalid_indexes_and_nonstructs() {
    let declarations = "type %Pair = struct ridl { x: i32, y: f64 } policy default unmanaged\ntype %Choice = enum ridl { A, B } policy default unmanaged";
    for (parameter, code) in [
        ("%value: %Pair", "%result = struct.extract %value, 2"),
        ("%value: %Pair", "%result = struct.extract %value, -1"),
        ("%value: %Pair", "%result = struct.extract %value, nonsense"),
        ("%value: %Choice", "%result = struct.extract %value, 0"),
        ("%value: i32", "%result = struct.extract %value, 0"),
        ("%value: %Pair", "%result = struct.extract %missing, 0"),
        ("%value: %Pair", "struct.extract %value, 0"),
        (
            "%value: %Pair",
            "%result = struct.extract %value, 0\n  %result = struct.extract %value, 1",
        ),
    ] {
        let source = module(&format!(
            "{declarations}\n\nfunc module @Nominal.invalid({parameter}) -> unit engine(native) {{\nbb0({parameter}):\n  {code}\n  return\n}}",
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid struct.extract accepted: {code}"
            );
        }
    }
    let wrong_return = module(&format!(
        "{declarations}\n\nfunc module @Nominal.wrong(%value: %Pair) -> i64 engine(native) {{\nbb0(%value: %Pair):\n  %result = struct.extract %value, 0\n  return %result\n}}",
    ));
    assert!(
        assemble_text_module(&parse_text_module(&wrong_return).expect("bad return source parses"))
            .is_err()
    );
}

#[test]
fn malformed_nominal_constructors_are_rejected() {
    let definition = "type %Record = struct ridl { first: i32, second: i64 } policy default unmanaged\ntype %Choice = enum ridl { None, Number(i32) } policy default unmanaged";
    for code in [
        "%new = struct.new %Missing(%x, %y)",
        "%new = struct.new %Record(%x)",
        "%new = struct.new %Record(%x, %y, %x)",
        "%new = struct.new %Record(%y, %x)",
        "%new = struct.new %Record(%x, %x)",
        "%new = enum.new %Choice, Unknown(%x)",
        "%new = enum.new %Choice, None(%x)",
        "%new = enum.new %Choice, Number()",
        "%new = enum.new %Choice, Number(%x, %y)",
        "%new = enum.new %Choice, Number(%y)",
        "%new = struct.insert %x, 0, %x",
        "%new = struct.insert %missing, 0, %x",
        "%new = struct.insert %source, 99, %x",
        "%new = struct.insert %source, 1, %x",
        "struct.new %Record(%x, %y)",
    ] {
        let source = module(&format!(
            "{definition}\n\nfunc module @Nominal.invalid(%x: i32, %y: i64, %source: %Record) -> unit engine(native) {{\nbb0(%x: i32, %y: i64, %source: %Record):\n  {code}\n  return\n}}"
        ));
        if let Ok(text) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&text).is_err(),
                "invalid nominal constructor assembled: {code}"
            );
        }
    }
}
