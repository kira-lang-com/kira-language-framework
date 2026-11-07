use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(mode: &str, body: &str) -> String {
    format!(
        "module @Raw tir 26.11.1 profile app semantics {mode}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("raw memory module parses");
    let encoded = assemble_text_module(&parsed).expect("raw memory CODE encodes");
    let binary = BinaryModule::decode(&encoded).expect("raw memory binary decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("raw CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("canonical raw CODE reassembles");
    assert_eq!(encoded, rebuilt);
    canonical
}

#[test]
fn pointer_offset_and_difference_round_trip() {
    let source = module(
        "defined",
        "func module @Raw.distance(%p: ptr, %q: ptr, %bytes: i64) -> i64 engine(native) {\nbb0(%p: ptr, %q: ptr, %bytes: i64):\n  %adjusted = ptr.offset %p, %bytes\n  %distance = ptr.diff %adjusted, %q\n  return %distance\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("ptr.offset %"));
    assert!(canonical.contains("ptr.diff %"));
}

#[test]
fn pointer_inbounds_and_memory_range_modifiers_round_trip() {
    let source = module(
        "permissive",
        "func module @Raw.copy(%dst: ptr, %src: ptr, %count: i64, %byte: i8) -> unit engine(native) {\nbb0(%dst: ptr, %src: ptr, %count: i64, %byte: i8):\n  %shifted = ptr.offset [inbounds] %dst, %count\n  mem.copy %shifted, %src, %count [volatile] [nooverlap]\n  mem.move %dst, %src, %count [volatile]\n  mem.set %dst, %byte, %count [volatile]\n  return\n}",
    );
    let canonical = round_trip(&source);
    for expected in [
        "ptr.offset [inbounds]",
        "mem.copy %",
        "[nooverlap]",
        "mem.move %",
        "mem.set %",
    ] {
        assert!(canonical.contains(expected), "missing {expected}");
    }
}

#[test]
fn prefetch_default_and_full_modifiers_round_trip() {
    let source = module(
        "defined",
        "func module @Raw.prefetch(%ptr: ptr) -> unit engine(native) {\nbb0(%ptr: ptr):\n  prefetch %ptr\n  prefetch %ptr [write] [locality 3] [instruction]\n  prefetch %ptr [read] [locality 1] [data]\n  return\n}",
    );
    let canonical = round_trip(&source);
    assert_eq!(canonical.matches("prefetch %").count(), 3);
    assert!(canonical.contains("[write] [locality 3] [instruction]"));
}

#[test]
fn pointers_from_different_address_spaces_rejected_for_difference() {
    let source = module(
        "defined",
        "func module @Raw.invalid(%a: ptr, %b: ptr<device>) -> i64 engine(native) {\nbb0(%a: ptr, %b: ptr<device>):\n  %difference = ptr.diff %a, %b\n  return %difference\n}",
    );
    let parsed = parse_text_module(&source).expect("TIR parses");
    assert!(assemble_text_module(&parsed).is_err());
}

#[test]
fn invalid_memory_argument_types_and_modifiers_rejected() {
    for (params, line) in [
        ("%ptr: ptr, %byte: i8", "ptr.offset %ptr, %byte"),
        ("%dst: ptr, %size: i64", "mem.set %dst, %size, %size"),
        (
            "%dst: ptr, %src: ptr, %size: i64",
            "mem.move %dst, %src, %size [nooverlap]",
        ),
        ("%dst: ptr, %size: i64", "prefetch %dst [locality 4]"),
    ] {
        let result = if line.starts_with("ptr.offset") {
            "  %value = "
        } else {
            "  "
        };
        let source = module(
            "permissive",
            &format!(
                "func module @Raw.bad({params}) -> unit engine(native) {{\nbb0({params}):\n{result}{line}\n  return\n}}"
            ),
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "bad raw memory assembled: {line}"
            );
        }
    }
}
