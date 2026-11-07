use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::leb::IntegerImmediate;
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryNominalKind, BinaryOperand, BinaryType,
    BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function: &str,
) -> Vec<Diagnostic> {
    body.blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
        .filter_map(|instruction| {
            let issue = match instruction.opcode {
                0x1e2 => check_switch(instruction, value_types, types),
                0x1e3 => check_enum_switch(instruction, body, value_types, types),
                _ => None,
            };
            issue.map(|message| {
                Diagnostic::error(
                    V2,
                    format!(
                        "function @{function} CODE offset {}: {message}",
                        instruction.offset
                    ),
                )
            })
        })
        .collect()
}

fn check_switch(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> Option<&'static str> {
    if !instruction.terminator || instruction.result_count != 0 || instruction.modifiers != 0 {
        return Some("switch.int requires zero results and modifiers");
    }
    let [
        BinaryOperand::Value(selector),
        BinaryOperand::SwitchCases(cases),
        BinaryOperand::BlockTarget { .. },
        BinaryOperand::Index(_),
    ] = instruction.operands.as_slice()
    else {
        return Some("switch.int operand shape is invalid");
    };
    let width = usize::try_from(*selector)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
        .and_then(|index| types.entries.get(index as usize))
        .and_then(|entry| match entry {
            BinaryType::Integer(width) if (1..=128).contains(width) => Some(*width as u16),
            _ => None,
        });
    let Some(width) = width else {
        return Some("switch.int selector must be an integer up to 128 bits");
    };
    let mut seen = HashSet::new();
    for (immediate, _, _) in cases {
        if !fits(*immediate, width) {
            return Some("switch.int case does not fit the selector type");
        }
        if !seen.insert(bits(*immediate, width)) {
            return Some("switch.int has duplicate case values");
        }
    }
    None
}

fn check_enum_switch(
    instruction: &BinaryInstruction,
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> Option<&'static str> {
    if !instruction.terminator || instruction.result_count != 0 || instruction.modifiers != 0 {
        return Some("switch.enum must be a zero-result terminator without modifiers");
    }
    let [
        BinaryOperand::Value(selector),
        BinaryOperand::SwitchEnumCases(cases),
        BinaryOperand::OptionalIndex(default),
    ] = instruction.operands.as_slice()
    else {
        return Some("switch.enum has an invalid binary operand shape");
    };
    let definition = usize::try_from(*selector)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
        .and_then(|index| types.entries.get(index as usize))
        .and_then(|ty| match ty {
            BinaryType::Nominal {
                declaration,
                arguments,
            } if arguments.is_empty() => Some(*declaration),
            _ => None,
        })
        .and_then(|index| {
            (types.declarations.get(index as usize)?.kind == BinaryNominalKind::Enum)
                .then(|| {
                    types
                        .definitions
                        .iter()
                        .find(|definition| definition.declaration == index)
                })
                .flatten()
        });
    let Some(definition) = definition else {
        return Some("switch.enum selector must be a defined nongeneric nominal enum");
    };
    if cases.is_empty() && default.is_none() {
        return Some("switch.enum needs a case or default");
    }
    if default.is_none() && cases.len() != definition.variants.len() {
        return Some("nonexhaustive switch.enum requires a default");
    }
    let mut last = None;
    for (variant, target) in cases {
        if last.is_some_and(|last| last >= *variant) {
            return Some("switch.enum variant indexes must be strictly increasing");
        }
        last = Some(*variant);
        let Some(payload) = definition
            .variants
            .get(*variant as usize)
            .map(|variant| &variant.payload)
        else {
            return Some("switch.enum case names an invalid variant index");
        };
        let Some(target) = usize::try_from(*target)
            .ok()
            .and_then(|index| body.blocks.get(index))
        else {
            return Some("switch.enum target is outside this function");
        };
        if target
            .parameters
            .iter()
            .map(|parameter| parameter.ty)
            .collect::<Vec<_>>()
            != *payload
        {
            return Some("switch.enum case block parameters must match the variant payload");
        }
    }
    if let Some(default) = default {
        let Some(target) = usize::try_from(*default)
            .ok()
            .and_then(|index| body.blocks.get(index))
        else {
            return Some("switch.enum default target is outside this function");
        };
        if !target.parameters.is_empty() {
            return Some("switch.enum default must not take payload parameters");
        }
    }
    None
}

fn bits(immediate: IntegerImmediate, width: u16) -> u128 {
    let value = if immediate.negative {
        immediate.magnitude.wrapping_neg()
    } else {
        immediate.magnitude
    };
    if width == 128 {
        value
    } else {
        value & ((1u128 << width) - 1)
    }
}

fn fits(immediate: IntegerImmediate, width: u16) -> bool {
    if immediate.negative {
        immediate.magnitude != 0 && immediate.magnitude <= (1u128 << (width - 1))
    } else if width == 128 {
        true
    } else {
        immediate.magnitude < (1u128 << width)
    }
}

#[cfg(test)]
mod tests {
    use super::check_switch;
    use tir_format::leb::IntegerImmediate;
    use tir_format::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};

    #[test]
    fn v2_rejects_duplicate_and_out_of_range_switch_cases() {
        let types = BinaryTypeTable {
            declarations: Vec::new(),
            entries: vec![
                BinaryType::Integer(8),
                BinaryType::Float(tir_types::FloatType::F32),
            ],
            definitions: Vec::new(),
        };
        let case = |negative, magnitude| {
            (
                IntegerImmediate {
                    negative,
                    magnitude,
                },
                1,
                Vec::new(),
            )
        };
        let switch = |selector, cases| BinaryInstruction {
            offset: 0,
            opcode: 0x1e2,
            modifiers: 0,
            result_count: 0,
            terminator: true,
            operands: vec![
                BinaryOperand::Value(selector),
                BinaryOperand::SwitchCases(cases),
                BinaryOperand::BlockTarget {
                    block: 1,
                    arguments: Vec::new(),
                },
                BinaryOperand::Index(0),
            ],
        };
        assert!(
            check_switch(
                &switch(0, vec![case(true, 1), case(false, 255)]),
                &[Some(0)],
                &types
            )
            .is_some()
        );
        assert!(check_switch(&switch(0, vec![case(false, 256)]), &[Some(0)], &types).is_some());
        assert!(check_switch(&switch(0, vec![case(true, 129)]), &[Some(0)], &types).is_some());
        assert!(
            check_switch(
                &switch(0, vec![case(true, 1), case(false, 5)]),
                &[Some(0)],
                &types
            )
            .is_none()
        );
        assert!(check_switch(&switch(0, vec![case(false, 1)]), &[Some(1)], &types).is_some());
    }
}
