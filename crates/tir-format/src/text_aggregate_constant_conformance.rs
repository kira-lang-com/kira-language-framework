use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @AggregateConstant tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}
fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("aggregate constant source parses");
    let bytes = assemble_text_module(&parsed).expect("aggregate constant CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("aggregate constant TIRB decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("aggregate constant disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical aggregate text parses"),
    )
    .expect("aggregate constant CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn trivial_tuple_with_integer_float_and_pointer_fields_round_trips() {
    let source = module(
        "func module @AggregateConstant.tuple() -> (i32, f32, ptr) engine(native) {\nbb0:\n  %v = const.aggregate (i32, f32, ptr)(0x20, 1.5, null)\n  return %v\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.aggregate (i32, f32, ptr)(32, "));
    assert!(canonical.contains("null)"));
}

#[test]
fn fixed_array_aggregate_constants_round_trip_and_flow_across_blocks() {
    let source = module(
        "func module @AggregateConstant.array() -> i64 engine(native) {\nbb0:\n  %first = const.aggregate [4 x i64](1, 2, 3, 4)\n  br bb1(%first)\nbb1(%values: [4 x i64]):\n  %second = const.aggregate [4 x i64](-1, 0, 0b11, 0x4)\n  %index = const.int i32, 0\n  %item = array.extract %second, %index\n  return %item\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.aggregate [4 x i64](1, 2, 3, 4)"));
    assert!(canonical.contains("const.aggregate [4 x i64](18446744073709551615, 0, 3, 4)"));
}

#[test]
fn tuple_array_and_other_constants_keep_shared_constants_indexes() {
    let source = module(
        "global module @AggregateConstant.seed: i32 = 7\n\nfunc module @AggregateConstant.vector() -> vec<8 x i32> engine(native) {\nbb0:\n  %v = const.vec vec<8 x i32>, [1, 2, 3, 4, 5, 6, 7, 8]\n  return %v\n}\n\nfunc module @AggregateConstant.tuple() -> (i32, f64) engine(native) {\nbb0:\n  %value = const.aggregate (i32, f64)(42, 2.0)\n  return %value\n}\n\nfunc module @AggregateConstant.float() -> f32 engine(native) {\nbb0:\n  %value = const.float f32, 1.5\n  return %value\n}\n",
    );
    let parsed = parse_text_module(&source).expect("mixed source parses");
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("mixed CODE assembles"))
            .expect("mixed TIRB decodes");
    let constants = binary
        .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
        .expect("mixed CONSTANTS decodes");
    assert_eq!(constants.records.len(), 4);
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.aggregate (i32, f64)(42, "));
    assert!(canonical.contains("const.vec vec<8 x i32>"));
    assert!(canonical.contains("const.float f32,"));
}

#[test]
fn nested_tuple_and_fixed_array_constants_round_trip() {
    let source = module(
        "func module @AggregateConstant.nested() -> ((i8, f32), [2 x i32]) engine(native) {\nbb0:\n  %aggregate = const.aggregate ((i8, f32), [2 x i32])((7, 1.5), (-2, 0x30))\n  return %aggregate\n}\n\nfunc module @AggregateConstant.array_tuple() -> [2 x (i32, f64)] engine(native) {\nbb0:\n  %aggregate = const.aggregate [2 x (i32, f64)]((1, 1.5), (2, -2.0))\n  return %aggregate\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.aggregate ((i8, f32), [2 x i32])((7, "));
    assert!(canonical.contains("(4294967294, 48))"));
    assert!(canonical.contains("const.aggregate [2 x (i32, f64)]((1, "));
}

#[test]
fn recursive_literal_nesting_round_trips_through_constants() {
    let source = module(
        "func module @AggregateConstant.deep() -> ([2 x (i16, ptr)], (i32, i32)) engine(native) {\nbb0:\n  %aggregate = const.aggregate ([2 x (i16, ptr)], (i32, i32))(((1, null), (2, null)), (3, 4))\n  return %aggregate\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.aggregate ([2 x (i16, ptr)], (i32, i32))"));
    assert_eq!(canonical.matches("null").count(), 2);
}

