use super::super::step::is_memory;
use super::{DecodedBranchSite, DecodedCondition, DecodedInstr};
use crate::bytecode::{Instr, Opcode, WidthCode, flags_width};
use crate::program::AllocShape;
pub(super) fn decode_instruction(
    instruction: Instr,
    branches: &[Option<DecodedBranchSite>],
    register_count: u32,
    allocations: &[AllocShape],
    call_site_count: usize,
    global_count: usize,
    raw_instructions: &mut Vec<Instr>,
) -> DecodedInstr {
    let valid_abc = [instruction.a, instruction.b, instruction.c]
        .iter()
        .all(|register| u32::from(*register) < register_count);
    match instruction.op {
        Opcode::Mov
            if u32::from(instruction.a) < register_count
                && u32::from(instruction.b) < register_count =>
        {
            DecodedInstr::Mov {
                a: instruction.a,
                b: instruction.b,
            }
        }
        Opcode::LoadI | Opcode::LeaFn if u32::from(instruction.a) < register_count => {
            let (low, high) = split_u32(instruction.bx() as u32);
            DecodedInstr::LoadI {
                a: instruction.a,
                low,
                high,
            }
        }
        Opcode::LoadNull if u32::from(instruction.a) < register_count => {
            DecodedInstr::LoadNull(instruction.a)
        }
        Opcode::Trunc | Opcode::ZExt | Opcode::SExt
            if u32::from(instruction.a) < register_count
                && u32::from(instruction.b) < register_count =>
        {
            let source = flags_width(instruction.flags).map(WidthCode::bits);
            let dest = flags_width(instruction.flags >> 3).map(WidthCode::bits);
            match (source, dest) {
                (Some(source), Some(dest)) if source <= 64 && dest <= 64 => {
                    let control = match instruction.op {
                        Opcode::Trunc => source.min(dest),
                        Opcode::ZExt => source,
                        _ => source | 0x80 | (dest << 8),
                    };
                    DecodedInstr::IntegerCast {
                        a: instruction.a,
                        b: instruction.b,
                        control: control as u16,
                    }
                }
                _ => decode_raw(instruction, false, raw_instructions),
            }
        }
        Opcode::Select
            if valid_abc
                && instruction
                    .c
                    .checked_add(1)
                    .is_some_and(|index| u32::from(index) < register_count) =>
        {
            DecodedInstr::Select {
                a: instruction.a,
                condition: instruction.b,
                if_true: instruction.c,
            }
        }
        Opcode::Ret => DecodedInstr::Return,
        Opcode::RetV if u32::from(instruction.a) < register_count => {
            DecodedInstr::ReturnValue(instruction.a)
        }
        Opcode::Nop | Opcode::Fence => DecodedInstr::Nop,
        Opcode::Load | Opcode::Store
            if u32::from(instruction.a) < register_count
                && u32::from(instruction.b) < register_count =>
        {
            match (instruction.op, flags_width(instruction.flags)) {
                (Opcode::Load, Some(WidthCode::W8)) => DecodedInstr::Load8 {
                    a: instruction.a,
                    b: instruction.b,
                },
                (Opcode::Load, Some(WidthCode::W16)) => DecodedInstr::Load16 {
                    a: instruction.a,
                    b: instruction.b,
                },
                (Opcode::Load, Some(WidthCode::W32)) => DecodedInstr::Load32 {
                    a: instruction.a,
                    b: instruction.b,
                },
                (Opcode::Load, Some(WidthCode::W64)) => DecodedInstr::Load64 {
                    a: instruction.a,
                    b: instruction.b,
                },
                (Opcode::Store, Some(WidthCode::W8)) => DecodedInstr::Store8 {
                    a: instruction.a,
                    b: instruction.b,
                },
                (Opcode::Store, Some(WidthCode::W16)) => DecodedInstr::Store16 {
                    a: instruction.a,
                    b: instruction.b,
                },
                (Opcode::Store, Some(WidthCode::W32)) => DecodedInstr::Store32 {
                    a: instruction.a,
                    b: instruction.b,
                },
                (Opcode::Store, Some(WidthCode::W64)) => DecodedInstr::Store64 {
                    a: instruction.a,
                    b: instruction.b,
                },
                _ => decode_raw(instruction, true, raw_instructions),
            }
        }
        Opcode::Add
            if valid_abc
                && instruction.flags & 0b111 == 3
                && instruction.flags & 0b11_0000 == 0 =>
        {
            DecodedInstr::Add64 {
                a: instruction.a,
                b: instruction.b,
                c: instruction.c,
            }
        }
        Opcode::Sub
            if valid_abc
                && instruction.flags & 0b111 == 3
                && instruction.flags & 0b11_0000 == 0 =>
        {
            DecodedInstr::Sub64 {
                a: instruction.a,
                b: instruction.b,
                c: instruction.c,
            }
        }
        Opcode::Mul
            if valid_abc
                && instruction.flags & 0b111 == 3
                && instruction.flags & 0b11_0000 == 0 =>
        {
            DecodedInstr::Mul64 {
                a: instruction.a,
                b: instruction.b,
                c: instruction.c,
            }
        }
        Opcode::PtrAdd if valid_abc => DecodedInstr::Add64 {
            a: instruction.a,
            b: instruction.b,
            c: instruction.c,
        },
        Opcode::PtrDiff if valid_abc => DecodedInstr::Sub64 {
            a: instruction.a,
            b: instruction.b,
            c: instruction.c,
        },
        Opcode::Icmp if valid_abc && instruction.flags & 0b111 == 3 => {
            decode_integer_compare(instruction)
        }
        Opcode::FAdd
        | Opcode::FSub
        | Opcode::FMul
        | Opcode::FDiv
        | Opcode::FRem
        | Opcode::FMin
        | Opcode::FMax
            if valid_abc =>
        {
            decode_float(instruction, FloatKind::Binary, raw_instructions)
        }
        Opcode::FNeg
        | Opcode::FAbs
        | Opcode::FSqrt
        | Opcode::FFloor
        | Opcode::FCeil
        | Opcode::FTrunc
        | Opcode::FRound
            if u32::from(instruction.a) < register_count
                && u32::from(instruction.b) < register_count =>
        {
            decode_float(instruction, FloatKind::Unary, raw_instructions)
        }
        Opcode::FCmp if valid_abc => {
            decode_float(instruction, FloatKind::Compare, raw_instructions)
        }
        Opcode::FFma
            if valid_abc
                && instruction
                    .c
                    .checked_add(1)
                    .is_some_and(|index| u32::from(index) < register_count) =>
        {
            decode_float(instruction, FloatKind::Fma, raw_instructions)
        }
        Opcode::Jmp => {
            let (low, high) = split_u32(instruction.bx() as u32);
            DecodedInstr::Jmp { low, high }
        }
        Opcode::Jif if u32::from(instruction.a) < register_count => {
            let (low, high) = split_u32(instruction.bx() as u32);
            DecodedInstr::Jif {
                condition: instruction.a,
                low,
                high,
            }
        }
        Opcode::Jifn if u32::from(instruction.a) < register_count => {
            let (low, high) = split_u32(instruction.bx() as u32);
            DecodedInstr::Jifn {
                condition: instruction.a,
                low,
                high,
            }
        }
        Opcode::Jif | Opcode::Jifn => DecodedInstr::Invalid,
        Opcode::Br if branch_site(branches, instruction.bx(), BranchKind::Jump) => {
            let (low, high) = split_u32(instruction.bx() as u32);
            DecodedInstr::Branch { low, high }
        }
        Opcode::BrIf if branch_site(branches, instruction.bx(), BranchKind::Register) => {
            let (low, high) = split_u32(instruction.bx() as u32);
            DecodedInstr::BranchIf { low, high }
        }
        Opcode::BrCmp if branch_site(branches, instruction.bx(), BranchKind::Compare) => {
            let (low, high) = split_u32(instruction.bx() as u32);
            DecodedInstr::BranchCompare { low, high }
        }
        Opcode::Br | Opcode::BrIf | Opcode::BrCmp => DecodedInstr::Invalid,
        Opcode::Call if valid_index(instruction.bx(), call_site_count) => {
            let (low, high) = split_u32(instruction.bx() as u32);
            DecodedInstr::Call { low, high }
        }
        Opcode::Call => DecodedInstr::Invalid,
        Opcode::GlobalA
            if u32::from(instruction.a) < register_count
                && valid_index(instruction.bx(), global_count) =>
        {
            let (index_low, index_high) = split_u32(instruction.bx() as u32);
            DecodedInstr::GlobalA {
                a: instruction.a,
                index_low,
                index_high,
            }
        }
        Opcode::GlobalA => DecodedInstr::Invalid,
        Opcode::Alloca if u32::from(instruction.a) < register_count => {
            if usize::try_from(instruction.bx())
                .ok()
                .and_then(|index| allocations.get(index))
                .is_none()
            {
                return DecodedInstr::Invalid;
            }
            let (index_low, index_high) = split_u32(instruction.bx() as u32);
            DecodedInstr::Alloca {
                a: instruction.a,
                index_low,
                index_high,
            }
        }
        Opcode::Alloca => DecodedInstr::Invalid,
        op if is_memory(op) => decode_raw(instruction, true, raw_instructions),
        _ => decode_raw(instruction, false, raw_instructions),
    }
}

