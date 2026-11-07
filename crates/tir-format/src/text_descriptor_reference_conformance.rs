use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Descriptor tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("descriptor.ref text parses");
    let bytes = assemble_text_module(&parsed).expect("descriptor.ref CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("descriptor.ref CODE decodes");
    let text =
        disassemble_text_module(&binary, u64::MAX).expect("descriptor.ref CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&text).expect("canonical descriptor.ref parses"))
            .expect("descriptor.ref CODE reassembles");
    assert_eq!(bytes, rebuilt);
    text
}

#[test]
fn scalar_tuple_and_nominal_descriptors_round_trip() {
    let source = module(
        "type %Record = struct ridl { value: i32 } policy default unmanaged\n\nfunc module @Descriptor.scalars() -> ptr engine(native) {\nbb0:\n  %first = descriptor.ref i32\n  %second = descriptor.ref (ptr, i64)\n  %third = descriptor.ref %Record\n  return %third\n}",
    );
    let text = round_trip(&source);
    for ty in [
        "descriptor.ref i32",
        "descriptor.ref (ptr, i64)",
        "descriptor.ref %Record",
    ] {
        assert!(text.contains(ty), "missing {ty}");
    }
}

#[test]
fn descriptor_pointer_result_may_cross_block_edge() {
    let source = module(
        "func module @Descriptor.forward() -> ptr engine(native) {\nbb0:\n  %address = descriptor.ref i64\n  br bb1(%address)\nbb1(%descriptor: ptr):\n  return %descriptor\n}",
    );
    assert!(round_trip(&source).contains("descriptor.ref i64"));
}

#[test]
fn descriptor_ref_rejects_bad_syntax_and_duplicate_results() {
    for code in [
        "descriptor.ref i32",
        "%first, %second = descriptor.ref i32",
        "%value = descriptor.ref UnknownType",
        "%value = descriptor.ref i0",
        "%value = descriptor.ref %Undefined",
        "%value = descriptor.ref i32\n  %value = descriptor.ref i64",
    ] {
        let source = module(&format!(
            "func module @Descriptor.invalid() -> unit engine(native) {{\nbb0:\n  {code}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "malformed descriptor.ref assembled: {code}"
            );
        }
    }
}
