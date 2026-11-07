use super::decode_instruction;
use crate::leb::write_uleb;
use crate::strings::StringTableBuilder;
use crate::types::{BinaryConstantTable, BinaryFunctionType, BinaryTypeTable};

fn decodes(opcode: u64, modifiers: u64, arity: usize) -> bool {
    let mut bytes = Vec::new();
    for value in [opcode, modifiers, 1] {
        write_uleb(value, &mut bytes).expect("valid instruction header");
    }
    for _ in 0..arity {
        write_uleb(1, &mut bytes).expect("valid SSA value delta");
    }
    let function = BinaryFunctionType {
        parameters: Vec::new(),
        result: 0,
        error: None,
        asynchronous: false,
        c_abi: false,
        foreign: false,
    };
    let mut cursor = 0;
    matches!(
        decode_instruction(
            &bytes,
            &mut cursor,
            0,
            2,
            1,
            &function,
            &BinaryTypeTable::default(),
            &BinaryConstantTable::default(),
            &StringTableBuilder::new().finish(),
            0,
        ),
        Ok(Some(_))
    )
}

#[test]
fn integer_flag_bits_are_accepted_only_on_their_defined_opcodes() {
    for (opcode, bit, arity) in [(0x028, 3, 2), (0x02c, 4, 2), (0x03e, 47, 1), (0x03f, 47, 1)] {
        assert!(decodes(opcode, 1u64 << bit, arity));
        assert!(!decodes(opcode, (1u64 << bit) | (1 << 5), arity));
    }
    for (opcode, modifiers, arity) in [
        (0x028, 1 << 4, 2),
        (0x02c, 1 << 3, 2),
        (0x03e, 1 << 3, 1),
        (0x03f, 1 << 4, 1),
        (0x040, 1 << 47, 1),
        (0x03d, 1 << 4, 3),
    ] {
        assert!(!decodes(opcode, modifiers, arity));
    }
}
