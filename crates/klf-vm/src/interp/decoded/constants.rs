use super::DecodedInstr;
use crate::bytecode::{Instr, Opcode};
use crate::program::Program;

pub(super) fn decode_constants(
    code: &mut [DecodedInstr],
    raw: &[Instr],
    program: &Program,
    register_count: u32,
) -> Vec<u64> {
    let mut constants = Vec::new();
    for instruction in code {
        let DecodedInstr::Other { low, high } = *instruction else {
            continue;
        };
        let raw = raw[super::join_u32(low, high) as usize];
        if raw.op != Opcode::LoadK {
            continue;
        }
        if u32::from(raw.a) >= register_count {
            *instruction = DecodedInstr::Invalid;
            continue;
        }
        let Ok(value) = crate::numeric::load_const(program, raw.bx()) else {
            continue;
        };
        let Ok(index) = u32::try_from(constants.len()) else {
            break;
        };
        constants.push(value);
        *instruction = DecodedInstr::LoadConstant { a: raw.a, index };
    }
    constants
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::Const;

    #[test]
    fn numeric_constants_validate_destinations_and_preserve_bits() {
        let program = Program {
            functions: vec![],
            consts: vec![Const::Int(-1), Const::Float((-0.0f64).to_bits())],
            imports: vec![],
            globals: vec![],
            initializers: vec![],
        };
        let raw = [
            Instr::from_bx(Opcode::LoadK, 0, 0),
            Instr::from_bx(Opcode::LoadK, 0, 1),
            Instr::from_bx(Opcode::LoadK, 1, 0),
            Instr::from_bx(Opcode::LoadK, 0, 2),
        ];
        let mut code = (0..4)
            .map(|low| DecodedInstr::Other { low, high: 0 })
            .collect::<Vec<_>>();
        assert_eq!(
            decode_constants(&mut code, &raw, &program, 1),
            vec![u64::MAX, (-0.0f64).to_bits()]
        );
        assert!(matches!(code[2], DecodedInstr::Invalid));
        assert!(matches!(code[3], DecodedInstr::Other { .. }));
    }
}
