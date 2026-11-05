//! Canonical text reconstruction for the supported direct stack and memory CODE.
use std::fmt::Write as _;

use crate::FormatError;
use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};

pub(crate) fn handles(opcode: u64) -> bool {
    matches!(opcode, 0x120 | 0x121 | 0x140..=0x145)
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let opcode = instruction.opcode;
    let selector = instruction.modifiers >> 48;
    let expected_results = u64::from(matches!(opcode, 0x120 | 0x140 | 0x141));
    if instruction.result_count != expected_results {
        return Err(malformed("memory result count does not match the opcode"));
    }
    if opcode == 0x120 {
        let [
            BinaryOperand::Index(space),
            BinaryOperand::Type(pointee),
            BinaryOperand::OptionalValue(None),
        ] = instruction.operands.as_slice()
        else {
            return Err(malformed("unsupported dynamic stack allocation operands"));
        };
        if *space != 0 || selector != 0 {
            return Err(malformed("only generic stack addresses are supported"));
        }
        let width = match get_type(types, *pointee)? {
            BinaryType::Integer(bits) => *bits,
            _ => {
                return Err(malformed(
                    "only integer stack allocations can be disassembled",
                ));
            }
        };
        let address = types.entries.iter().position(|entry| {
            matches!(entry, BinaryType::Address { pointee: pointee_index, space: 0 } if pointee_index == pointee)
        }).ok_or_else(|| malformed("stack address result type is not interned"))?;
        let result = format!("v{}", values.len());
        writeln!(text, "    %{result} = stack.alloc i{width}").map_err(formatting_error)?;
        values.push((result, address as u64));
        return Ok(());
    }
    let opcode_name = match opcode {
        0x121 => "stack.dealloc",
        0x140 => "load",
        0x141 => "load.borrow",
        0x142 => "store",
        0x143 => "copy.addr",
        0x144 => "move.addr",
        0x145 => "destroy.addr",
        _ => return Err(malformed("unsupported memory opcode")),
    };
    let mode = match (opcode, selector) {
        (0x140, 0) | (0x142, 0) => "trivial",
        (0x140, 1) => "copy",
        (0x140, 2) => "take",
        (0x142, 1) => "init",
        (0x142, 2) => "assign",
        (0x143 | 0x144, 0) => "init",
        (0x143 | 0x144, 1) => "assign",
        (0x121 | 0x141 | 0x145, 0) => "",
        _ => return Err(malformed("invalid memory operation selector")),
    };
    if matches!(opcode, 0x121 | 0x140 | 0x141 | 0x145) {
        let [BinaryOperand::Value(value)] = instruction.operands.as_slice() else {
            return Err(malformed("memory operation expects one SSA value"));
        };
        let (name, ty) = get_value(values, *value)?;
        let ty = address_pointee(types, ty)?;
        if matches!(opcode, 0x140 | 0x141) {
            let result = format!("v{}", values.len());
            if opcode == 0x140 {
                writeln!(text, "    %{result} = load [{mode}] %{name}")
                    .map_err(formatting_error)?;
            } else {
                writeln!(text, "    %{result} = load.borrow %{name}").map_err(formatting_error)?;
            }
            values.push((result, ty));
        } else {
            writeln!(text, "    {opcode_name} %{name}").map_err(formatting_error)?;
        }
    } else {
        let [BinaryOperand::Values(indices)] = instruction.operands.as_slice() else {
            return Err(malformed("memory transfer expects two SSA values"));
        };
        let [source, destination] = indices.as_slice() else {
            return Err(malformed("memory transfer has the wrong operand count"));
        };
        let (source_name, source_ty) = get_value(values, *source)?;
        let (dest_name, dest_ty) = get_value(values, *destination)?;
        let dest_pointee = address_pointee(types, dest_ty)?;
        let actual_source = if opcode == 0x142 {
            source_ty
        } else {
            address_pointee(types, source_ty)?
        };
        if actual_source != dest_pointee {
            return Err(malformed(
                "memory transfer source and destination types disagree",
            ));
        }
        if opcode == 0x142 {
            writeln!(text, "    store [{mode}] %{source_name} to %{dest_name}")
                .map_err(formatting_error)?;
        } else {
            writeln!(
                text,
                "    {opcode_name} %{source_name} to [{mode}] %{dest_name}"
            )
            .map_err(formatting_error)?;
        }
    }
    Ok(())
}

fn address_pointee(types: &BinaryTypeTable, index: u64) -> Result<u64, FormatError> {
    match get_type(types, index)? {
        BinaryType::Address { pointee, .. } => Ok(*pointee),
        _ => Err(malformed("memory operation operand is not a typed address")),
    }
}

fn get_type(types: &BinaryTypeTable, index: u64) -> Result<&BinaryType, FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| types.entries.get(index))
        .ok_or_else(|| malformed("memory type index is out of range"))
}

fn get_value(values: &[(String, u64)], index: u64) -> Result<(&str, u64), FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| values.get(index))
        .map(|(name, ty)| (name.as_str(), *ty))
        .ok_or_else(|| malformed("memory SSA value index is out of range"))
}

fn malformed(message: &'static str) -> FormatError {
    FormatError::malformed(None, message)
}
fn formatting_error(err: std::fmt::Error) -> FormatError {
    FormatError::malformed(None, err.to_string())
}
