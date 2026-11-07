//! T1 V2 operand/result checks for target queries, scalar expectation hints,
//! permissive assumptions, and the fixed zero-argument portable intrinsics.
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
    let mut next_result = 0usize;
    for block in &body.blocks {
        next_result = next_result.saturating_add(block.parameters.len());
        if let Some(instructions) = &block.instructions {
            for instruction in instructions {
                if let Some(message) = check(instruction, value_types, next_result, types) {
                    diagnostics.push(Diagnostic::error(
                        V2,
                        format!(
                            "function @{function} CODE offset {}: {message}",
                            instruction.offset,
                        ),
                    ));
                }
                next_result = next_result.saturating_add(instruction.result_count as usize);
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
    let ty = |value: u64| {
        usize::try_from(value)
            .ok()
            .and_then(|value| value_types.get(value))
            .copied()
            .flatten()
    };
    let integer = |value: u64, bits: u64| {
        ty(value)
            .and_then(|index| types.entries.get(index as usize))
            .is_some_and(|entry| matches!(entry, BinaryType::Integer(size) if *size == bits))
    };
    let result = value_types.get(result_index).copied().flatten();
    match instruction.opcode {
        0x243 => {
            let valid = matches!(instruction.operands.as_slice(), [BinaryOperand::Value(value)]
                if integer(*value, 1));
            (!valid).then(|| "assume requires one i1 SSA operand".to_owned())
        }
        0x244 => {
            let valid = match instruction.operands.as_slice() {
                [
                    BinaryOperand::Value(value),
                    BinaryOperand::Integer(expected),
                    BinaryOperand::OptionalFloat32(probability),
                ] => {
                    let source = ty(*value);
                    let width = source
                        .and_then(|source| types.entries.get(source as usize))
                        .and_then(|entry| match entry {
                            BinaryType::Integer(bits) => Some(*bits),
                            _ => None,
                        });
                    width.is_some_and(|width| fits(expected.negative, expected.magnitude, width))
                        && source == result
                        && probability.is_none_or(|bits| {
                            let value = f32::from_bits(bits);
                            value.is_finite() && (0.0..=1.0).contains(&value)
                        })
                }
                _ => false,
            };
            (!valid).then(|| {
                "expect requires a fitting integer immediate and matching integer result".to_owned()
            })
        }
        0x223 | 0x224 => {
            let width = if instruction.opcode == 0x223 { 1 } else { 64 };
            let valid = result
                .and_then(|index| types.entries.get(index as usize))
                .is_some_and(|entry| matches!(entry, BinaryType::Integer(size) if *size == width));
            (!valid).then(|| "target query has an invalid result type".to_owned())
        }
        0x230 => match instruction.operands.as_slice() {
            [BinaryOperand::Index(11 | 14)] | [BinaryOperand::Index(12 | 13)] => {
                let result_expected = matches!(
                    instruction.operands.as_slice(),
                    [BinaryOperand::Index(12 | 13)]
                );
                let valid = if result_expected {
                    instruction.result_count == 1
                        && result
                            .and_then(|index| types.entries.get(index as usize))
                            .is_some_and(|ty| matches!(ty, BinaryType::Integer(64)))
                } else {
                    instruction.result_count == 0
                };
                (!valid).then(|| {
                    "portable zero-argument intrinsic has the wrong result type".to_owned()
                })
            }
            _ => None, // Other intrinsic signatures are verified by their own family.
        },
        _ => None,
    }
}

fn fits(negative: bool, magnitude: u128, width: u64) -> bool {
    if width == 0 {
        return false;
    }
    if negative {
        if magnitude == 0 || magnitude > (1u128 << 127) {
            return false;
        }
        if width >= 128 {
            return true;
        }
        magnitude <= (1u128 << (width - 1))
    } else {
        width >= 128 || magnitude < (1u128 << width)
    }
}

#[cfg(test)]
mod tests {
    use super::fits;

    #[test]
    fn expectation_integer_limits_are_exact() {
        assert!(fits(false, 255, 8));
        assert!(!fits(false, 256, 8));
        assert!(fits(true, 128, 8));
        assert!(!fits(true, 129, 8));
        assert!(!fits(true, 0, 8));
        assert!(fits(false, u128::MAX, 128));
        assert!(fits(true, 1u128 << 127, 128));
        assert!(!fits(true, (1u128 << 127) + 1, 256));
    }
}
