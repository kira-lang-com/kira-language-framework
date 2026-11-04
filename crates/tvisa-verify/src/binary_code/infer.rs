use super::{calls, gpu_collectives};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, FunctionRecordSummary, ImportCounts,
    TvisaValueTypeRecord, TypeSection, VariantIndex,
};

pub(super) fn infer_value_types(
    body: &BinaryFunctionBody,
    function: &FunctionRecordSummary,
    types: &TypeSection,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    variants: &VariantIndex,
) -> Vec<Option<u64>> {
    let mut value_types = Vec::with_capacity(body.header.value_count as usize);
    for block in &body.blocks {
        value_types.extend(block.parameter_types.iter().copied().map(Some));
        for instruction in &block.instructions {
            let results = if instruction.opcode == 0x700 {
                infer_instruction_results(instruction, &value_types, types, function)
            } else {
                calls::infer_result_types(
                    instruction,
                    &value_types,
                    types,
                    functions,
                    imports,
                    variants,
                )
                .unwrap_or_else(|| {
                    infer_instruction_results(instruction, &value_types, types, function)
                })
            };
            value_types.extend(results);
        }
    }
    value_types
}

fn infer_instruction_results(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
    function: &FunctionRecordSummary,
) -> Vec<Option<u64>> {
    let count = usize::from(instruction.result_count);
    let mut results = vec![None; count];
    let first_value_type = value_operand_type(instruction, 0, value_types);
    let explicit_type = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(type_index) => Some(*type_index),
            _ => None,
        });
    let i1 = integer_type(types, 1);
    let i32 = integer_type(types, 32);
    let inferred = match instruction.opcode {
        0x000 | 0x001 | 0x004 | 0x005 | 0x007 | 0x008 | 0x146 | 0x200 | 0x211 | 0x220 | 0x222
        | 0x270 | 0x400 | 0x401 => explicit_type,
        0x011 => types
            .value_types
            .iter()
            .position(|ty| matches!(ty, TvisaValueTypeRecord::Sampler))
            .map(|index| index as u64),
        0x00a | 0x280 | 0x282 => types
            .value_types
            .iter()
            .position(|ty| matches!(ty, TvisaValueTypeRecord::Code))
            .map(|index| index as u64),
        0x00b | 0x00c | 0x00f | 0x273 | 0x281 => pointer_type(types, 0),
        0x010 => integer_type(types, 64),
        0x002 => i1,
        0x003 => instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Index(space) => pointer_type(types, *space as u8),
                _ => None,
            }),
        0x020..=0x045 => match instruction.opcode - 0x020 {
            28 => comparison_result_type(types, first_value_type),
            29 => value_operand_type(instruction, 1, value_types),
            _ => first_value_type,
        },
        0x060..=0x092 => match instruction.opcode - 0x060 {
            20 => comparison_result_type(types, first_value_type),
            21 => retype_lane(types, first_value_type, i32),
            22 => first_value_type,
            24 => i32,
            25 => None,
            _ => first_value_type,
        },
        0x0a0..=0x0ac => explicit_type,
        0x0c0..=0x0d7 => infer_vector_result(
            instruction,
            value_types,
            types,
            explicit_type,
            first_value_type,
        ),
        0x148 | 0x202 => {
            if instruction.opcode == 0x202 {
                value_operand_type(instruction, 1, value_types)
            } else {
                first_value_type
            }
        }
        0x149 => integer_type(types, 64),
        0x1a0 | 0x1a1 | 0x1a9 => None,
        0x1d0 => pointer_type(types, 0),
        0x203 => value_operand_type(instruction, 1, value_types),
        0x223 => i1,
        0x224 => integer_type(types, 64),
        0x406 => types
            .value_types
            .iter()
            .position(|ty| {
                matches!(
                    ty,
                    TvisaValueTypeRecord::FixedVector {
                        lanes: 4,
                        element_type,
                    } if matches!(types.value_types.get(*element_type as usize), Some(TvisaValueTypeRecord::Float { tag: 0x04 }))
                )
            })
            .map(|index| index as u64),
        0x409 => pointer_type(types, 8),
        0x230 => {
            let id = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Index(id) => Some(*id),
                    _ => None,
                });
            match id {
                Some(0 | 1) => i32,
                Some(2..=6) => first_value_type,
                Some(12 | 13) => integer_type(types, 64),
                Some(15) => value_operand_type(instruction, 1, value_types),
                Some(16) => i1,
                _ => None,
            }
        }
        0x700 => instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Index(index) => usize::try_from(*index).ok(),
                _ => None,
            })
            .and_then(|index| function.slots.get(index))
            .and_then(|slot| pointer_type(types, slot.address_space)),
        0x703 | 0x704 | 0x70a | 0x70c..=0x70e => pointer_type(types, 0),
        0x708 | 0x70b => types
            .value_types
            .iter()
            .position(|ty| matches!(ty, TvisaValueTypeRecord::Code))
            .map(|index| index as u64),
        0x70f => integer_type(types, 64),
        _ => gpu_collectives::infer_result_type(instruction, value_types, types),
    };
    if let Some(result) = results.first_mut() {
        *result = inferred;
    }
    if matches!(instruction.opcode, 0x023..=0x025 | 0x203)
        && let Some(result) = results.get_mut(1)
    {
        *result = i1;
    }
    if instruction.opcode == 0x076
        && let Some(result) = results.get_mut(1)
    {
        *result = retype_lane(types, first_value_type, i32);
    }
    if instruction.opcode == 0x1d0
        && let Some(result) = results.get_mut(1)
    {
        *result = i32;
    }
    results
}

