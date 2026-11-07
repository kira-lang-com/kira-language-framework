#[cfg(test)]
mod tests {
    use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

    fn module_with_body(body: &str) -> String {
        format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.chain(%a: i32, %b: i32) -> i32 engine(native) {{\nbb0(%a: i32, %b: i32):\n{body}\n}}\n"
        )
    }

    #[test]
    fn integer_chain_is_canonical_after_disassembly() {
        let source = module_with_body(
            "  %sum = add %a, %b\n  %product = mul %sum, %b\n  %result = sub %product, %a\n  return %result",
        );
        let text = parse_text_module(&source).expect("text parses");
        let bytes = assemble_text_module(&text).expect("chain assembles");
        let module = BinaryModule::decode(&bytes).expect("binary decodes");
        let disassembled = disassemble_text_module(&module, u64::MAX).expect("chain disassembles");
        let round_trip =
            assemble_text_module(&parse_text_module(&disassembled).expect("text reparses"))
                .expect("binary reassembles");
        assert_eq!(bytes, round_trip);
    }

    #[test]
    fn intermediate_icmp_i1_type_is_interned_and_round_trips() {
        let source = module_with_body(
            "  %equal = icmp eq %a, %b\n  %sum = add %a, %b\n  %result = select %equal, %sum, %a\n  return %result",
        );
        let parsed = parse_text_module(&source).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("icmp chain assembles");
        let module = BinaryModule::decode(&bytes).expect("binary decodes");
        let types = module.type_table(u64::MAX).expect("type table decodes");
        assert!(
            types
                .entries
                .iter()
                .any(|ty| matches!(ty, crate::BinaryType::Integer(1)))
        );
        let text = disassemble_text_module(&module, u64::MAX).expect("icmp chain disassembles");
        let reassembled = assemble_text_module(&parse_text_module(&text).expect("text parses"))
            .expect("round trip assembles");
        assert_eq!(bytes, reassembled);
    }

    #[test]
    fn mixed_constants_arithmetic_and_compare_round_trip() {
        let source = module_with_body(
            "  %five = const.int i32, 5\n  %sum = add %a, %five\n  %same = icmp eq %sum, %b\n  %flag = const.bool true\n  %keep = and %same, %flag\n  %value = select %keep, %sum, %b\n  return %value",
        );
        let parsed = parse_text_module(&source).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("mixed function assembles");
        let decoded = BinaryModule::decode(&bytes).expect("binary decodes");
        let text = disassemble_text_module(&decoded, u64::MAX).expect("binary disassembles");
        let reassembled = assemble_text_module(&parse_text_module(&text).expect("text parses"))
            .expect("binary reassembles");
        assert_eq!(bytes, reassembled);
    }

    #[test]
    fn undefined_or_mismatched_chain_inputs_are_rejected() {
        for body in [
            "  %sum = add %a, %missing\n  %product = mul %sum, %b\n  return %product",
            "  %sum = icmp eq %a, %b\n  %product = add %sum, %a\n  return %product",
            "  %sum = add %a, %b\n  %sum = mul %sum, %b\n  return %sum",
        ] {
            let text = parse_text_module(&module_with_body(body)).expect("text parses");
            assert!(
                assemble_text_module(&text).is_err(),
                "invalid body assembled: {body}"
            );
        }
    }
}
