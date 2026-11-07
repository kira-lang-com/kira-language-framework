//! V2 verification for the typed CPU va_list and setjmp CODE subset.
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
    let mut diagnostics = Vec::new();
    let mut result_index = 0usize;
    for block in &body.blocks {
        result_index += block.parameters.len();
        if let Some(instructions) = &block.instructions {
            for instruction in instructions {
                if matches!(instruction.opcode, 0x210..=0x215)
                    && let Some(message) = check(instruction, value_types, result_index, types)
                {
                    diagnostics.push(Diagnostic::error(
                        V2,
                        format!(
                            "function @{function} CODE offset {}: {message}",
                            instruction.offset
                        ),
                    ));
                }
                result_index = result_index.saturating_add(instruction.result_count as usize);
            }
        }
    }
    diagnostics
}

fn check(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    result_index: usize,
    types: &BinaryTypeTable,
) -> Option<String> {
    let type_of = |value: u64| {
        usize::try_from(value)
            .ok()
            .and_then(|index| value_types.get(index))
            .copied()
            .flatten()
    };
    let ptr = |value: u64| {
        type_of(value)
            .and_then(|index| types.entries.get(index as usize))
            .is_some_and(|ty| matches!(ty, BinaryType::Pointer(0)))
    };
    let supported = |ty: u64| {
        matches!(
            types.entries.get(ty as usize),
            Some(BinaryType::Integer(8 | 16 | 32 | 64 | 128))
        ) || matches!(types.entries.get(ty as usize),
            Some(BinaryType::Float(float)) if matches!(float.bits(), 32 | 64))
            || matches!(types.entries.get(ty as usize), Some(BinaryType::Pointer(_)))
    };
    let i32_index = || {
        value_types
            .get(result_index)
            .copied()
            .flatten()
            .and_then(|ty| types.entries.get(ty as usize))
            .is_some_and(|ty| matches!(ty, BinaryType::Integer(32)))
    };
    let valid = match instruction.opcode {
        0x210 | 0x213 => {
            instruction.result_count == 0
                && matches!(instruction.operands.as_slice(), [BinaryOperand::Value(list)] if ptr(*list))
        }
        0x211 => {
            instruction.result_count == 1
                && matches!(instruction.operands.as_slice(),
                    [BinaryOperand::Type(ty), BinaryOperand::Value(list)]
                    if supported(*ty) && ptr(*list) && value_types.get(result_index).copied().flatten() == Some(*ty))
        }
        0x212 => {
            instruction.result_count == 0
                && matches!(instruction.operands.as_slice(), [BinaryOperand::Values(args)]
                    if args.len() == 2 && ptr(args[0]) && ptr(args[1]))
        }
        0x214 => {
            instruction.result_count == 1
                && matches!(instruction.operands.as_slice(), [BinaryOperand::Value(buffer)] if ptr(*buffer))
                && i32_index()
        }
        0x215 => {
            instruction.result_count == 0
                && instruction.modifiers == 0
                && instruction.terminator
                && matches!(instruction.operands.as_slice(), [BinaryOperand::Values(args)]
                    if args.len() == 2 && ptr(args[0])
                        && type_of(args[1]).and_then(|index| types.entries.get(index as usize))
                            .is_some_and(|ty| matches!(ty, BinaryType::Integer(32))))
        }
        _ => true,
    };
    (!valid).then(|| {
        "variadic instruction expects a generic ptr and matching scalar result types".to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::check;
    use tir_format::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};

    #[test]
    fn v2_rejects_bad_vararg_result_and_list_shapes() {
        let types = BinaryTypeTable {
            declarations: Vec::new(),
            entries: vec![
                BinaryType::Pointer(0),
                BinaryType::Integer(32),
                BinaryType::Integer(64),
            ],
            definitions: Vec::new(),
        };
        let make = |opcode, result_count, operands| BinaryInstruction {
            offset: 0,
            opcode,
            modifiers: 0,
            result_count,
            terminator: false,
            operands,
        };
        let inferred = [Some(0), Some(1), Some(2)];
        assert!(
            check(
                &make(0x210, 0, vec![BinaryOperand::Value(1)]),
                &inferred,
                2,
                &types
            )
            .is_some()
        );
        assert!(
            check(
                &make(
                    0x211,
                    1,
                    vec![BinaryOperand::Type(2), BinaryOperand::Value(0)]
                ),
                &inferred,
                1,
                &types
            )
            .is_some()
        );
        assert!(
            check(
                &make(0x212, 0, vec![BinaryOperand::Values(vec![0, 1])]),
                &inferred,
                2,
                &types
            )
            .is_some()
        );
        assert!(
            check(
                &make(0x214, 1, vec![BinaryOperand::Value(0)]),
                &inferred,
                2,
                &types
            )
            .is_some()
        );
        assert!(
            check(
                &make(0x214, 1, vec![BinaryOperand::Value(0)]),
                &inferred,
                1,
                &types
            )
            .is_none()
        );
        assert!(
            check(
                &make(0x215, 0, vec![BinaryOperand::Values(vec![0, 2])]),
                &inferred,
                3,
                &types,
            )
            .is_some()
        );
    }
}