fn value_operand_type(
    instruction: &BinaryInstruction,
    operand_index: usize,
    value_types: &[Option<u64>],
) -> Option<u64> {
    let mut values = Vec::new();
    for operand in &instruction.operands {
        match operand {
            BinaryOperand::Value(value) => values.push(*value),
            BinaryOperand::Values(operands) => values.extend_from_slice(operands),
            _ => {}
        }
    }
    value_type(*values.get(operand_index)?, value_types)
}

fn value_type(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn integer_type(types: &TypeSection, bits: u64) -> Option<u64> {
    types
        .value_types
        .iter()
        .position(
            |ty| matches!(ty, TvisaValueTypeRecord::Integer { bits: width } if *width == bits),
        )
        .map(|index| index as u64)
}

fn pointer_type(types: &TypeSection, address_space: u8) -> Option<u64> {
    types
        .value_types
        .iter()
        .position(|ty| matches!(ty, TvisaValueTypeRecord::Pointer { address_space: space } if *space == address_space))
        .map(|index| index as u64)
}

fn vector_element_type(types: &TypeSection, vector_type: u64) -> Option<u64> {
    match types.value_types.get(vector_type as usize)? {
        TvisaValueTypeRecord::FixedVector { element_type, .. }
        | TvisaValueTypeRecord::ScalableVector { element_type } => Some(*element_type),
        _ => None,
    }
}

fn infer_vector_result(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
    explicit_type: Option<u64>,
    first_value_type: Option<u64>,
) -> Option<u64> {
    let relative = instruction.opcode - 0x0c0;
    match relative {
        0 | 4 | 7 | 9 => explicit_type,
        1 | 6 => first_value_type.and_then(|ty| vector_element_type(types, ty)),
        2 | 11 | 12 | 13 | 14 | 15 | 16 | 17 => first_value_type,
        3 => {
            let source = first_value_type?;
            let element_type = vector_element_type(types, source)?;
            let lane_count = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Integers(indices) => u64::try_from(indices.len()).ok(),
                    _ => None,
                })?;
            find_fixed_vector(types, lane_count, element_type)
        }
        5 => integer_type(types, 64),
        8 | 10 => None,
        18 => {
            let values = value_operands(instruction);
            let left = value_type(*values.first()?, value_types)?;
            let right = value_type(*values.get(1)?, value_types)?;
            let (
                TvisaValueTypeRecord::FixedVector {
                    lanes: left_lanes,
                    element_type,
                },
                TvisaValueTypeRecord::FixedVector {
                    lanes: right_lanes,
                    element_type: right_element,
                },
            ) = (
                types.value_types.get(left as usize)?,
                types.value_types.get(right as usize)?,
            )
            else {
                return None;
            };
            if element_type != right_element {
                return None;
            }
            find_fixed_vector(types, left_lanes.checked_add(*right_lanes)?, *element_type)
        }
        19 => {
            let TvisaValueTypeRecord::FixedVector {
                lanes,
                element_type,
            } = types.value_types.get(first_value_type? as usize)?
            else {
                return None;
            };
            (lanes % 2 == 0).then(|| find_fixed_vector(types, lanes / 2, *element_type))?
        }
        20 => {
            let values = value_operands(instruction);
            let indices = value_type(*values.get(1)?, value_types)?;
            let element_type = integer_type(types, 8)?;
            match types.value_types.get(indices as usize)? {
                TvisaValueTypeRecord::FixedVector { lanes, .. } => {
                    find_fixed_vector(types, *lanes, element_type)
                }
                TvisaValueTypeRecord::ScalableVector { .. } => {
                    find_scalable_vector(types, element_type)
                }
                _ => None,
            }
        }
        21 => {
            let element_type = integer_type(types, 1)?;
            find_scalable_vector(types, element_type)
        }
        22 | 23 => integer_type(types, 64),
        _ => None,
    }
}

