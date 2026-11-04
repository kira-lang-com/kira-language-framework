use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryNominalKind, BinaryOperand, BinaryType,
    BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const UNINIT: u64 = 1 << 30;

pub(super) fn verify_body(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut cursor = 0usize;
    for block in &body.blocks {
        cursor = cursor.saturating_add(block.parameters.len());
        let Some(instructions) = &block.instructions else {
            cursor = cursor.saturating_add(block.instruction_count as usize);
            continue;
        };
        for instruction in instructions {
            if (0x160..=0x16a).contains(&instruction.opcode)
                && !verify_instruction(instruction, value_types, cursor, types)
            {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "function @{function_name} CODE offset {}: heap opcode 0x{:03X} has incompatible operands, result type, or ownership shape",
                        instruction.offset, instruction.opcode
                    ),
                ));
            }
            cursor = cursor.saturating_add(instruction.result_count as usize);
        }
    }
    diagnostics
}

fn verify_instruction(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    result_start: usize,
    types: &BinaryTypeTable,
) -> bool {
    let allowed = if instruction.opcode == 0x164 {
        UNINIT
    } else {
        0
    };
    if instruction.modifiers & !allowed != 0 {
        return false;
    }
    let values = value_ids(instruction);
    let result = result_type(value_types, result_start);
    match instruction.opcode {
        0x160 | 0x161 => verify_allocation(instruction, result, types),
        0x162 | 0x163 => verify_payload_address(instruction, &values, value_types, result, types),
        0x164 => {
            values.len() == 2
                && instruction.result_count == 1
                && values.iter().all(|value| {
                    value_type(value_types, *value).is_some_and(|ty| is_integer(types, ty, 64))
                })
                && result.is_some_and(|ty| is_pointer(types, ty, 0))
        }
        0x165 => {
            values.len() == 3
                && instruction.result_count == 1
                && value_type(value_types, values[0]).is_some_and(|pointer| {
                    is_pointer(types, pointer, 0)
                        && result.is_some_and(|actual| same_type(types, actual, pointer))
                })
                && values[1..].iter().all(|value| {
                    value_type(value_types, *value).is_some_and(|ty| is_integer(types, ty, 64))
                })
        }
        0x166 => {
            values.len() == 1
                && instruction.result_count == 0
                && value_type(value_types, values[0]).is_some_and(|ty| is_pointer(types, ty, 0))
        }
        0x167 => verify_weak_new(instruction, &values, value_types, result, types),
        0x168 => verify_weak_load(instruction, &values, value_types, result, types),
        0x169 => {
            values.len() == 1
                && instruction.result_count == 1
                && value_type(value_types, values[0]).is_some_and(|ty| {
                    matches!(entry(types, ty), Some(BinaryType::Reference { .. }))
                })
                && result.is_some_and(|ty| is_pointer(types, ty, 0))
        }
        0x16a => {
            values.len() == 1
                && instruction.result_count == 1
                && value_type(value_types, values[0]).is_some_and(|ty| {
                    matches!(entry(types, ty), Some(BinaryType::Reference { .. }))
                })
                && result.is_some_and(|ty| is_integer(types, ty, 1))
        }
        _ => false,
    }
}

fn verify_allocation(
    instruction: &BinaryInstruction,
    result: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    if instruction.result_count != 1 || !value_ids(instruction).is_empty() {
        return false;
    }
    let (Some(allocated), Some(result)) = (type_operand(instruction), result) else {
        return false;
    };
    let expected = if instruction.opcode == 0x160 {
        BinaryType::Reference { pointee: allocated }
    } else {
        BinaryType::Unique { pointee: allocated }
    };
    entry(types, result) == Some(&expected)
}

fn verify_payload_address(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    result: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    if values.len() != 1 || instruction.result_count != 1 {
        return false;
    }
    let Some(source) = value_type(value_types, values[0]) else {
        return false;
    };
    let pointee = match (instruction.opcode, entry(types, source)) {
        (0x162, Some(BinaryType::Reference { pointee }))
        | (0x163, Some(BinaryType::Unique { pointee })) => *pointee,
        _ => return false,
    };
    result.is_some_and(|actual| {
        matches!(entry(types, actual), Some(BinaryType::Address { pointee: ty, space: 0 }) if same_type(types, *ty, pointee))
    })
}

fn verify_weak_new(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    result: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    if values.len() != 1 || instruction.result_count != 1 {
        return false;
    }
    let Some(source) = value_type(value_types, values[0]) else {
        return false;
    };
    let Some(BinaryType::Reference { pointee }) = entry(types, source) else {
        return false;
    };
    result.is_some_and(|actual| {
        matches!(entry(types, actual), Some(BinaryType::Weak { pointee: ty }) if same_type(types, *ty, *pointee))
    })
}

fn verify_weak_load(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    result: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    if values.len() != 1 || instruction.result_count != 1 {
        return false;
    }
    let Some(weak) = value_type(value_types, values[0]) else {
        return false;
    };
    let Some(BinaryType::Weak { pointee }) = entry(types, weak) else {
        return false;
    };
    let Some(optional) = optional_reference_type(types, *pointee) else {
        return false;
    };
    result.is_some_and(|actual| same_type(types, actual, optional))
}

fn optional_reference_type(types: &BinaryTypeTable, pointee: u64) -> Option<u64> {
    let reference = types.entries.iter().position(
        |ty| matches!(ty, BinaryType::Reference { pointee: actual } if *actual == pointee),
    )?;
    let reference = u64::try_from(reference).ok()?;
    let mut declarations = types
        .declarations
        .iter()
        .enumerate()
        .filter(|(_, declaration)| {
            declaration.kind == BinaryNominalKind::Enum
                && declaration.generic_parameters.len() == 1
                && declaration.name.trim_start_matches('%').rsplit('.').next() == Some("Optional")
        });
    let declaration = u64::try_from(declarations.next()?.0).ok()?;
    if declarations.next().is_some() {
        return None;
    }
    types
        .entries
        .iter()
        .position(|ty| {
            matches!(
                ty,
                BinaryType::Nominal { declaration: actual, arguments }
                    if *actual == declaration && arguments == &[reference]
            )
        })
        .and_then(|index| u64::try_from(index).ok())
}

fn is_pointer(types: &BinaryTypeTable, ty: u64, space: u8) -> bool {
    matches!(entry(types, ty), Some(BinaryType::Pointer(actual)) if *actual == space)
}

fn is_integer(types: &BinaryTypeTable, ty: u64, width: u64) -> bool {
    matches!(entry(types, ty), Some(BinaryType::Integer(actual)) if *actual == width)
}

fn value_ids(instruction: &BinaryInstruction) -> Vec<u64> {
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

fn value_type(value_types: &[Option<u64>], value: u64) -> Option<u64> {
    value_types
        .get(usize::try_from(value).ok()?)
        .copied()
        .flatten()
}

fn result_type(value_types: &[Option<u64>], start: usize) -> Option<u64> {
    value_types.get(start).copied().flatten()
}

fn type_operand(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })
}

fn entry(types: &BinaryTypeTable, ty: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(ty).ok()?)
}

fn same_type(types: &BinaryTypeTable, left: u64, right: u64) -> bool {
    entry(types, left) == entry(types, right) && entry(types, left).is_some()
}