#[test]
fn nested_aggregate_rejects_bad_delimiters_arity_and_field_types() {
    for (ty, values) in [
        ("((i32, i32), f32)", "((1), 2.0)"),
        ("((i32, i32), f32)", "((1, 2, 3), 2.0)"),
        ("((i32, i32), f32)", "((1, 2), 0.1)"),
        ("([2 x i8], i32)", "((1, 256), 2)"),
        ("([2 x i8], i32)", "((1, 2], 3)"),
        ("([2 x i8], i32)", "(1, 2, 3)"),
        ("([2 x i8], i32)", "((1, 2),)"),
        ("[2 x (i32, ptr)]", "((1, null), (2, 0))"),
        ("[2 x (i32, ptr)]", "((1, null), (2, null)"),
    ] {
        let source = module(&format!(
            "func module @AggregateConstant.bad() -> unit engine(native) {{\nbb0:\n  %value = const.aggregate {ty}{values}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid nested aggregate assembled: {ty}{values}"
            );
        }
    }
}

#[test]
fn aggregate_constant_rejects_wrong_arity_types_and_inexact_literals() {
    for (ty, values) in [
        ("(i32, f32)", "(1)"),
        ("(i32, f32)", "(1, 2.0, 3)"),
        ("(i32, f32)", "(1, 0.1)"),
        ("(i32, ptr)", "(1, 0x1234)"),
        ("(i8, i32)", "(256, 2)"),
        ("(i8, i32)", "(-129, 2)"),
        ("[3 x i16]", "(1, 2)"),
        ("[2 x f16]", "(0.1, 2.0)"),
        ("i32", "(1)"),
        ("(ref i32, i32)", "(null, 1)"),
    ] {
        let source = module(&format!(
            "func module @AggregateConstant.bad() -> unit engine(native) {{\nbb0:\n  %value = const.aggregate {ty}{values}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid aggregate assembled: {ty}{values}"
            );
        }
    }
}

#[test]
fn trivial_nominal_struct_constant_round_trips_with_declared_field_types() {
    let source = module(
        "type %Record = struct ridl { count: i32, weight: f64, location: ptr } policy default unmanaged\n\nfunc module @AggregateConstant.record() -> %Record engine(native) {\nbb0:\n  %literal = const.aggregate %Record(42, 1.5, null)\n  return %literal\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.aggregate %Record(42, "));
    assert!(canonical.contains("null)"));
}

#[test]
fn nested_tuple_of_nominal_struct_constants_preserves_fields() {
    let source = module(
        "type %Pair = struct ridl { x: i32, y: f32 } policy default unmanaged\n\nfunc module @AggregateConstant.nested() -> (%Pair, [2 x i64]) engine(native) {\nbb0:\n  %literal = const.aggregate (%Pair, [2 x i64])((1, 1.5), (-2, 3))\n  return %literal\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.aggregate (%Pair, [2 x i64])((1, "));
    assert!(canonical.contains("18446744073709551614, 3)"));
}

#[test]
fn nominal_aggregate_rejects_bad_field_shapes_and_unknown_types() {
    for (ty, args) in [
        ("%Pair", "(1)"),
        ("%Pair", "(1, 1.5, 3)"),
        ("%Pair", "(1.5, 3)"),
        ("%Pair", "(1, 0.1)"),
        ("%Unknown", "(1, 2)"),
        ("%Managed", "(null, 2)"),
    ] {
        let source = module(&format!(
            "type %Pair = struct ridl {{ x: i32, y: f32 }} policy default unmanaged\ntype %Managed = struct ridl {{ owned: ref i32, count: i32 }} policy default unmanaged\n\nfunc module @AggregateConstant.invalid() -> unit engine(native) {{\nbb0:\n  %literal = const.aggregate {ty}{args}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid nominal constant encoded: {ty}{args}"
            );
        }
    }
}

