use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Indirect tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("indirect-call text parses");
    let bytes = assemble_text_module(&parsed).expect("indirect-call CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("indirect-call CODE decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("indirect-call CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("indirect-call CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn indirect_call_through_typed_function_value_round_trips() {
    let body = "func module @Indirect.caller(%callee: fn(i32) -> i32, %value: i32) -> i32 engine(native) {\nbb0(%callee: fn(i32) -> i32, %value: i32):\n  %out = call.indirect %callee : fn(i32) -> i32 (%value)\n  return %out\n}";
    let canonical = round_trip(&module(body));
    assert!(canonical.contains(" = call.indirect %"));
}

#[test]
fn unit_indirect_call_with_no_arguments_round_trips() {
    let body = "func module @Indirect.unit(%callee: fn() -> unit) -> unit engine(native) {\nbb0(%callee: fn() -> unit):\n  call.indirect %callee : fn() -> unit ()\n  return\n}";
    let canonical = round_trip(&module(body));
    assert!(canonical.contains("call.indirect %"));
}

#[test]
fn function_values_can_cross_blocks_before_indirect_call() {
    let body = "func module @Indirect.forward(%callee: fn(i64) -> i64, %v: i64) -> i64 engine(native) {\nbb0(%callee: fn(i64) -> i64, %v: i64):\n  br bb1(%callee, %v)\nbb1(%f: fn(i64) -> i64, %x: i64):\n  %out = call.indirect %f : fn(i64) -> i64 (%x)\n  return %out\n}";
    let canonical = round_trip(&module(body));
    assert!(canonical.contains("call.indirect"));
}

#[test]
fn mismatched_function_type_arguments_and_results_are_rejected() {
    for body in [
        "func module @Indirect.bad(%f: fn(i32) -> i32, %x: i64) -> i32 engine(native) {\nbb0(%f: fn(i32) -> i32, %x: i64):\n  %out = call.indirect %f : fn(i32) -> i32 (%x)\n  return %out\n}",
        "func module @Indirect.bad(%f: fn(i32) -> i32, %x: i32) -> i32 engine(native) {\nbb0(%f: fn(i32) -> i32, %x: i32):\n  %out = call.indirect %f : fn(i64) -> i32 (%x)\n  return %out\n}",
        "func module @Indirect.bad(%f: fn(i32) -> i32) -> unit engine(native) {\nbb0(%f: fn(i32) -> i32):\n  call.indirect %f : fn(i32) -> i32 ()\n  return\n}",
        "func module @Indirect.bad(%f: fn() -> unit) -> unit engine(native) {\nbb0(%f: fn() -> unit):\n  %out = call.indirect %f : fn() -> unit ()\n  return\n}",
        "func module @Indirect.bad(%p: ptr) -> unit engine(native) {\nbb0(%p: ptr):\n  call.indirect %p : fn() -> unit ()\n  return\n}",
    ] {
        let source = module(body);
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "mismatched indirect call assembled: {body}"
            );
        }
    }
}

#[test]
fn distinct_indirect_call_typeids_are_sorted_deduplicated_and_round_trip() {
    let source = module(
        "func module @Indirect.small(%f: borrow fn(i32) -> i32, %x: i32) -> i32 engine(native) {\nbb0(%f: borrow fn(i32) -> i32, %x: i32):\n  %a = call.indirect %f : fn(i32) -> i32 (%x)\n  %b = call.indirect %f : fn(i32) -> i32 (%a)\n  return %b\n}\n\nfunc module @Indirect.large(%f: borrow fn(i64) -> i64, %x: i64) -> i64 engine(native) {\nbb0(%f: borrow fn(i64) -> i64, %x: i64):\n  %result = call.indirect %f : fn(i64) -> i64 (%x)\n  return %result\n}",
    );
    let parsed = parse_text_module(&source).expect("indirect call text parses");
    let bytes = assemble_text_module(&parsed).expect("multiple indirect calls assemble");
    let binary = BinaryModule::decode(&bytes).expect("binary decodes");
    let analysis = binary.analysis_summary(u64::MAX).expect("ANALYSIS decodes");
    assert_eq!(analysis.indirect_call_typeids.len(), 2);
    assert!(analysis.indirect_call_typeids[0] < analysis.indirect_call_typeids[1]);
    let text = round_trip(&source);
    assert_eq!(text.matches("call.indirect").count(), 3);
}

#[test]
fn disassembler_refuses_incorrect_indirect_call_typeid_inventory() {
    let source = module(
        "func module @Indirect.run(%f: borrow fn(i32) -> i32, %x: i32) -> i32 engine(native) {\nbb0(%f: borrow fn(i32) -> i32, %x: i32):\n  %result = call.indirect %f : fn(i32) -> i32 (%x)\n  return %result\n}",
    );
    let parsed = parse_text_module(&source).expect("indirect source parses");
    let bytes = assemble_text_module(&parsed).expect("indirect CODE assembles");
    let mut binary = BinaryModule::decode(&bytes).expect("module decodes");
    let section = binary
        .sections
        .iter_mut()
        .find(|section| section.kind == 14)
        .expect("ANALYSIS section exists");
    let mut payload = section
        .read_uncompressed(u64::MAX)
        .expect("ANALYSIS payload decodes");
    assert_eq!(&payload[0..5], &[1, 0, 0, 0, 1]);
    payload[5] ^= 0x80;
    *section = crate::Section::from_uncompressed(14, payload);
    assert!(
        disassemble_text_module(&binary, u64::MAX).is_err(),
        "disassembly must not drop a mismatched indirect-call typeid"
    );
}

#[test]
fn indirect_call_site_attributes_round_trip_and_preserve_analysis() {
    let source = module(
        "func module @Indirect.caller(%fn: borrow fn(i32) -> i32, %x: i32) -> i32 engine(native) {\nbb0(%fn: borrow fn(i32) -> i32, %x: i32):\n  %result = call.indirect [noinline] [cold] %fn : fn(i32) -> i32 (%x)\n  return %result\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("call.indirect [cold] [noinline] %"));
}
