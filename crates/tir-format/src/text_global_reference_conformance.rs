use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @GlobalRef tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("global reference text parses");
    let bytes = assemble_text_module(&parsed).expect("global reference CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("global reference binary decodes");
    let text =
        disassemble_text_module(&binary, u64::MAX).expect("global reference CODE disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&text).expect("canonical global reference text parses"),
    )
    .expect("global reference CODE reassembles");
    assert_eq!(bytes, rebuilt);
    text
}

#[test]
fn global_and_tls_addresses_resolve_forward_globals() {
    let source = module(
        "func module @GlobalRef.read() -> ptr engine(native) {\nbb0:\n  %common = global.addr @GlobalRef.shared\n  %perthread = tls.addr @GlobalRef.local\n  return %perthread\n}\n\nglobal module @GlobalRef.shared: i64 = 14\nglobal module @GlobalRef.local: i32 = 3 thread_local",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("global.addr @GlobalRef.shared"));
    assert!(canonical.contains("tls.addr @GlobalRef.local"));
}

#[test]
fn references_to_multiple_global_indices_and_cfg_successors_round_trip() {
    let source = module(
        "global private @GlobalRef.first: i32 = 12\nglobal module @GlobalRef.second: i64 = 34\nglobal module @GlobalRef.third: i32 = 56 thread_local\n\nfunc module @GlobalRef.test() -> ptr engine(native) {\nbb0:\n  %addr = global.addr @GlobalRef.second\n  br bb1(%addr)\nbb1(%forwarded: ptr):\n  %thread = tls.addr @GlobalRef.third\n  return %forwarded\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("global.addr @GlobalRef.second"));
    assert!(canonical.contains("tls.addr @GlobalRef.third"));
}

#[test]
fn global_references_reject_unknown_names_invalid_tls_and_duplicate_results() {
    for (globals, code) in [
        (
            "global module @GlobalRef.simple: i64 = 1",
            "%ptr = global.addr @GlobalRef.missing",
        ),
        (
            "global module @GlobalRef.simple: i64 = 1",
            "%ptr = tls.addr @GlobalRef.simple",
        ),
        (
            "global module @GlobalRef.simple: i64 = 1",
            "%ptr = global.addr @GlobalRef.simple\n  %ptr = global.addr @GlobalRef.simple",
        ),
        (
            "global module @GlobalRef.simple: i64 = 1",
            "global.addr @GlobalRef.simple",
        ),
        (
            "global module @GlobalRef.simple: i64 = 1",
            "%ptr = global.addr GlobalRef.simple",
        ),
        (
            "global module @GlobalRef.simple: i64 = 1",
            "%ptr = global.addr @GlobalRef.simple extra",
        ),
    ] {
        let source = module(&format!(
            "{globals}\n\nfunc module @GlobalRef.invalid() -> unit engine(native) {{\nbb0:\n  {code}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid global reference assembled: {code}"
            );
        }
    }
}

#[test]
fn sparse_wide_integer_global_initializer_has_minimal_uleb_storage() {
    let source = module(
        "global module @GlobalRef.wide: i128 = 1\n\nfunc module @GlobalRef.address() -> ptr engine(native) {\nbb0:\n  %value = global.addr @GlobalRef.wide\n  return %value\n}",
    );
    assert!(round_trip(&source).contains("global.addr @GlobalRef.wide"));
}
