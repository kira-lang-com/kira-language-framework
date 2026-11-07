use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @RawAccess tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(input: &str) -> String {
    let parsed = parse_text_module(input).expect("raw-access text parses");
    let bytes = assemble_text_module(&parsed).expect("raw-access CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("binary decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("raw-access CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn scalar_raw_load_store_with_alignment_and_flags_round_trip() {
    let input = module(
        "func module @RawAccess.roundtrip(%p: ptr, %value: i32) -> i32 engine(native) {\nbb0(%p: ptr, %value: i32):\n  raw.store %value, %p [align 4] [volatile] [nontemporal]\n  %loaded = raw.load i32, %p [align 4] [volatile] [nontemporal]\n  return %loaded\n}",
    );
    let canonical = round_trip(&input);
    assert!(canonical.contains("raw.store %"));
    assert!(canonical.contains("raw.load i32, %"));
    assert_eq!(
        canonical
            .matches("[align 4] [volatile] [nontemporal]")
            .count(),
        2
    );
}

#[test]
fn raw_loads_from_second_block_and_float_types_round_trip() {
    let input = module(
        "func module @RawAccess.float(%p: ptr<device>, %v: f32) -> f32 engine(native) {\nbb0(%p: ptr<device>, %v: f32):\n  raw.store %v, %p\n  br bb1(%p)\nbb1(%addr: ptr<device>):\n  %loaded = raw.load f32, %addr [align 4]\n  return %loaded\n}",
    );
    let canonical = round_trip(&input);
    assert!(canonical.contains("raw.load f32"));
    assert!(canonical.contains("raw.store"));
}

#[test]
fn raw_access_rejects_unsupported_or_duplicate_modifiers_and_types() {
    for (params, line) in [
        ("%p: ptr", "%loaded = raw.load i32, %p [align 3]"),
        ("%p: ptr", "%loaded = raw.load i32, %p [align 4] [align 8]"),
        ("%p: ptr", "%loaded = raw.load i1, %p"),
        ("%v: i32", "%loaded = raw.load i32, %v"),
        ("%p: ptr, %v: i1", "raw.store %v, %p"),
        ("%p: ptr, %v: i32", "raw.store %v, %p [volatile] [volatile]"),
        ("%p: ptr, %v: i32", "raw.store %v, %p [align 0]"),
    ] {
        let input = module(&format!(
            "func module @RawAccess.bad({params}) -> unit engine(native) {{\nbb0({params}):\n  {line}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&input) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid raw access assembled: {line}"
            );
        }
    }
}

#[test]
fn disassembly_does_not_erase_incorrect_unmanaged_analysis() {
    let input = module(
        "func module @RawAccess.load(%p: ptr) -> i32 engine(native) {\nbb0(%p: ptr):\n  %v = raw.load i32, %p\n  return %v\n}",
    );
    let parsed = parse_text_module(&input).expect("raw-access text parses");
    let bytes = assemble_text_module(&parsed).expect("raw-access CODE assembles");
    let mut module = BinaryModule::decode(&bytes).expect("binary decodes");
    let section = module
        .sections
        .iter_mut()
        .find(|section| section.kind == 14)
        .expect("ANALYSIS section exists");
    let mut payload = section
        .read_uncompressed(u64::MAX)
        .expect("ANALYSIS payload reads");
    assert_eq!(&payload[..3], &[1, 2, 2]);
    payload[1] = 0;
    payload[2] = 0;
    *section = crate::Section::from_uncompressed(14, payload);
    assert!(
        disassemble_text_module(&module, u64::MAX).is_err(),
        "tirc dis must not silently replace unmanaged ANALYSIS with managed state"
    );
}
