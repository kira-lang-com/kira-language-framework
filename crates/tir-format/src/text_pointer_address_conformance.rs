use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @PointerAddress tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("pointer/address conversion text parses");
    let bytes = assemble_text_module(&parsed).expect("pointer/address CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("pointer/address TIRB decodes");
    let text =
        disassemble_text_module(&binary, u64::MAX).expect("pointer/address CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&text).expect("canonical pointer/address parses"))
            .expect("pointer/address CODE reassembles");
    assert_eq!(bytes, rebuilt);
    text
}

#[test]
fn address_to_pointer_and_back_round_trip() {
    let source = module(
        "func module @PointerAddress.to_ptr(%address: *i32) -> ptr engine(native) {\nbb0(%address: *i32):\n  %p = addr.toptr %address\n  return %p\n}\n\nfunc module @PointerAddress.from_ptr(%p: ptr) -> *i64 engine(native) {\nbb0(%p: ptr):\n  %a = ptr.toaddr %p as *i64\n  return %a\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("addr.toptr %arg0"));
    assert!(canonical.contains("ptr.toaddr %arg0 as *i64"));
}

#[test]
fn pointer_address_chain_round_trip_and_cross_block() {
    let source = module(
        "func module @PointerAddress.chain(%address: *i64) -> *i64 engine(native) {\nbb0(%address: *i64):\n  %ptr = addr.toptr %address\n  br bb1(%ptr)\nbb1(%raw: ptr):\n  %address2 = ptr.toaddr %raw as *i64\n  return %address2\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("addr.toptr %arg0"));
    assert!(canonical.contains("ptr.toaddr %"));
}

#[test]
fn non_generic_address_spaces_preserved_in_casts() {
    let source = module(
        "func module @PointerAddress.space(%address: *i32<device>) -> ptr<device> engine(native) {\nbb0(%address: *i32<device>):\n  %raw = addr.toptr %address\n  return %raw\n}\n\nfunc module @PointerAddress.rebuild(%raw: ptr<device>) -> *i32<device> engine(native) {\nbb0(%raw: ptr<device>):\n  %address = ptr.toaddr %raw as *i32<device>\n  return %address\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("addr.toptr %arg0"));
    assert!(canonical.contains("ptr.toaddr %arg0 as *i32<device>"));
}

#[test]
fn pointer_address_conversion_rejects_wrong_types_and_address_spaces() {
    for (params, opcode) in [
        ("%v: i32", "%out = addr.toptr %v"),
        ("%v: ptr", "%out = addr.toptr %v"),
        ("%v: *i32", "%out = ptr.toaddr %v as *i32"),
        ("%v: ptr", "%out = ptr.toaddr %v as ptr"),
        ("%v: ptr", "%out = ptr.toaddr %v as *i32<thread>"),
        ("%v: ptr<device>", "%out = ptr.toaddr %v as *i32"),
        ("%v: *i32", "%out = addr.toptr %missing"),
        ("%v: ptr", "%out = ptr.toaddr %missing as *i32"),
        ("%v: *i32", "addr.toptr %v"),
        ("%v: ptr", "%out = ptr.toaddr %v as *Missing"),
    ] {
        let source = module(&format!(
            "func module @PointerAddress.invalid({params}) -> unit engine(native) {{\nbb0({params}):\n  {opcode}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid pointer/address conversion accepted: {opcode}"
            );
        }
    }
}

#[test]
fn address_cast_layout_compatible_scalar_types_round_trip() {
    let source = module(
        "func module @PointerAddress.int_float(%address: *i32) -> *f32 engine(native) {\nbb0(%address: *i32):\n  %view = addr.cast %address to *f32\n  return %view\n}\n\nfunc module @PointerAddress.float_int(%address: *f64) -> *i64 engine(native) {\nbb0(%address: *f64):\n  %view = addr.cast %address to *i64\n  return %view\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("addr.cast %arg0 to *f32"));
    assert!(canonical.contains("addr.cast %arg0 to *i64"));
}

#[test]
fn address_cast_preserves_space_and_cross_block_ssa() {
    let source = module(
        "func module @PointerAddress.cross(%address: *i32<device>) -> *f32<device> engine(native) {\nbb0(%address: *i32<device>):\n  %view = addr.cast %address to *f32<device>\n  br bb1(%view)\nbb1(%forwarded: *f32<device>):\n  return %forwarded\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("addr.cast %arg0 to *f32<device>"));
}

#[test]
fn address_cast_rejects_incompatible_layout_and_invalid_sources() {
    for (params, opcode) in [
        ("%a: *i32", "%view = addr.cast %a to *i64"),
        ("%a: *i32<device>", "%view = addr.cast %a to *f32"),
        ("%a: *i32", "%view = addr.cast %a to *f32<device>"),
        ("%a: ptr", "%view = addr.cast %a to *i64"),
        ("%a: *i32", "%view = addr.cast %missing to *f32"),
        ("%a: *i32", "%view = addr.cast %a to ptr"),
        ("%a: *i32", "%view = addr.cast %a to *Missing"),
        ("%a: *i32", "addr.cast %a to *f32"),
    ] {
        let source = module(&format!(
            "func module @PointerAddress.invalid({params}) -> unit engine(native) {{\nbb0({params}):\n  {opcode}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid addr.cast assembled: {opcode}"
            );
        }
    }
}
