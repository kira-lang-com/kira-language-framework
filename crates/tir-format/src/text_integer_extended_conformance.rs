use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(ty: &str, operation: &str) -> String {
    format!(
        "module @Integer tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Integer.run(%a: {ty}, %b: {ty}) -> {ty} engine(native) {{\nbb0(%a: {ty}, %b: {ty}):\n  %result = {operation}\n  return %result\n}}\n"
    )
}

fn assert_round_trip(source: &str) {
    let parsed = parse_text_module(source).expect("integer text parses");
    let bytes = assemble_text_module(&parsed).expect("integer CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("CODE decodes");
    let text = disassemble_text_module(&binary, u64::MAX).expect("CODE disassembles");
    let reassembled =
        assemble_text_module(&parse_text_module(&text).expect("canonical text parses"))
            .expect("canonical integer CODE reassembles");
    assert_eq!(bytes, reassembled, "integer operation must be canonical");
}

#[test]
fn default_mode_integer_division_shifts_rotates_and_minmax_round_trip() {
    let binary = [
        "sdiv", "udiv", "srem", "urem", "shl", "lshr", "ashr", "rotl", "rotr", "smin", "smax",
        "umin", "umax", "clmul", "clmulh",
    ];
    for ty in ["i32", "i64", "vec<4 x i32>"] {
        for opcode in binary {
            if ty.starts_with("vec<") && matches!(opcode, "clmul" | "clmulh") {
                continue;
            }
            assert_round_trip(&module(ty, &format!("{opcode} %a, %b")));
        }
    }
}

#[test]
fn integer_bit_operations_round_trip() {
    for ty in ["i16", "i32", "i64", "vec<4 x i32>"] {
        for opcode in ["ctlz", "cttz", "ctpop", "bswap", "bitreverse"] {
            assert_round_trip(&module(ty, &format!("{opcode} %a")));
        }
    }
}

#[test]
fn funnel_shifts_round_trip_and_require_three_operands() {
    for ty in ["i32", "i64", "vec<4 x i32>"] {
        for opcode in ["fshl", "fshr"] {
            assert_round_trip(&module(ty, &format!("{opcode} %a, %b, %a")));
            for invalid in [
                format!("{opcode} %a, %b"),
                format!("{opcode} %a, %b, %missing"),
            ] {
                let source = module(ty, &invalid);
                let parsed =
                    parse_text_module(&source).expect("malformed funnel shift text parses");
                assert!(
                    assemble_text_module(&parsed).is_err(),
                    "invalid funnel shift assembled: {invalid}"
                );
            }
        }
    }
}

#[test]
fn bswap_and_carryless_multiply_reject_invalid_widths() {
    for (ty, operation) in [
        ("i8", "bswap %a"),
        ("i24", "bswap %a"),
        ("vec<4 x i8>", "bswap %a"),
        ("i16", "clmul %a, %b"),
        ("i128", "clmulh %a, %b"),
        ("vec<2 x i16>", "clmulh %a, %b"),
        ("vec<4 x i32>", "clmul %a, %b"),
    ] {
        let source = module(ty, operation);
        let parsed = parse_text_module(&source).expect("invalid instruction text parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "invalid {ty} `{operation}` assembled"
        );
    }
}

#[test]
fn integer_mode_modifiers_are_not_silently_dropped() {
    for operation in [
        "sdiv [wrap] %a, %b",
        "shl [trap s] %a, %b",
        "lshr [exact] %a, %b",
    ] {
        let source = module("i32", operation);
        let parsed = parse_text_module(&source).expect("modifier text parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "unsupported mode was silently accepted: {operation}"
        );
    }
}

#[test]
fn exact_disjoint_and_poison_flags_round_trip_with_spec_bits() {
    for (operation, expected_bit) in [
        ("udiv [exact] %a, %b", 3),
        ("or [disjoint] %a, %b", 4),
        ("ctlz [poison] %a", 47),
        ("cttz [poison] %a", 47),
    ] {
        let source = module("i32", operation).replace("semantics defined", "semantics permissive");
        let parsed = parse_text_module(&source).expect("integer modifier text parses");
        let bytes = assemble_text_module(&parsed).expect("modifier CODE assembles");
        let binary = BinaryModule::decode(&bytes).expect("binary decodes");
        let code = binary
            .code_table(
                u64::MAX,
                &binary
                    .signature_table(u64::MAX, &binary.type_table(u64::MAX).expect("types"))
                    .expect("signatures"),
            )
            .expect("CODE");
        let first = &code.functions[0].blocks[0]
            .instructions
            .as_ref()
            .expect("instructions")[0];
        assert_eq!(first.modifiers, 1 << expected_bit);
        let canonical = disassemble_text_module(&binary, u64::MAX).expect("CODE disassembles");
        assert!(canonical.contains(operation.split(' ').next().expect("opcode")));
        let reassembled =
            assemble_text_module(&parse_text_module(&canonical).expect("text reparses"))
                .expect("canonical CODE reassembles");
        assert_eq!(bytes, reassembled);
    }
}

#[test]
fn repeated_or_wrongly_placed_integer_flags_are_rejected() {
    for operation in [
        "udiv [exact] [exact] %a, %b",
        "or [exact] %a, %b",
        "ctpop [poison] %a",
        "ctlz [disjoint] %a",
        "or %a, %b [disjoint]",
    ] {
        let source = module("i32", operation).replace("semantics defined", "semantics permissive");
        let parsed = parse_text_module(&source).expect("modifier syntax parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "invalid modifier assembled: {operation}"
        );
    }
}
