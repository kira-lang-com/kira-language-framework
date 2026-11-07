use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(mode: &str, body: &str) -> String {
    format!(
        "module @RawHeap tir 26.11.1 profile app semantics {mode}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("raw heap text parses");
    let encoded = assemble_text_module(&parsed).expect("raw heap CODE assembles");
    let module = BinaryModule::decode(&encoded).expect("raw heap binary decodes");
    let analysis = module.analysis_summary(u64::MAX).expect("analysis decodes");
    assert_eq!(analysis.functions[0].trust_class, 2);
    assert_eq!(analysis.functions[0].reason_mask & 0x02, 0x02);
    let canonical = disassemble_text_module(&module, u64::MAX).expect("raw heap CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("raw heap CODE reassembles");
    assert_eq!(encoded, rebuilt);
    canonical
}

#[test]
fn allocation_reallocation_and_free_round_trip() {
    let source = module(
        "defined",
        "func module @RawHeap.run(%size: i64, %alignment: i64) -> unit engine(native) {\nbb0(%size: i64, %alignment: i64):\n  %pointer = heap.alloc.raw %size, %alignment\n  %resized = heap.realloc.raw %pointer, %size, %alignment\n  heap.free.raw %resized\n  return\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("heap.alloc.raw %"));
    assert!(canonical.contains("heap.realloc.raw %"));
    assert!(canonical.contains("heap.free.raw %"));
}

#[test]
fn permissive_uninitialized_allocation_round_trips() {
    let source = module(
        "permissive",
        "func module @RawHeap.run(%size: i64, %alignment: i64) -> ptr engine(native) {\nbb0(%size: i64, %alignment: i64):\n  %result = heap.alloc.raw %size, %alignment [uninit]\n  return %result\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("heap.alloc.raw %"));
    assert!(canonical.contains("[uninit]"));
}

#[test]
fn raw_heap_allocation_in_second_block_round_trips() {
    let source = module(
        "defined",
        "func module @RawHeap.run(%size: i64, %align: i64) -> unit engine(native) {\nbb0(%size: i64, %align: i64):\n  br bb1(%size, %align)\nbb1(%bytes: i64, %alignment: i64):\n  %allocated = heap.alloc.raw %bytes, %alignment\n  heap.free.raw %allocated\n  return\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("heap.alloc.raw"));
}

#[test]
fn invalid_raw_heap_operand_shapes_are_rejected() {
    for (params, operation) in [
        (
            "%size: i32, %align: i64",
            "%ptr = heap.alloc.raw %size, %align",
        ),
        ("%size: i64", "%ptr = heap.alloc.raw %size, %size [unknown]"),
        ("%ptr: ptr<device>", "heap.free.raw %ptr"),
        (
            "%size: i64, %align: i64",
            "%ptr = heap.realloc.raw %size, %align, %size",
        ),
        (
            "%size: i64, %align: i64",
            "%ptr = heap.realloc.raw %size, %size, %align [uninit]",
        ),
    ] {
        let body = format!(
            "func module @RawHeap.invalid({params}) -> unit engine(native) {{\nbb0({params}):\n  {operation}\n  return\n}}"
        );
        let source = module("permissive", &body);
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid raw heap assembled: {operation}"
            );
        }
    }
}
