use super::super::decoded::{DecodedCondition, DecodedInstr, DecodedLoopPlan};
use super::super::frames::Frame;
use super::{apply_branch_edge, read_register, write_register};
use crate::bytecode::{Instr, Opcode, WidthCode};
use crate::numeric::{float_binary, float_compare, float_fma, float_unary};

#[inline(always)]
pub(super) fn execute_integer_cast(a: u16, b: u16, control: u16, registers: &mut [u64]) {
    let value = read_register(registers, b);
    let bits = control & 0x7f;
    let value = if control & 0x80 == 0 {
        value & (u64::MAX >> (64 - bits))
    } else {
        let shift = 64 - bits;
        (((value << shift) as i64 >> shift) as u64) & (u64::MAX >> (64 - (control >> 8)))
    };
    write_register(registers, a, value);
}

#[inline(always)]
pub(super) fn execute_select(a: u16, condition: u16, if_true: u16, registers: &mut [u64]) {
    let source = if read_register(registers, condition) & 1 == 1 {
        if_true
    } else {
        if_true + 1
    };
    let value = read_register(registers, source);
    write_register(registers, a, value);
}

pub(super) fn execute_float_instruction(instruction: Instr, registers: &mut [u64]) {
    let read = |index| read_register(registers, index);
    let value = match instruction.op {
        Opcode::FAdd
        | Opcode::FSub
        | Opcode::FMul
        | Opcode::FDiv
        | Opcode::FRem
        | Opcode::FMin
        | Opcode::FMax => float_binary(
            instruction.op,
            read(instruction.b),
            read(instruction.c),
            instruction.flags & 1 == 1,
        ),
        Opcode::FNeg
        | Opcode::FAbs
        | Opcode::FSqrt
        | Opcode::FFloor
        | Opcode::FCeil
        | Opcode::FTrunc
        | Opcode::FRound => float_unary(
            instruction.op,
            read(instruction.b),
            instruction.flags & 1 == 1,
        ),
        Opcode::FCmp => u64::from(float_compare(
            instruction.flags >> 1,
            read(instruction.b),
            read(instruction.c),
            instruction.flags & 1 == 1,
        )),
        Opcode::FFma => float_fma(
            read(instruction.b),
            read(instruction.c),
            read(instruction.c + 1),
            instruction.flags & 1 == 1,
        ),
        _ => unreachable!("decoded float opcode"),
    };
    write_register(registers, instruction.a, value);
}

pub(super) fn float_raw_index(instruction: DecodedInstr) -> Option<u32> {
    match instruction {
        DecodedInstr::FloatBinary(index)
        | DecodedInstr::FloatUnary(index)
        | DecodedInstr::FloatCompare(index)
        | DecodedInstr::FloatFma(index) => Some(index),
        _ => None,
    }
}

pub(super) fn execute_numeric_loop(frame: &mut Frame, plan: &DecodedLoopPlan) -> usize {
    let DecodedCondition::IntegerCompare {
        result,
        lhs,
        rhs,
        width,
        predicate,
    } = plan.condition
    else {
        unreachable!("numeric loop plans have integer comparison conditions")
    };
    loop {
        let matched = integer_compare_fast(
            predicate,
            read_register(&frame.registers, lhs),
            read_register(&frame.registers, rhs),
            width,
        );
        write_register(&mut frame.registers, result, u64::from(matched));
        if matched == plan.body_when_matched {
            apply_branch_edge(frame, &plan.body_edge);
            for instruction in &plan.body {
                execute_numeric_instruction(&mut frame.registers, *instruction);
            }
            apply_branch_edge(frame, &plan.back_edge);
        } else {
            apply_branch_edge(frame, &plan.exit_edge);
            return plan.exit_edge.target;
        }
    }
}

