use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @ManagedHeap tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("managed heap text parses");
    let bytes = assemble_text_module(&parsed).expect("managed heap CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("managed heap binary decodes");
    let text = disassemble_text_module(&binary, u64::MAX).expect("managed heap CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&text).expect("canonical managed heap parses"))
            .expect("managed heap CODE reassembles");
    assert_eq!(bytes, rebuilt);
    text
}

#[test]
fn counted_and_unique_heap_allocation_round_trip_with_owned_destroy() {
    let source = module(
        "func module @ManagedHeap.allocate() -> unit engine(native) {\nbb0:\n  %reference = heap.alloc.counted i32\n  %unique = heap.alloc.unique i64\n  destroy %reference\n  destroy %unique\n  return\n}",
    );
    let text = round_trip(&source);
    assert!(text.contains("heap.alloc.counted i32"));
    assert!(text.contains("heap.alloc.unique i64"));
}

#[test]
fn managed_reference_addresses_and_unowned_queries_round_trip() {
    let source = module(
        "func module @ManagedHeap.access(%r: borrow ref i32, %u: borrow uniq i64) -> i1 engine(native) {\nbb0(%r: borrow ref i32, %u: borrow uniq i64):\n  %a = ref.addr %r\n  %b = uniq.addr %u\n  %p = ref.unowned %r\n  %unique = ref.isunique %r\n  return %unique\n}",
    );
    let canonical = round_trip(&source);
    for opcode in [
        "ref.addr %",
        "uniq.addr %",
        "ref.unowned %",
        "ref.isunique %",
    ] {
        assert!(canonical.contains(opcode), "missing {opcode}");
    }
}

#[test]
fn managed_heap_calls_can_be_interleaved_with_blocks() {
    let source = module(
        "func module @ManagedHeap.allocate() -> uniq i32 engine(native) {\nbb0:\n  %object = heap.alloc.unique i32\n  br bb1(%object)\nbb1(%forwarded: owned uniq i32):\n  return %forwarded\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("heap.alloc.unique i32"));
}

#[test]
fn managed_heap_rejects_invalid_argument_and_result_shapes() {
    for (params, expression) in [
        ("", "heap.alloc.counted i32"),
        ("", "%result = heap.alloc.unique i32 [uninit]"),
        ("", "%result = heap.alloc.counted unit"),
        ("", "%result = heap.alloc.unique never"),
        ("%r: ref i32", "%a = uniq.addr %r"),
        ("%r: ref i32", "%a = ref.addr %missing"),
        ("%p: ptr", "%a = ref.unowned %p"),
        ("%u: uniq i32", "%a = ref.isunique %u"),
        ("%r: ref i32", "ref.isunique %r"),
        ("", "%result = heap.alloc.counted %UnknownType"),
    ] {
        let source = module(&format!(
            "func module @ManagedHeap.invalid({params}) -> unit engine(native) {{\nbb0({params}):\n  {expression}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid managed heap opcode assembled: {expression}"
            );
        }
    }
}

#[test]
fn weak_new_and_optional_weak_load_round_trip() {
    let source = module(
        "type %Optional<$T> = enum ridl { None, Some($T) } policy { copy: implicit(@ManagedHeap.optional_copy), destroy: fn(@ManagedHeap.optional_destroy), relocate: bitwise, exclusivity: static, pairing: counted, lifetime: eager }\n\nfunc package @ManagedHeap.optional_copy(%dst: ptr, %src: ptr, %type: ptr) -> unit engine(native)\nfunc package @ManagedHeap.optional_destroy(%object: ptr, %type: ptr) -> unit engine(native)\n\nfunc module @ManagedHeap.create(%strong: borrow ref i32) -> weak i32 engine(native) {\nbb0(%strong: borrow ref i32):\n  %weak = weak.new %strong\n  return %weak\n}\n\nfunc module @ManagedHeap.promote(%weak: borrow weak i32) -> %Optional<ref i32> engine(native) {\nbb0(%weak: borrow weak i32):\n  %value = weak.load %weak\n  return %value\n}",
    );
    let text = round_trip(&source);
    assert!(text.contains("weak.new %"));
    assert!(text.contains("weak.load %"));
}

#[test]
fn weak_reference_operations_reject_mismatched_input_types() {
    for (parameters, expression) in [
        ("%r: uniq i32", "%result = weak.new %r"),
        ("%r: ref i32", "%result = weak.load %r"),
        ("%r: weak i32", "%result = weak.new %r"),
        ("%r: weak i32", "%result = weak.load %missing"),
        ("%r: weak i32", "weak.load %r"),
    ] {
        let source = module(&format!(
            "func module @ManagedHeap.invalid({parameters}) -> unit engine(native) {{\nbb0({parameters}):\n  {expression}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid weak opcode assembled: {expression}"
            );
        }
    }
}

#[test]
fn weak_load_rejects_optional_enum_without_counted_lifecycle() {
    let source = module(
        "type %Optional<$T> = enum ridl { None, Some($T) } policy default unmanaged\n\nfunc module @ManagedHeap.invalid(%weak: borrow weak i32) -> %Optional<ref i32> engine(native) {\nbb0(%weak: borrow weak i32):\n  %strong = weak.load %weak\n  return %strong\n}",
    );
    let parsed = parse_text_module(&source).expect("unmanaged Optional type syntax parses");
    assert!(
        assemble_text_module(&parsed).is_err(),
        "weak.load may not silently drop a counted reference inside a trivial Optional"
    );
}