fn value_operands(instruction: &BinaryInstruction) -> Vec<u64> {
    instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        })
        .copied()
        .collect()
}

fn find_fixed_vector(types: &TypeSection, lanes: u64, element_type: u64) -> Option<u64> {
    types
        .value_types
        .iter()
        .position(|ty| {
            matches!(ty, TvisaValueTypeRecord::FixedVector { lanes: actual_lanes, element_type: actual_element }
                if *actual_lanes == lanes && *actual_element == element_type)
        })
        .map(|index| index as u64)
}

fn find_scalable_vector(types: &TypeSection, element_type: u64) -> Option<u64> {
    types
        .value_types
        .iter()
        .position(|ty| matches!(ty, TvisaValueTypeRecord::ScalableVector { element_type: actual } if *actual == element_type))
        .map(|index| index as u64)
}

fn comparison_result_type(types: &TypeSection, operand_type: Option<u64>) -> Option<u64> {
    let i1 = integer_type(types, 1)?;
    match types.value_types.get(operand_type? as usize)? {
        TvisaValueTypeRecord::FixedVector { lanes, .. } => types
            .value_types
            .iter()
            .position(|ty| {
                matches!(ty, TvisaValueTypeRecord::FixedVector { lanes: mask_lanes, element_type } if mask_lanes == lanes && *element_type == i1)
            })
            .map(|index| index as u64),
        TvisaValueTypeRecord::ScalableVector { .. } => types
            .value_types
            .iter()
            .position(|ty| matches!(ty, TvisaValueTypeRecord::ScalableVector { element_type } if *element_type == i1))
            .map(|index| index as u64),
        _ => Some(i1),
    }
}

fn retype_lane(
    types: &TypeSection,
    vector_type: Option<u64>,
    scalar_type: Option<u64>,
) -> Option<u64> {
    let scalar_type = scalar_type?;
    match types.value_types.get(vector_type? as usize)? {
        TvisaValueTypeRecord::FixedVector { lanes, .. } => types
            .value_types
            .iter()
            .position(|ty| matches!(ty, TvisaValueTypeRecord::FixedVector { lanes: result_lanes, element_type } if result_lanes == lanes && *element_type == scalar_type))
            .map(|index| index as u64),
        TvisaValueTypeRecord::ScalableVector { .. } => types
            .value_types
            .iter()
            .position(|ty| matches!(ty, TvisaValueTypeRecord::ScalableVector { element_type } if *element_type == scalar_type))
            .map(|index| index as u64),
        _ => Some(scalar_type),
    }
}
