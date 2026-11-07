use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Payload tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("enum payload text parses");
    let bytes = assemble_text_module(&parsed).expect("enum payload CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("enum payload CODE decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("enum payload CODE disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("enum payload canonical text parses"),
    )
    .expect("enum payload CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn enum_take_trivial_one_field_payload_round_trips() {
    let source = module(
        "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\n\nfunc module @Payload.take(%value: %Choice) -> i32 engine(native) {\nbb0(%value: %Choice):\n  %number = enum.take %value, Number\n  return %number\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("enum.take %arg0, Number"));
}

#[test]
fn enum_project_trivial_borrowed_payload_round_trips() {
    let source = module(
        "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\n\nfunc module @Payload.project(%value: borrow %Choice) -> unit engine(native) {\nbb0(%value: borrow %Choice):\n  %number = enum.project %value, Number\n  return\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("enum.project %arg0, Number"));
}

#[test]
fn enum_take_multi_field_payload_forms_tuple_and_flows_to_successor() {
    let source = module(
        "type %Choice = enum ridl { None, Pair(i32, i64) } policy default unmanaged\n\nfunc module @Payload.pair(%value: %Choice) -> i64 engine(native) {\nbb0(%value: %Choice):\n  %payload = enum.take %value, Pair\n  br bb1(%payload)\nbb1(%both: (i32, i64)):\n  %x = tuple.extract %both, 1\n  return %x\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("enum.take %arg0, Pair"));
    assert!(canonical.contains("tuple.extract"));
}

#[test]
fn enum_take_payloadless_variant_yields_unit() {
    let source = module(
        "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\n\nfunc module @Payload.none(%value: %Choice) -> unit engine(native) {\nbb0(%value: %Choice):\n  %empty = enum.take %value, None\n  return\n}",
    );
    assert!(round_trip(&source).contains("enum.take %arg0, None"));
}

#[test]
fn enum_payload_rejects_wrong_variants_and_nonnominal_sources() {
    let decl = "type %Choice = enum ridl { None, Number(i32), Pair(i32, i64) } policy default unmanaged\ntype %Record = struct ridl { number: i32 } policy default unmanaged";
    for (parameters, expression) in [
        ("%value: %Choice", "%number = enum.take %value, Missing"),
        ("%value: %Record", "%number = enum.take %value, Number"),
        ("%value: i32", "%number = enum.take %value, Number"),
        ("%value: %Choice", "%number = enum.take %missing, Number"),
        ("%value: %Choice", "%number = enum.project %value, Missing"),
        (
            "%value: %Choice",
            "%number = enum.take %value, Number\n  %number = enum.take %value, Number",
        ),
        ("%value: %Choice", "enum.take %value, Number"),
        ("%value: %Choice", "%number = enum.take %value, Number()"),
    ] {
        let source = module(&format!(
            "{decl}\n\nfunc module @Payload.bad({parameters}) -> unit engine(native) {{\nbb0({parameters}):\n  {expression}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid enum payload accepted: {expression}"
            );
        }
    }
}

#[test]
fn enum_take_rejects_nontrivial_managed_payload() {
    let source = module(
        "type %Choice = enum ridl { None, Owned(ref i32) } policy default unmanaged\n\nfunc module @Payload.bad(%value: %Choice) -> ref i32 engine(native) {\nbb0(%value: %Choice):\n  %number = enum.take %value, Owned\n  return %number\n}",
    );
    let parsed = parse_text_module(&source).expect("managed enum parses");
    assert!(
        assemble_text_module(&parsed).is_err(),
        "managed enum payload lowering should fail closed"
    );
}