fn decode_integer_compare(instruction: Instr) -> DecodedInstr {
    let registers = (instruction.a, instruction.b, instruction.c);
    match instruction.flags >> 3 {
        0 => DecodedInstr::IcmpEq64 {
            a: registers.0,
            b: registers.1,
            c: registers.2,
        },
        1 => DecodedInstr::IcmpNe64 {
            a: registers.0,
            b: registers.1,
            c: registers.2,
        },
        2 => DecodedInstr::IcmpSlt64 {
            a: registers.0,
            b: registers.1,
            c: registers.2,
        },
        3 => DecodedInstr::IcmpSle64 {
            a: registers.0,
            b: registers.1,
            c: registers.2,
        },
        4 => DecodedInstr::IcmpSgt64 {
            a: registers.0,
            b: registers.1,
            c: registers.2,
        },
        5 => DecodedInstr::IcmpSge64 {
            a: registers.0,
            b: registers.1,
            c: registers.2,
        },
        6 => DecodedInstr::IcmpUlt64 {
            a: registers.0,
            b: registers.1,
            c: registers.2,
        },
        7 => DecodedInstr::IcmpUle64 {
            a: registers.0,
            b: registers.1,
            c: registers.2,
        },
        8 => DecodedInstr::IcmpUgt64 {
            a: registers.0,
            b: registers.1,
            c: registers.2,
        },
        _ => DecodedInstr::IcmpUge64 {
            a: registers.0,
            b: registers.1,
            c: registers.2,
        },
    }
}

