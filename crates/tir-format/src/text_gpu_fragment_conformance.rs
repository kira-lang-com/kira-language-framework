use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Fragments tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("fragment text parses");
    let bytes = assemble_text_module(&parsed).expect("fragment CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("fragment TIRB decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("fragment CODE disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical fragment text parses"),
    )
    .expect("fragment CODE reassembles");
    assert_eq!(
        bytes, rebuilt,
        "fragment binary/text/binary must be byte-identical"
    );
    canonical
}

#[test]
fn fragment_demote_and_raster_order_controls_round_trip() {
    let source = module(
        "func module @Fragments.controls() -> unit target(gpu) {\nbb0:\n  frag.demote\n  frag.raster_order.begin\n  frag.raster_order.end\n  return\n}\n",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("    frag.demote\n"));
    assert!(canonical.contains("    frag.raster_order.begin\n"));
    assert!(canonical.contains("    frag.raster_order.end\n"));
}

#[test]
fn fragment_attachment_indices_round_trip_full_u32_range() {
    for (input, canonical_index) in [("0", "0"), ("0x10", "16"), ("4294967295", "4294967295")] {
        let source = module(&format!(
            "func module @Fragments.attachment() -> vec<4 x f32> target(gpu) {{\nbb0:\n  %value = frag.load_attachment {input}\n  return %value\n}}\n"
        ));
        let canonical = round_trip(&source);
        assert!(canonical.contains(&format!("frag.load_attachment {canonical_index}")));
    }
}

#[test]
fn fragment_imageblock_tile_address_round_trips() {
    let source = module(
        "func module @Fragments.tile() -> ptr<tile> target(gpu) {\nbb0:\n  %address = frag.imageblock.addr\n  return %address\n}\n",
    );
    assert!(round_trip(&source).contains("frag.imageblock.addr"));
}

#[test]
fn fragment_values_and_controls_work_across_multiple_cfg_blocks() {
    let source = module(
        "func module @Fragments.cfg() -> vec<4 x f32> target(gpu) {\nbb0:\n  %first = frag.load_attachment 3\n  frag.raster_order.begin\n  br bb1(%first)\nbb1(%data: vec<4 x f32>):\n  frag.raster_order.end\n  frag.demote\n  return %data\n}\n",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("frag.load_attachment 3"));
    assert!(canonical.contains("frag.raster_order.begin"));
    assert!(canonical.contains("frag.raster_order.end"));
}

#[test]
fn fragment_opcodes_reject_wrong_results_operands_and_attachment_index() {
    for operation in [
        "%v = frag.demote",
        "frag.demote 0",
        "%v = frag.raster_order.begin",
        "frag.raster_order.begin 1",
        "%v = frag.raster_order.end",
        "frag.raster_order.end extra",
        "frag.load_attachment 0",
        "%v, %w = frag.load_attachment 1",
        "%v = frag.load_attachment -1",
        "%v = frag.load_attachment 4294967296",
        "%v = frag.load_attachment banana",
        "%v = frag.imageblock.addr 1",
        "frag.imageblock.addr",
        "%v, %w = frag.imageblock.addr",
    ] {
        let source = module(&format!(
            "func module @Fragments.invalid() -> unit target(gpu) {{\nbb0:\n  {operation}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid fragment CODE accepted: {operation}"
            );
        }
    }
}

#[test]
fn fragment_opcodes_are_rejected_outside_gpu_target() {
    for operation in [
        "frag.demote",
        "frag.raster_order.begin",
        "frag.raster_order.end",
        "%v = frag.load_attachment 1",
        "%v = frag.imageblock.addr",
    ] {
        let source = module(&format!(
            "func module @Fragments.cpu() -> unit engine(native) {{\nbb0:\n  {operation}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "CPU fragment instruction encoded: {operation}"
            );
        }
    }
}
