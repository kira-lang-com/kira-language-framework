use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Device tir 26.11.1 profile driver platform tessera semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n  source \"0000000000000000000000000000000000000000000000000000000000000000\"\n\n{body}\n"
    )
}

fn round_trip(input: &str) -> String {
    let parsed = parse_text_module(input).expect("device text parses");
    let bytes = assemble_text_module(&parsed).expect("device CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("binary decodes");
    let analysis = binary.analysis_summary(u64::MAX).expect("analysis decodes");
    assert_eq!(analysis.functions[0].trust_class, 2);
    assert_eq!(analysis.functions[0].reason_mask & (1 << 7), 1 << 7);
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("device CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn ordered_mmio_load_store_and_io_fences_round_trip() {
    let source = module(
        "func module @Device.access(%reg: ptr<device>, %value: i32) -> i32 engine(native) nounwind {\nbb0(%reg: ptr<device>, %value: i32):\n  fence.io\n  fence.io [write]\n  mmio.store %value, %reg [order release] [align 4]\n  %read = mmio.load i32, %reg [order acquire] [align 4]\n  fence.io [full]\n  return %read\n}",
    );
    let canonical = round_trip(&source);
    for expected in [
        "fence.io [write]",
        "fence.io [full]",
        "mmio.store %",
        "[order release] [align 4]",
        "mmio.load i32,",
        "[order acquire] [align 4]",
    ] {
        assert!(canonical.contains(expected), "missing {expected}");
    }
}

#[test]
fn omitted_and_explicit_relaxed_modes_round_trip() {
    let source = module(
        "func module @Device.relaxed(%reg: ptr, %value: i16) -> i16 engine(native) nounwind {\nbb0(%reg: ptr, %value: i16):\n  mmio.store %value, %reg\n  mmio.store %value, %reg [order relaxed]\n  %read = mmio.load i16, %reg\n  %read2 = mmio.load i16, %reg [order relaxed]\n  return %read2\n}",
    );
    let canonical = round_trip(&source);
    assert_eq!(canonical.matches("[order relaxed]").count(), 2);
}

#[test]
fn mmio_float_in_later_block_round_trips() {
    let source = module(
        "func module @Device.float(%p: ptr<device>, %value: f32) -> f32 engine(native) nounwind {\nbb0(%p: ptr<device>, %value: f32):\n  mmio.store %value, %p [align 4]\n  br bb1(%p)\nbb1(%reg: ptr<device>):\n  %loaded = mmio.load f32, %reg [order acquire] [align 4]\n  return %loaded\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("mmio.load f32"));
}

#[test]
fn mmio_rejects_bad_width_order_alignment_and_operands() {
    for (params, expression) in [
        ("%p: ptr", "%v = mmio.load i128, %p"),
        ("%p: ptr", "%v = mmio.load i32, %p [order release]"),
        ("%p: ptr", "%v = mmio.load i32, %p [align 3]"),
        (
            "%p: ptr",
            "%v = mmio.load i32, %p [order acquire] [order relaxed]",
        ),
        ("%x: i64", "%v = mmio.load i32, %x"),
        ("%p: ptr, %v: i64", "mmio.store %v, %p [order acquire]"),
        ("%p: ptr, %v: i128", "mmio.store %v, %p"),
        ("", "fence.io [unknown]"),
        ("", "fence.io [full] extra"),
    ] {
        let body = format!(
            "func module @Device.bad({params}) -> unit engine(native) nounwind {{\nbb0({params}):\n  {expression}\n  return\n}}"
        );
        let input = module(&body);
        if let Ok(parsed) = parse_text_module(&input) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid MMIO CODE assembled: {expression}"
            );
        }
    }
}