fn decode_raw(instruction: Instr, memory: bool, raw_instructions: &mut Vec<Instr>) -> DecodedInstr {
    let Ok(index) = u32::try_from(raw_instructions.len()) else {
        return DecodedInstr::Invalid;
    };
    raw_instructions.push(instruction);
    let (low, high) = split_u32(index);
    if memory {
        DecodedInstr::Memory { low, high }
    } else {
        DecodedInstr::Other { low, high }
    }
}

#[derive(Clone, Copy)]
enum FloatKind {
    Binary,
    Unary,
    Compare,
    Fma,
}

fn decode_float(
    instruction: Instr,
    kind: FloatKind,
    raw_instructions: &mut Vec<Instr>,
) -> DecodedInstr {
    if instruction.flags & 1 == 1 {
        let (a, b, c) = (instruction.a, instruction.b, instruction.c);
        match instruction.op {
            Opcode::FAdd => return DecodedInstr::FAdd64 { a, b, c },
            Opcode::FSub => return DecodedInstr::FSub64 { a, b, c },
            Opcode::FMul => return DecodedInstr::FMul64 { a, b, c },
            Opcode::FDiv => return DecodedInstr::FDiv64 { a, b, c },
            _ => {}
        }
    }
    let Ok(index) = u32::try_from(raw_instructions.len()) else {
        return DecodedInstr::Invalid;
    };
    raw_instructions.push(instruction);
    match kind {
        FloatKind::Binary => DecodedInstr::FloatBinary(index),
        FloatKind::Unary => DecodedInstr::FloatUnary(index),
        FloatKind::Compare => DecodedInstr::FloatCompare(index),
        FloatKind::Fma => DecodedInstr::FloatFma(index),
    }
}

fn split_u32(value: u32) -> (u16, u16) {
    (value as u16, (value >> 16) as u16)
}

fn valid_index(index: i32, length: usize) -> bool {
    usize::try_from(index).is_ok_and(|index| index < length)
}

#[derive(Clone, Copy)]
enum BranchKind {
    Jump,
    Register,
    Compare,
}

fn branch_site(branches: &[Option<DecodedBranchSite>], index: i32, kind: BranchKind) -> bool {
    let Some(site) = usize::try_from(index)
        .ok()
        .and_then(|index| branches.get(index))
        .and_then(Option::as_ref)
    else {
        return false;
    };
    matches!(
        (kind, site.condition.as_ref(), site.else_edge.is_some()),
        (BranchKind::Jump, None, false)
            | (BranchKind::Register, Some(DecodedCondition::Register(_)), _)
            | (
                BranchKind::Compare,
                Some(DecodedCondition::IntegerCompare { .. }),
                _
            )
    )
}