#[test]
fn explicit_union_active_member_constants_round_trip() {
    let source = module(
        "type %Choice = explicit explicit(size 8, align 8) { bits: i64 @0, fraction: f64 @0 } policy default unmanaged\n\nfunc module @AggregateConstant.bits() -> %Choice engine(native) {\nbb0:\n  %literal = const.aggregate %Choice(0, 0x1234)\n  return %literal\n}\n\nfunc module @AggregateConstant.fraction() -> %Choice engine(native) {\nbb0:\n  %literal = const.aggregate %Choice(1, 1.5)\n  return %literal\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.aggregate %Choice(0, 4660)"));
    assert!(canonical.contains("const.aggregate %Choice(1, "));
}

#[test]
fn explicit_union_inside_tuple_preserves_active_member() {
    let source = module(
        "type %Choice = explicit explicit(size 8, align 8) { bits: i64 @0, fraction: f64 @0 } policy default unmanaged\n\nfunc module @AggregateConstant.nested_union() -> (%Choice, i32) engine(native) {\nbb0:\n  %literal = const.aggregate (%Choice, i32)((1, 2.0), 3)\n  return %literal\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.aggregate (%Choice, i32)((1, "));
}

#[test]
fn explicit_union_rejects_unknown_member_and_wrong_payload() {
    for (ty, literal) in [
        ("%Choice", "(-1, 42)"),
        ("%Choice", "(2, 42)"),
        ("%Choice", "(0, 1.5)"),
        ("%Choice", "(1, 0.1)"),
        ("%Choice", "(0)"),
        ("%Choice", "(0, 42, 1)"),
        ("%Record", "(0, 42)"),
    ] {
        let source = module(&format!(
            "type %Choice = explicit explicit(size 8, align 8) {{ bits: i64 @0, fraction: f64 @0 }} policy default unmanaged\ntype %Record = struct ridl {{ bits: i64 }} policy default unmanaged\n\nfunc module @AggregateConstant.bad_union() -> unit engine(native) {{\nbb0:\n  %literal = const.aggregate {ty}{literal}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid explicit union accepted: {ty}{literal}"
            );
        }
    }
}

#[test]
fn explicit_union_tuple_and_array_members_round_trip() {
    let source = module(
        "type %Packet = explicit explicit(size 16, align 8) { pair: (i64, i32) @0, words: [4 x i32] @0 } policy default unmanaged\n\nfunc module @AggregateConstant.pair() -> %Packet engine(native) {\nbb0:\n  %literal = const.aggregate %Packet(0, (0x42, -3))\n  return %literal\n}\n\nfunc module @AggregateConstant.words() -> %Packet engine(native) {\nbb0:\n  %literal = const.aggregate %Packet(1, (1, 2, 3, 4))\n  return %literal\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.aggregate %Packet(0, (66, 4294967293))"));
    assert!(canonical.contains("const.aggregate %Packet(1, (1, 2, 3, 4))"));
}

#[test]
fn explicit_union_tuple_member_rejects_wrong_nested_shape() {
    let source = module(
        "type %Packet = explicit explicit(size 16, align 8) { pair: (i64, i32) @0, words: [4 x i32] @0 } policy default unmanaged\n\nfunc module @AggregateConstant.bad() -> unit engine(native) {\nbb0:\n  %literal = const.aggregate %Packet(0, (1, 2, 3))\n  return\n}",
    );
    let parsed = parse_text_module(&source).expect("bad explicit union source parses");
    assert!(assemble_text_module(&parsed).is_err());
}

#[test]
fn aggregate_negative_integer_with_nonbyte_width_clears_padding_bits() {
    let source = module(
        "func module @AggregateConstant.odd_width() -> (i5, i9, i1) engine(native) {\nbb0:\n  %value = const.aggregate (i5, i9, i1)(-1, -2, true)\n  return %value\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("const.aggregate (i5, i9, i1)(31, 510, 1)"));
}