fn execute_numeric_instruction(registers: &mut [u64], instruction: DecodedInstr) {
    match instruction {
        DecodedInstr::Add64 { a, b, c } => {
            let value = read_register(registers, b).wrapping_add(read_register(registers, c));
            write_register(registers, a, value);
        }
        DecodedInstr::Sub64 { a, b, c } => {
            let value = read_register(registers, b).wrapping_sub(read_register(registers, c));
            write_register(registers, a, value);
        }
        DecodedInstr::Mul64 { a, b, c } => {
            let value = read_register(registers, b).wrapping_mul(read_register(registers, c));
            write_register(registers, a, value);
        }
        _ => {
            let (a, b, c, predicate) = integer_compare_operands(instruction)
                .expect("numeric loop plans contain only numeric instructions");
            let matched = integer_compare_fast(
                predicate,
                read_register(registers, b),
                read_register(registers, c),
                WidthCode::W64,
            );
            write_register(registers, a, u64::from(matched));
        }
    }
}

pub(super) fn integer_compare_operands(instruction: DecodedInstr) -> Option<(u16, u16, u16, u8)> {
    let (a, b, c, predicate) = match instruction {
        DecodedInstr::IcmpEq64 { a, b, c } => (a, b, c, 0),
        DecodedInstr::IcmpNe64 { a, b, c } => (a, b, c, 1),
        DecodedInstr::IcmpSlt64 { a, b, c } => (a, b, c, 2),
        DecodedInstr::IcmpSle64 { a, b, c } => (a, b, c, 3),
        DecodedInstr::IcmpSgt64 { a, b, c } => (a, b, c, 4),
        DecodedInstr::IcmpSge64 { a, b, c } => (a, b, c, 5),
        DecodedInstr::IcmpUlt64 { a, b, c } => (a, b, c, 6),
        DecodedInstr::IcmpUle64 { a, b, c } => (a, b, c, 7),
        DecodedInstr::IcmpUgt64 { a, b, c } => (a, b, c, 8),
        DecodedInstr::IcmpUge64 { a, b, c } => (a, b, c, 9),
        _ => return None,
    };
    Some((a, b, c, predicate))
}

#[inline(always)]
pub(super) fn integer_compare_fast(predicate: u8, lhs: u64, rhs: u64, width: WidthCode) -> bool {
    if width != WidthCode::W64 {
        return crate::numeric::integer_compare(predicate, lhs, rhs, width);
    }
    match predicate {
        0 => lhs == rhs,
        1 => lhs != rhs,
        2 => (lhs as i64) < (rhs as i64),
        3 => (lhs as i64) <= (rhs as i64),
        4 => (lhs as i64) > (rhs as i64),
        5 => (lhs as i64) >= (rhs as i64),
        6 => lhs < rhs,
        7 => lhs <= rhs,
        8 => lhs > rhs,
        _ => lhs >= rhs,
    }
}

#[inline(always)]
pub(super) fn execute_float_pair<const METERED: bool>(
    pair: super::super::decoded::FloatMultiplyAdd,
    registers: &mut [u64],
    pc: &mut usize,
) {
    let left = f64::from_bits(read_register(registers, pair.multiply_b));
    let right = f64::from_bits(read_register(registers, pair.multiply_c));
    write_register(registers, pair.multiply_a, (left * right).to_bits());
    *pc += 1;
    if !METERED {
        let left = f64::from_bits(read_register(registers, pair.add_b));
        let right = f64::from_bits(read_register(registers, pair.add_c));
        write_register(registers, pair.add_a, (left + right).to_bits());
        *pc += 1;
    }
}

#[cfg(test)]
mod cast_tests {
    use super::*;
    use crate::numeric::convert_integer;

    #[test]
    fn predecoded_casts_match_every_scalar_width() {
        for source in 0..4u8 {
            for dest in 0..4u8 {
                let source_bits = 8u16 << source;
                let dest_bits = 8u16 << dest;
                for op in [Opcode::Trunc, Opcode::ZExt, Opcode::SExt] {
                    let control = match op {
                        Opcode::Trunc => source_bits.min(dest_bits),
                        Opcode::ZExt => source_bits,
                        _ => source_bits | 0x80 | (dest_bits << 8),
                    };
                    for value in [0, 127, 128, 255, 256, 0x1234_5678_9abc_def0, u64::MAX] {
                        let mut registers = [value, 0];
                        execute_integer_cast(1, 0, control, &mut registers);
                        assert_eq!(
                            registers[1],
                            convert_integer(op, value, source | (dest << 3)).unwrap()
                        );
                        execute_integer_cast(0, 0, control, &mut registers);
                        assert_eq!(registers[0], registers[1]);
                    }
                }
            }
        }
    }
}
