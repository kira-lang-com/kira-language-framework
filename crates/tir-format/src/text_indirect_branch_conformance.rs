use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @IndirectBr tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("indirect branch source parses");
    let encoded =
        assemble_text_module(&parsed).expect("block address + indirect branch CODE assembles");
    let binary = BinaryModule::decode(&encoded).expect("indirect branch binary decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("indirect branch CODE reassembles");
    assert_eq!(encoded, rebuilt);
    canonical
}

#[test]
fn block_addr_and_indirect_branch_round_trip() {
    let source = module(
        "func module @IndirectBr.f() -> unit engine(native) {\nbb0:\n  %address = block.addr bb2\n  br.indirect %address, [bb2, bb1]\nbb1:\n  return\nbb2:\n  return\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("block.addr bb2"));
    assert!(canonical.contains("br.indirect %"));
}

#[test]
fn block_address_is_an_ssa_value_that_may_dominate_a_later_block() {
    let source = module(
        "func module @IndirectBr.f(%condition: i1) -> unit engine(native) {\nbb0(%condition: i1):\n  %address = block.addr bb2\n  cond.br %condition, bb1(), bb2()\nbb1:\n  br.indirect %address, [bb2]\nbb2:\n  return\n}",
    );
    assert!(round_trip(&source).contains("br.indirect %"));
}

#[test]
fn block_addr_and_indirect_branch_reject_unsafe_provenance_and_targets() {
    let examples = [
        "func module @IndirectBr.bad(%p: ptr) -> unit engine(native) {\nbb0(%p: ptr):\n  br.indirect %p, [bb1]\nbb1:\n  return\n}",
        "func module @IndirectBr.bad() -> unit engine(native) {\nbb0:\n  %address = block.addr bb2\n  br.indirect %address, [bb1]\nbb1:\n  return\nbb2:\n  return\n}",
        "func module @IndirectBr.bad() -> unit engine(native) {\nbb0:\n  %address = block.addr bb2\n  br.indirect %address, [bb2, bb2]\nbb1:\n  return\nbb2:\n  return\n}",
        "func module @IndirectBr.bad() -> unit engine(native) {\nbb0:\n  %address = block.addr bb2\n  br.indirect %address, []\nbb1:\n  return\nbb2:\n  return\n}",
        "func module @IndirectBr.bad() -> unit engine(native) {\nbb0:\n  %address = block.addr bb3\n  br.indirect %address, [bb1]\nbb1:\n  return\n}",
        "func module @IndirectBr.bad() -> unit engine(native) {\nbb0:\n  %address = block.addr bb1\n  br.indirect %address, [bb1]\nbb1(%arg: i32):\n  return\n}",
        "func module @IndirectBr.bad() -> ptr engine(native) {\nbb0:\n  %address = block.addr bb0\n  return %address\n}",
        "func module @IndirectBr.bad() -> unit engine(native) {\nbb0:\n  %address = block.addr bb1\n  br.indirect %address, [bb1]\nbb1:\n  %copy = copy %address\n  return\n}",
    ];
    for body in examples {
        let source = module(body);
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid indirect branch assembled: {body}"
            );
        }
    }
}
