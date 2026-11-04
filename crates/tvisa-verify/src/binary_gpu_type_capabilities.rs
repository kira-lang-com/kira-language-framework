use std::collections::HashSet;

use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, ParameterItemSummary, ResultItemSummary,
    TvisaValueTypeRecord, TypeSection,
};

pub(super) fn collect(
    type_roots: &[u64],
    signature_roots: &[u64],
    types: &TypeSection,
) -> HashSet<(u8, &'static str)> {
    let mut required = HashSet::new();
    let mut visited_types = HashSet::new();
    let mut visited_signatures = HashSet::new();
    for index in type_roots {
        visit_type(
            *index,
            types,
            &mut required,
            &mut visited_types,
            &mut visited_signatures,
        );
    }
    for index in signature_roots {
        visit_signature(
            *index,
            types,
            &mut required,
            &mut visited_types,
            &mut visited_signatures,
        );
    }
    required
}

pub(super) fn cooperative_instruction_requirements(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> HashSet<(u8, &'static str)> {
    let mut required = HashSet::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| (0x560..=0x568).contains(&instruction.opcode))
    {
        if uses_float_kind(instruction, value_types, types, 0x0c) {
            required.insert((9, "coop.blockscaled"));
        }
    }
    required
}

fn uses_float_kind(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
    tag: u8,
) -> bool {
    instruction.operands.iter().any(|operand| match operand {
        BinaryOperand::Type(type_index) => {
            contains_float_kind(*type_index, types, tag, &mut HashSet::new())
        }
        BinaryOperand::Value(value) => value_type(*value, value_types)
            .is_some_and(|index| contains_float_kind(index, types, tag, &mut HashSet::new())),
        BinaryOperand::Values(values) => values.iter().any(|value| {
            value_type(*value, value_types)
                .is_some_and(|index| contains_float_kind(index, types, tag, &mut HashSet::new()))
        }),
        _ => false,
    })
}

fn value_type(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn contains_float_kind(
    index: u64,
    types: &TypeSection,
    tag: u8,
    visited: &mut HashSet<u64>,
) -> bool {
    if !visited.insert(index) {
        return false;
    }
    let Some(ty) = usize::try_from(index)
        .ok()
        .and_then(|index| types.value_types.get(index))
    else {
        return false;
    };
    if matches!(ty, TvisaValueTypeRecord::Float { tag: actual } if *actual == tag) {
        return true;
    }
    let nested = match ty {
        TvisaValueTypeRecord::FixedVector { element_type, .. }
        | TvisaValueTypeRecord::ScalableVector { element_type }
        | TvisaValueTypeRecord::Texture {
            component_type: element_type,
            ..
        }
        | TvisaValueTypeRecord::Tensor { element_type, .. }
        | TvisaValueTypeRecord::CooperativeMatrix { element_type, .. } => Some(*element_type),
        _ => None,
    };
    nested.is_some_and(|element| contains_float_kind(element, types, tag, visited))
}

fn visit_type(
    index: u64,
    types: &TypeSection,
    required: &mut HashSet<(u8, &'static str)>,
    visited_types: &mut HashSet<u64>,
    visited_signatures: &mut HashSet<u64>,
) {
    if !visited_types.insert(index) {
        return;
    }
    let Some(ty) = usize::try_from(index)
        .ok()
        .and_then(|index| types.value_types.get(index))
    else {
        return;
    };
    let nested = match ty {
        TvisaValueTypeRecord::Float { tag: 0x05 } => {
            required.insert((0, "float64"));
            None
        }
        TvisaValueTypeRecord::Pointer { address_space: 8 } => {
            required.insert((3, "tile"));
            None
        }
        TvisaValueTypeRecord::Pointer { address_space: 9 } => {
            required.insert((6, "work_graphs"));
            None
        }
        TvisaValueTypeRecord::FixedVector { element_type, .. }
        | TvisaValueTypeRecord::ScalableVector { element_type }
        | TvisaValueTypeRecord::Texture {
            component_type: element_type,
            ..
        }
        | TvisaValueTypeRecord::Tensor { element_type, .. } => Some(*element_type),
        TvisaValueTypeRecord::CooperativeMatrix { element_type, .. } => {
            match usize::try_from(*element_type)
                .ok()
                .and_then(|index| types.value_types.get(index))
            {
                Some(TvisaValueTypeRecord::Float { tag: 0x07 | 0x08 }) => {
                    required.insert((8, "coop.fp8"));
                }
                Some(TvisaValueTypeRecord::Float {
                    tag: 0x09 | 0x0a | 0x0b,
                }) => {
                    required.insert((9, "coop.blockscaled"));
                }
                _ => {}
            }
            Some(*element_type)
        }
        TvisaValueTypeRecord::VisibleFunctionTable { signature } => {
            visit_signature(
                *signature,
                types,
                required,
                visited_types,
                visited_signatures,
            );
            None
        }
        _ => None,
    };
    if let Some(nested) = nested {
        visit_type(nested, types, required, visited_types, visited_signatures);
    }
}

fn visit_signature(
    index: u64,
    types: &TypeSection,
    required: &mut HashSet<(u8, &'static str)>,
    visited_types: &mut HashSet<u64>,
    visited_signatures: &mut HashSet<u64>,
) {
    if !visited_signatures.insert(index) {
        return;
    }
    let Some(signature) = usize::try_from(index)
        .ok()
        .and_then(|index| types.signatures.get(index))
    else {
        return;
    };
    for parameter in &signature.parameters {
        match parameter {
            ParameterItemSummary::Leaf { type_index } => visit_type(
                *type_index,
                types,
                required,
                visited_types,
                visited_signatures,
            ),
            ParameterItemSummary::Group(group) => {
                for member in &group.members {
                    visit_type(
                        member.type_index,
                        types,
                        required,
                        visited_types,
                        visited_signatures,
                    );
                }
            }
            ParameterItemSummary::CAggregate { index } => {
                if let Some(aggregate) = usize::try_from(*index)
                    .ok()
                    .and_then(|index| types.c_aggregates.get(index))
                {
                    for member in &aggregate.members {
                        visit_type(
                            member.type_index,
                            types,
                            required,
                            visited_types,
                            visited_signatures,
                        );
                    }
                }
            }
            _ => {}
        }
    }
    for result in &signature.results {
        match result {
            ResultItemSummary::Leaf { type_index } => visit_type(
                *type_index,
                types,
                required,
                visited_types,
                visited_signatures,
            ),
            ResultItemSummary::Group(group) => {
                for member in &group.members {
                    visit_type(
                        member.type_index,
                        types,
                        required,
                        visited_types,
                        visited_signatures,
                    );
                }
            }
        }
    }
}
