use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @EnumAddress tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("enum address text parses");
    let bytes = assemble_text_module(&parsed).expect("enum address CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("enum address TIRB decodes");
    let text = disassemble_text_module(&binary, u64::MAX).expect("enum address CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&text).expect("canonical enum address parses"))
            .expect("canonical enum address reassembles");
    assert_eq!(bytes, rebuilt);
    text
}

#[test]
fn enum_payload_address_scalar_round_trip() {
    let source = module(
        "type %Choice = enum ridl { None, Number(i32), Pair(i32, i64) } policy default unmanaged\n\nfunc module @EnumAddress.payload(%base: *%Choice) -> *i32 engine(native) {\nbb0(%base: *%Choice):\n  %projected = enum.payload.addr %base, Number\n  return %projected\n}",
    );
    assert!(round_trip(&source).contains("enum.payload.addr %arg0, Number"));
}

#[test]
fn enum_payload_address_tuple_round_trip_across_block() {
    let source = module(
        "type %Choice = enum ridl { None, Number(i32), Pair(i32, i64) } policy default unmanaged\n\nfunc module @EnumAddress.pair(%base: *%Choice) -> *(i32, i64) engine(native) {\nbb0(%base: *%Choice):\n  %projected = enum.payload.addr %base, Pair\n  br bb1(%projected)\nbb1(%forwarded: *(i32, i64)):\n  return %forwarded\n}",
    );
    assert!(round_trip(&source).contains("enum.payload.addr %arg0, Pair"));
}

#[test]
fn enum_payload_address_numeric_variant_is_canonicalized_by_name() {
    let source = module(
        "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\n\nfunc module @EnumAddress.numeric(%base: *%Choice) -> *i32 engine(native) {\nbb0(%base: *%Choice):\n  %projected = enum.payload.addr %base, 1\n  return %projected\n}",
    );
    assert!(round_trip(&source).contains("enum.payload.addr %arg0, Number"));
}

#[test]
fn enum_inject_payloadless_variant_round_trips() {
    let source = module(
        "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\n\nfunc module @EnumAddress.inject(%base: *%Choice) -> unit engine(native) {\nbb0(%base: *%Choice):\n  enum.inject %base, None\n  return\n}",
    );
    assert!(round_trip(&source).contains("enum.inject %arg0, None"));
}

#[test]
fn enum_address_rejects_invalid_variants_and_missing_initialization_proof() {
    let declarations = "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\ntype %Pair = struct ridl { number: i32 } policy default unmanaged";
    for (params, instruction) in [
        (
            "%base: *%Choice",
            "%result = enum.payload.addr %base, Missing",
        ),
        ("%base: *%Choice", "%result = enum.payload.addr %base, 2"),
        ("%base: *%Choice", "%result = enum.payload.addr %base, -1"),
        (
            "%base: *%Choice",
            "%result = enum.payload.addr %missing, Number",
        ),
        ("%base: *%Pair", "%result = enum.payload.addr %base, Number"),
        ("%base: i32", "%result = enum.payload.addr %base, Number"),
        ("%base: *%Choice", "enum.payload.addr %base, Number"),
        ("%base: *%Choice", "enum.inject %base, Number"),
        ("%base: *%Choice", "enum.inject %base, Missing"),
        ("%base: *%Choice", "%result = enum.inject %base, None"),
    ] {
        let source = module(&format!(
            "{declarations}\n\nfunc module @EnumAddress.invalid({params}) -> unit engine(native) {{\nbb0({params}):\n  {instruction}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid enum address assembled: {instruction}"
            );
        }
    }
}
