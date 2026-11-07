use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(semantics: &str, body: &str) -> String {
    format!(
        "module @Freeze tir 26.11.1 profile app semantics {semantics}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

pub(super) fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("text parses");
    let bytes = assemble_text_module(&parsed).expect("freeze CODE assembles");
    let module = BinaryModule::decode(&bytes).expect("binary decodes");
    let canonical = disassemble_text_module(&module, u64::MAX).expect("CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("canonical CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn scalar_float_and_pointer_freeze_round_trip() {
    let float = module(
        "permissive",
        "func module @Freeze.float() -> f32 engine(native) {\nbb0:\n  %poison = const.poison f32\n  %safe = freeze %poison\n  return %safe\n}",
    );
    assert!(round_trip(&float).contains("freeze %"));

    let pointer = module(
        "defined",
        "func module @Freeze.pointer(%p: ptr<device>) -> ptr<device> engine(native) {\nbb0(%p: ptr<device>):\n  %safe = freeze %p\n  return %safe\n}",
    );
    assert!(round_trip(&pointer).contains("freeze %"));
}

#[test]
fn integer_vector_freeze_round_trip() {
    let vector = module(
        "defined",
        "func module @Freeze.vector(%v: vec<4 x i32>) -> vec<4 x i32> engine(native) {\nbb0(%v: vec<4 x i32>):\n  %safe = freeze %v\n  return %safe\n}",
    );
    assert!(round_trip(&vector).contains("freeze %"));
}

#[test]
fn freeze_rejects_address_only_stack_operand() {
    let source = module(
        "defined",
        "func module @Freeze.bad() -> unit engine(native) {\nbb0:\n  %slot = stack.alloc i32\n  %bad = freeze %slot\n  stack.dealloc %slot\n  return\n}",
    );
    let parsed = parse_text_module(&source).expect("text parses");
    assert!(
        assemble_text_module(&parsed).is_err(),
        "freeze must not materialize an address-only SSA value"
    );
}
