//! V2 typed checks for the Tessera device I/O subset (MMIO and I/O fences).
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function: &str,
) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    let mut next_result = 0usize;
    for block in &body.blocks {
        next_result = next_result.saturating_add(block.parameters.len());
        if let Some(instructions) = &block.instructions {
            for instruction in instructions {
                if matches!(instruction.opcode, 0x270..=0x272)
                    && let Some(message) = check(instruction, value_types, next_result, types)
                {
                    errors.push(Diagnostic::error(
                        V2,
                        format!(
                            "function @{function} CODE offset {}: {message}",
                            instruction.offset
                        ),
                    ));
                }
                next_result = next_result.saturating_add(instruction.result_count as usize);
            }
        }
    }
    errors
}

fn check(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    result_index: usize,
    types: &BinaryTypeTable,
) -> Option<String> {
    let ty = |index: u64| {
        usize::try_from(index)
            .ok()
            .and_then(|index| value_types.get(index))
            .copied()
            .flatten()
    };
    let scalar = |index: u64| {
        matches!(
            types.entries.get(index as usize),
            Some(BinaryType::Integer(8 | 16 | 32 | 64))
        ) || matches!(types.entries.get(index as usize),
            Some(BinaryType::Float(float)) if matches!(float.bits(), 16 | 32 | 64))
    };
    let pointer = |index: u64| {
        ty(index)
            .and_then(|ty| types.entries.get(ty as usize))
            .is_some_and(|ty| matches!(ty, BinaryType::Pointer(_)))
    };
    let alignment = |align: u64| align == 0 || align.is_power_of_two();
    match instruction.opcode {
        0x270 => {
            let valid = match instruction.operands.as_slice() {
                [
                    BinaryOperand::Type(result_ty),
                    BinaryOperand::Value(address),
                    BinaryOperand::Index(align),
                ] => {
                    instruction.result_count == 1
                        && instruction.modifiers & !(0x7 << 20) == 0
                        && (instruction.modifiers >> 20) <= 2
                        && scalar(*result_ty)
                        && pointer(*address)
                        && alignment(*align)
                        && value_types.get(result_index).copied().flatten() == Some(*result_ty)
                }
                _ => false,
            };
            (!valid).then(|| "mmio.load requires a 1/2/4/8-byte scalar result, pointer, and valid alignment/order".to_owned())
        }
        0x271 => {
            let valid = match instruction.operands.as_slice() {
                [BinaryOperand::Values(args), BinaryOperand::Index(align)] if args.len() == 2 => {
                    instruction.result_count == 0
                        && instruction.modifiers & !(0x7 << 20) == 0
                        && matches!(instruction.modifiers >> 20, 0 | 1 | 3)
                        && ty(args[0]).is_some_and(scalar)
                        && pointer(args[1])
                        && alignment(*align)
                }
                _ => false,
            };
            (!valid).then(|| {
                "mmio.store requires a 1/2/4/8-byte scalar, pointer, and valid alignment/order"
                    .to_owned()
            })
        }
        0x272 => {
            let valid = instruction.result_count == 0
                && instruction.operands.is_empty()
                && instruction.modifiers & !(0xf << 48) == 0
                && instruction.modifiers >> 48 <= 2;
            (!valid).then(|| {
                "fence.io requires a valid read/write/full selector and no operands".to_owned()
            })
        }
        _ => None,
    }
}
