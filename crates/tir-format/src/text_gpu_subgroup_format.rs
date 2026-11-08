//! Canonical disassembly for 0x440–0x452 typed subgroup collectives.
use std::fmt::Write as _;

use tir_types::FloatType;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, text_gpu_subgroup as subgroup};

pub(crate) fn format(
    text: &mut String,
    operation: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let index = usize::try_from(
        operation
            .opcode
            .checked_sub(0x440)
            .ok_or_else(|| malformed("not a subgroup instruction"))?,
    )
    .map_err(|_| malformed("subgroup opcode index overflow"))?;
    let name = subgroup::NAMES
        .get(index)
        .ok_or_else(|| malformed("unknown subgroup CODE opcode"))?;
    if operation.modifiers != 0 || operation.result_count != 1 {
        return Err(malformed(
            "subgroup CODE requires zero modifiers and one result",
        ));
    }
    let count = subgroup::input_count(operation.opcode);
    let (input_indices, trailing) = match (count, operation.operands.as_slice()) {
        (0, []) => (Vec::new(), &[][..]),
        (1, [BinaryOperand::Value(value)]) => (vec![*value], &[][..]),
        (1, [BinaryOperand::Value(value), rest @ ..]) => (vec![*value], rest),
        (2, [BinaryOperand::Values(items)]) if items.len() == 2 => (items.clone(), &[][..]),
        (2, [BinaryOperand::Values(items), rest @ ..]) if items.len() == 2 => (items.clone(), rest),
        _ => return Err(malformed("subgroup CODE SSA operand shape is invalid")),
    };
    let operands = input_indices
        .iter()
        .map(|index| {
            usize::try_from(*index)
                .ok()
                .and_then(|index| values.get(index))
                .map(|(name, ty)| (name.as_str(), *ty))
                .ok_or_else(|| malformed("subgroup CODE refers to an undefined SSA value"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let first = operands
        .first()
        .and_then(|(_, ty)| types.entries.get(*ty as usize));
    if !valid_inputs(
        operation.opcode,
        first,
        operands.get(1).map(|(_, ty)| *ty),
        types,
    ) {
        return Err(malformed(
            "subgroup CODE operand or lane type is incompatible",
        ));
    }
    let mut options = String::new();
    if matches!(operation.opcode, 0x450..=0x452) {
        let [
            BinaryOperand::Index(operator),
            BinaryOperand::Index(cluster),
        ] = trailing
        else {
            return Err(malformed(
                "subgroup reduction requires operator and cluster fields",
            ));
        };
        let operator_name = subgroup::OPERATORS
            .get(*operator as usize)
            .ok_or_else(|| malformed("subgroup reduction operator is out of range"))?;
        let binary_ty = first.ok_or_else(|| malformed("missing subgroup reduction input type"))?;
        if !compatible_reduction_operator(binary_ty, *operator) {
            return Err(malformed(
                "subgroup reduction operator has incompatible input type",
            ));
        }
        write!(options, " [{operator_name}]").map_err(|issue| malformed(issue.to_string()))?;
        if *cluster != 0 {
            if *cluster > 128 || !cluster.is_power_of_two() {
                return Err(malformed("subgroup cluster must be power of two up to 128"));
            }
            write!(options, " [cluster {cluster}]")
                .map_err(|issue| malformed(issue.to_string()))?;
        }
    } else if !trailing.is_empty() {
        return Err(malformed(
            "non-reduction subgroup CODE has trailing operands",
        ));
    }
    let result_name = format!("v{}", values.len());
    write!(text, "    %{result_name} = {name}").map_err(|issue| malformed(issue.to_string()))?;
    for (position, (name, _)) in operands.iter().enumerate() {
        write!(text, "{}%{name}", if position == 0 { " " } else { ", " })
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    write!(text, "{options}\n").map_err(|issue| malformed(issue.to_string()))?;
    let result_ty = if let Some(ty) = subgroup::result_type(name) {
        crate::text_descriptor_reference::type_index(types, &ty)
            .ok_or_else(|| malformed("subgroup result type missing from TYPES"))? as u64
    } else {
        operands
            .first()
            .ok_or_else(|| malformed("subgroup result operand missing"))?
            .1
    };
    values.push((result_name, result_ty));
    Ok(())
}

fn valid_inputs(
    opcode: u64,
    first: Option<&BinaryType>,
    second_ty: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    let second_i32 = second_ty.is_some_and(|index| {
        matches!(
            types.entries.get(index as usize),
            Some(BinaryType::Integer(32))
        )
    });
    match opcode {
        0x440 => first.is_none() && second_ty.is_none(),
        0x441 | 0x442 | 0x446 => matches!(first, Some(BinaryType::Integer(1))),
        0x443 => first.is_some_and(scalar),
        0x444 | 0x44b..=0x44f => first.is_some_and(|first| numeric(first, types)) && second_i32,
        0x445 => first.is_some_and(|first| numeric(first, types)),
        0x447..=0x44a => first.is_some_and(|first| ballot(first, types)),
        0x450..=0x452 => first.is_some_and(scalar),
        _ => false,
    }
}

fn scalar(ty: &BinaryType) -> bool {
    matches!(
        ty,
        BinaryType::Integer(1..=64)
            | BinaryType::Float(FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64)
    )
}
fn numeric(ty: &BinaryType, types: &BinaryTypeTable) -> bool {
    scalar(ty)
        || matches!(ty, BinaryType::FixedVector { element, .. }
        if types.entries.get(*element as usize).is_some_and(scalar))
}
fn ballot(ty: &BinaryType, types: &BinaryTypeTable) -> bool {
    matches!(ty, BinaryType::FixedVector { lanes: 4, element }
        if matches!(types.entries.get(*element as usize), Some(BinaryType::Integer(32))))
}
fn compatible_reduction_operator(ty: &BinaryType, operator: u64) -> bool {
    match operator {
        2 | 5 | 8 | 11 => matches!(
            ty,
            BinaryType::Float(FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64)
        ),
        0 | 1 | 3 | 4 | 6 | 7 | 9 | 10 | 12 | 13 | 14 => matches!(ty, BinaryType::Integer(1..=64)),
        _ => false,
    }
}
fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
