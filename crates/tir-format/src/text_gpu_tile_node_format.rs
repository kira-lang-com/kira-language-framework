//! Canonical disassembly of tile and work-graph CODE, including output mode.
use std::fmt::Write as _;
use tir_types::{AddressSpace, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, text_gpu_tile_node as node};

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let name = node::NAMES
        .iter()
        .find_map(|(name, opcode)| (*opcode == instruction.opcode).then_some(*name))
        .ok_or_else(|| malformed("unknown tile/node CODE opcode"))?;
    let result_type = node::result_type(instruction.opcode);
    let expected_results = u64::from(result_type.is_some());
    let allowed = if instruction.opcode == 0x5c2 {
        1 << 48
    } else {
        0
    };
    if instruction.result_count != expected_results || instruction.modifiers & !allowed != 0 {
        return Err(malformed(
            "tile/node CODE result count or modifiers are invalid",
        ));
    }
    let count = node::input_count(instruction.opcode);
    let input_indices = match (count, instruction.operands.as_slice()) {
        (0, []) => Vec::new(),
        (1, [BinaryOperand::Value(value)]) => vec![*value],
        (2, [BinaryOperand::Values(values)]) if values.len() == 2 => values.clone(),
        _ => return Err(malformed("tile/node CODE has invalid SSA operand shape")),
    };
    let inputs = input_indices
        .iter()
        .map(|index| {
            usize::try_from(*index)
                .ok()
                .and_then(|index| values.get(index))
                .map(|(name, ty)| (name.as_str(), *ty))
                .ok_or_else(|| malformed("tile/node CODE references undefined SSA value"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !valid_types(instruction.opcode, &inputs, types) {
        return Err(malformed("tile/node CODE operand type is incompatible"));
    }
    let result_name = result_type.as_ref().map(|_| format!("v{}", values.len()));
    if let Some(name) = &result_name {
        write!(text, "    %{name} = ").map_err(|issue| malformed(issue.to_string()))?;
    } else {
        text.push_str("    ");
    }
    text.push_str(name);
    for (position, (name, _)) in inputs.iter().enumerate() {
        write!(text, "{}%{name}", if position == 0 { " " } else { ", " })
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    if instruction.modifiers & (1 << 48) != 0 {
        text.push_str(" [group]");
    }
    text.push('\n');
    if let (Some(name), Some(ty)) = (result_name, result_type) {
        let ty = crate::text_descriptor_reference::type_index(types, &ty)
            .ok_or_else(|| malformed("tile/node CODE result type missing from TYPES"))?
            as u64;
        values.push((name, ty));
    }
    Ok(())
}

fn valid_types(opcode: u64, inputs: &[(&str, u64)], types: &BinaryTypeTable) -> bool {
    let get = |index: usize| {
        inputs
            .get(index)
            .and_then(|(_, ty)| types.entries.get(*ty as usize))
    };
    let integer = |ty: &BinaryType| matches!(ty, BinaryType::Integer(1..=64));
    let node_ptr = |index: usize| {
        crate::text_descriptor_reference::type_index(
            types,
            &Type::Pointer(AddressSpace::NodeRecord),
        )
        .is_some_and(|expected| {
            inputs
                .get(index)
                .is_some_and(|(_, actual)| *actual == expected as u64)
        })
    };
    match opcode {
        0x5a0 => matches!(get(0), Some(BinaryType::FixedVector { lanes: 2, element })
            if matches!(types.entries.get(*element as usize), Some(BinaryType::Integer(32)))),
        0x5c0 => get(0).is_some_and(integer),
        0x5c1 | 0x5c5 => true,
        0x5c2 => get(0).is_some_and(integer) && get(1).is_some_and(integer),
        0x5c3 => node_ptr(0) && get(1).is_some_and(integer),
        0x5c4 => node_ptr(0),
        _ => false,
    }
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
