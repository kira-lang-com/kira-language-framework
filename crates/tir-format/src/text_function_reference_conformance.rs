use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @FunctionRef tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("function-reference text parses");
    let bytes = assemble_text_module(&parsed).expect("func.ref CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("func.ref CODE decodes");
    let text = disassemble_text_module(&binary, u64::MAX).expect("func.ref CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&text).expect("canonical func.ref parses"))
            .expect("canonical func.ref reassembles");
    assert_eq!(bytes, rebuilt);
    text
}

#[test]
fn forward_func_ref_can_be_invoked_with_call_indirect() {
    let source = module(
        "func module @FunctionRef.main(%x: i32) -> i32 engine(native) {\nbb0(%x: i32):\n  %target = func.ref @FunctionRef.double\n  %result = call.indirect %target : fn(i32) -> i32 (%x)\n  return %result\n}\n\nfunc module @FunctionRef.double(%v: i32) -> i32 engine(native) {\nbb0(%v: i32):\n  %sum = add %v, %v\n  return %sum\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains(" = func.ref @FunctionRef.double"));
    assert!(canonical.contains(" = call.indirect %"));
}

#[test]
fn func_ref_to_unit_function_round_trips() {
    let source = module(
        "func module @FunctionRef.main() -> unit engine(native) {\nbb0:\n  %target = func.ref @FunctionRef.sink\n  call.indirect %target : fn() -> unit ()\n  return\n}\n\nfunc module @FunctionRef.sink() -> unit engine(native) {\nbb0:\n  return\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("call.indirect %"));
}

#[test]
fn func_ref_may_be_forwarded_to_later_block() {
    let source = module(
        "func module @FunctionRef.main(%x: i32) -> i32 engine(native) {\nbb0(%x: i32):\n  %target = func.ref @FunctionRef.identity\n  br bb1(%target, %x)\nbb1(%callee: fn(i32) -> i32, %v: i32):\n  %result = call.indirect %callee : fn(i32) -> i32 (%v)\n  return %result\n}\n\nfunc module @FunctionRef.identity(%v: i32) -> i32 engine(native) {\nbb0(%v: i32):\n  return %v\n}",
    );
    assert!(round_trip(&source).contains("func.ref @FunctionRef.identity"));
}

#[test]
fn func_ref_rejects_unknown_names_invalid_syntax_and_result_counts() {
    for instruction in [
        "%target = func.ref @FunctionRef.unknown",
        "func.ref @FunctionRef.sink",
        "%first, %second = func.ref @FunctionRef.sink",
        "%target = func.ref @FunctionRef.sink<>",
    ] {
        let source = module(&format!(
            "func module @FunctionRef.main() -> unit engine(native) {{\nbb0:\n  {instruction}\n  return\n}}\n\nfunc module @FunctionRef.sink() -> unit engine(native) {{\nbb0:\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "accepted malformed func.ref: {instruction}"
            );
        }
    }
}
