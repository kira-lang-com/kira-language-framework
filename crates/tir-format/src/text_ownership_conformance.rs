#[cfg(test)]
mod ownership_tests {
    use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

    #[test]
    fn explicit_copy_move_destroy_round_trip() {
        let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.own(%a: i32, %b: i32) -> i32 engine(native) {\nbb0(%a: i32, %b: i32):\n  %dup = copy %a\n  %taken = move %dup\n  destroy %a\n  %sum = add %taken, %b\n  return %sum\n}\n";
        let parsed = parse_text_module(source).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("ownership ops assemble");
        let binary = BinaryModule::decode(&bytes).expect("binary decodes");
        let text = disassemble_text_module(&binary, u64::MAX).expect("ownership ops disassemble");
        let rebuilt =
            assemble_text_module(&parse_text_module(&text).expect("canonical text reparses"))
                .expect("canonical text reassembles");
        assert_eq!(bytes, rebuilt);
    }

    #[test]
    fn ownership_instruction_arity_is_enforced() {
        let body = "  %dup = copy %a, %b\n  destroy %dup\n  %sum = add %a, %b\n  return %sum";
        let source = format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.own(%a: i32, %b: i32) -> i32 engine(native) {{\nbb0(%a: i32, %b: i32):\n{body}\n}}\n"
        );
        let parsed = parse_text_module(&source).expect("text parses");
        assert!(assemble_text_module(&parsed).is_err());
    }
}
