use crate::FormatError;
use crate::strings::StringTable;
use crate::types::{
    BinaryConstantTable, BinaryFunctionType, BinaryInstruction, BinaryOperand, BinaryTypeTable,
};

#[path = "binary_code_instruction_addresses.rs"]
mod addresses;
#[path = "binary_code_instruction_aggregates.rs"]
mod aggregates;
#[path = "binary_code_instruction_atomics.rs"]
mod atomics;
#[path = "binary_code_instruction_calls.rs"]
mod calls;
#[path = "binary_code_instruction_constants.rs"]
mod constants;
#[path = "binary_code_instruction_control.rs"]
mod control;
#[path = "binary_code_instruction_coroutines.rs"]
mod coroutines;
#[path = "binary_code_instruction_gpu_advanced.rs"]
mod gpu_advanced;
#[path = "binary_code_instruction_gpu_collectives.rs"]
mod gpu_collectives;
#[path = "binary_code_instruction_gpu_control.rs"]
mod gpu_control;
#[path = "binary_code_instruction_gpu_dispatch.rs"]
mod gpu_dispatch;
#[path = "binary_code_instruction_gpu_geometry.rs"]
mod gpu_geometry;
#[path = "binary_code_instruction_gpu_ray.rs"]
mod gpu_ray;
#[path = "binary_code_instruction_gpu_textures.rs"]
mod gpu_textures;
#[path = "binary_code_instruction_heap.rs"]
mod heap;
#[path = "binary_code_instruction_helpers.rs"]
mod helpers;
#[path = "binary_code_instruction_memory.rs"]
mod memory;
#[path = "binary_code_instruction_metadata.rs"]
mod metadata;
#[path = "binary_code_instruction_misc.rs"]
mod misc;
#[path = "binary_code_instruction_ownership.rs"]
mod ownership;
#[path = "binary_code_instruction_speculation.rs"]
mod speculation;
#[path = "binary_code_instruction_symbols.rs"]
mod symbol_refs;
#[path = "binary_code_instruction_unwinding.rs"]
mod unwinding;
#[path = "binary_code_instruction_varargs.rs"]
mod varargs;
#[path = "binary_code_instruction_vectors.rs"]
mod vectors;

use constants::{
    decode_const_aggregate, decode_const_bool, decode_const_bytes, decode_const_float,
    decode_const_int, decode_const_null, decode_const_string, decode_const_vec,
    validate_sampler_descriptor,
};
use helpers::*;

const SELECTOR_MASK: u64 = 0xf << 48;

pub(crate) fn decode_instructions(
    bytes: &[u8],
    base: usize,
    count: u64,
    current_value: u64,
    block_count: u64,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    strings: &StringTable,
    assembly_count: u64,
) -> Result<Option<(Vec<BinaryInstruction>, u64)>, FormatError> {
    let capacity = usize::try_from(count)
        .map_err(|_| malformed(base, "CODE instruction count does not fit in memory"))?;
    let mut instructions = Vec::new();
    instructions
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(base, "CODE instruction allocation failed"))?;
    let mut cursor = 0;
    let mut next_value = current_value;
    for instruction_index in 0..count {
        let Some(instruction) = decode_instruction(
            bytes,
            &mut cursor,
            base,
            next_value,
            block_count,
            function,
            types,
            constants,
            strings,
            assembly_count,
        )?
        else {
            return Ok(None);
        };
        if instruction.terminator && instruction_index + 1 != count {
            return Err(malformed(
                usize::try_from(instruction.offset).unwrap_or(base),
                "CODE terminator is not the final instruction in its block",
            ));
        }
        if instruction_index + 1 == count && !instruction.terminator {
            return Err(malformed(
                usize::try_from(instruction.offset).unwrap_or(base),
                "CODE block does not end in a terminator",
            ));
        }
        next_value = next_value
            .checked_add(instruction.result_count)
            .ok_or_else(|| malformed(base + cursor, "CODE SSA value number overflows"))?;
        instructions.push(instruction);
    }
    if cursor != bytes.len() {
        return Err(malformed(
            base + cursor,
            "trailing bytes after CODE instructions",
        ));
    }
    Ok(Some((instructions, next_value)))
}

#[allow(clippy::too_many_arguments)]
fn decode_instruction(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    block_count: u64,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    strings: &StringTable,
    assembly_count: u64,
) -> Result<Option<BinaryInstruction>, FormatError> {
    let start = *cursor;
    let opcode = read_index(bytes, cursor, base, "instruction opcode")?;
    let modifiers = read_index(bytes, cursor, base, "instruction modifier word")?;
    if modifiers >> 52 != 0 {
        return Err(malformed(
            base + start,
            "reserved CODE modifier bits are set",
        ));
    }
    let result_offset = *cursor;
    let result_count = read_index(bytes, cursor, base, "instruction result count")?;
    let mut operands = Vec::new();
    let decoded = match opcode {
        0x000 => {
            require_no_modifiers(modifiers, base + start)?;
            decode_const_int(bytes, cursor, base, types, &mut operands)?;
            Some(((), 1, false))
        }
        0x001 => {
            require_no_modifiers(modifiers, base + start)?;
            decode_const_float(bytes, cursor, base, types, constants, &mut operands)?;
            Some(((), 1, false))
        }
        0x002 => {
            require_no_modifiers(modifiers, base + start)?;
            decode_const_bool(bytes, cursor, base, &mut operands)?;
            Some(((), 1, false))
        }
        0x003 => {
            require_no_modifiers(modifiers, base + start)?;
            decode_const_null(bytes, cursor, base, &mut operands)?;
            Some(((), 1, false))
        }
        0x004 | 0x005 => {
            require_no_modifiers(modifiers, base + start)?;
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            Some(((), 1, false))
        }
        0x006 => {
            require_no_modifiers(modifiers, base + start)?;
            decode_const_string(bytes, cursor, base, strings, &mut operands)?;
            Some(((), 1, false))
        }
        0x007 => {
            require_no_modifiers(modifiers, base + start)?;
            decode_const_bytes(bytes, cursor, base, types, &mut operands)?;
            Some(((), 1, false))
        }
        0x008 => {
            require_no_modifiers(modifiers, base + start)?;
            decode_const_vec(bytes, cursor, base, types, constants, &mut operands)?;
            Some(((), 1, false))
        }
        0x009 => {
            require_no_modifiers(modifiers, base + start)?;
            decode_const_aggregate(bytes, cursor, base, types, constants, &mut operands)?;
            Some(((), 1, false))
        }
        0x00a..=0x010 => {
            let results = symbol_refs::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                start,
                block_count,
                types,
                strings,
                &mut operands,
            )?;
            Some(((), results, false))
        }
        0x011 => {
            require_no_modifiers(modifiers, base + start)?;
            let payload = read_bytes(bytes, cursor, base, 23, "sampler descriptor")?;
            validate_sampler_descriptor(&payload, base + *cursor - payload.len())?;
            operands.push(BinaryOperand::Bytes(payload));
            Some(((), 1, false))
        }
        0x020..=0x045 => {
            let relative = opcode - 0x020;
            let selector = (modifiers & SELECTOR_MASK) >> 48;
            if (relative == 28 && selector > 9) || (relative != 28 && selector != 0) {
                return Err(malformed(
                    base + start,
                    "invalid integer instruction selector",
                ));
            }
            let values = match relative {
                14 | 15 | 16 | 30 | 31 | 32 | 33 | 34 | 37 => 1,
                22 | 23 | 29 => 3,
                _ => 2,
            };
            read_values(bytes, cursor, base, current_value, values, &mut operands)?;
            Some(((), 1, false))
        }
        0x060..=0x092 => {
            let relative = opcode - 0x060;
            let selector = (modifiers & SELECTOR_MASK) >> 48;
            if (relative == 20 && selector > 15) || (relative != 20 && selector != 0) {
                return Err(malformed(
                    base + start,
                    "invalid floating-point instruction selector",
                ));
            }
            let values = match relative {
                5 | 6 | 7 | 15..=19 | 21 | 22 | 26..=31 | 33..=46 | 48 | 50 => 1,
                8 | 9 => 3,
                24 | 25 => 0,
                _ => 2,
            };
            read_values(bytes, cursor, base, current_value, values, &mut operands)?;
            let results = if relative == 25 { 0 } else { 1 };
            Some(((), results, false))
        }
        0x0a0..=0x0ac => {
            let selector = (modifiers & SELECTOR_MASK) >> 48;
            if (opcode == 0x0ac && selector > 2) || (opcode != 0x0ac && selector != 0) {
                return Err(malformed(
                    base + start,
                    "invalid conversion instruction selector",
                ));
            }
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            Some(((), 1, false))
        }
        0x0c0..=0x0d7 => {
            let results = vectors::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                types,
                &mut operands,
            )?;
            Some(((), results, false))
        }
        0x100..=0x10d => {
            let expected_results = aggregates::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                result_count,
                types,
                &mut operands,
            )?;
            Some(((), expected_results, false))
        }
        0x120..=0x128 => {
            let results = addresses::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                types,
                &mut operands,
            )?;
            Some(((), results, false))
        }
        0x140..=0x14d => {
            let results = memory::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                types,
                &mut operands,
            )?;
            Some(((), results, false))
        }
        0x160..=0x16a => {
            let results = heap::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                types,
                &mut operands,
            )?;
            Some(((), results, false))
        }
        0x180..=0x188 => {
            let results = ownership::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                types,
                &mut operands,
            )?;
            Some(((), results, false))
        }
        0x200..=0x206 => {
            let results = atomics::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                types,
                &mut operands,
            )?;
            Some(((), results, false))
        }
        0x220..=0x224 | 0x230 | 0x240..=0x246 => {
            let (results, terminator) = metadata::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                block_count,
                types,
                strings,
                assembly_count,
                &mut operands,
            )?;
            Some(((), results, terminator))
        }
        0x250..=0x252 | 0x270..=0x273 | 0x280..=0x282 => {
            let results = misc::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                types,
                &mut operands,
            )?;
            Some(((), results, false))
        }
        0x260..=0x261 => {
            let (results, terminator) = speculation::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                &mut operands,
            )?;
            Some(((), results, terminator))
        }
        0x1d0..=0x1d4 => {
            let (results, terminator) = unwinding::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                types,
                &mut operands,
            )?;
            Some(((), results, terminator))
        }
        0x1a0..=0x1aa | 0x1e7 => {
            let (results, terminator) = calls::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                start,
                current_value,
                block_count,
                result_count,
                types,
                strings,
                &mut operands,
            )?;
            Some(((), results, terminator))
        }
        0x1c0..=0x1c9 => {
            let (results, terminator) = coroutines::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                start,
                current_value,
                block_count,
                result_count,
                &mut operands,
            )?;
            Some(((), results, terminator))
        }
        0x210..=0x215 => {
            let (results, terminator) = varargs::decode(
                opcode,
                modifiers,
                bytes,
                cursor,
                base,
                current_value,
                types,
                &mut operands,
            )?;
            Some(((), results, terminator))
        }
        0x1e0 => {
            require_no_modifiers(modifiers, base + start)?;
            operands.push(read_block_target(
                bytes,
                cursor,
                base,
                current_value,
                block_count,
            )?);
            Some(((), 0, true))
        }
        0x1e1 => {
            require_no_modifiers(modifiers, base + start)?;
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            operands.push(read_block_target(
                bytes,
                cursor,
                base,
                current_value,
                block_count,
            )?);
            operands.push(read_block_target(
                bytes,
                cursor,
                base,
                current_value,
                block_count,
            )?);
            operands.push(BinaryOperand::Index(read_merge_target(
                bytes,
                cursor,
                base,
                block_count,
            )?));
            Some(((), 0, true))
        }
        0x1e2 => {
            require_no_modifiers(modifiers, base + start)?;
            decode_switch_int(
                bytes,
                cursor,
                base,
                current_value,
                block_count,
                &mut operands,
            )?;
            Some(((), 0, true))
        }
        0x1e3 | 0x1e4 => {
            require_no_modifiers(modifiers, base + start)?;
            control::decode_indirect_or_enum_branch(
                opcode,
                bytes,
                cursor,
                base,
                current_value,
                block_count,
                &mut operands,
            )?;
            Some(((), 0, true))
        }
        0x1e5 => {
            require_no_modifiers(modifiers, base + start)?;
            let results = if is_unit(function.result, types) {
                0
            } else {
                1
            };
            read_values(bytes, cursor, base, current_value, results, &mut operands)?;
            Some(((), 0, true))
        }
        0x1e6 => {
            require_no_modifiers(modifiers, base + start)?;
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            Some(((), 0, true))
        }
        0x1e8 => {
            if modifiers & !SELECTOR_MASK != 0 || ((modifiers & SELECTOR_MASK) >> 48) > 1 {
                return Err(malformed(base + start, "invalid unreachable modifier"));
            }
            Some(((), 0, true))
        }
        0x1e9 => {
            require_no_modifiers(modifiers, base + start)?;
            let code_offset = *cursor;
            let code = read_integer(bytes, cursor, base, "trap code")?;
            if code.negative || code.magnitude > u128::from(u16::MAX) {
                return Err(malformed(code_offset, "trap code must fit in u16"));
            }
            operands.push(BinaryOperand::Integer(code));
            Some(((), 0, true))
        }
        _ => match gpu_dispatch::decode(
            opcode,
            modifiers,
            bytes,
            cursor,
            base,
            start,
            current_value,
            types,
            constants,
            strings,
            &mut operands,
        )? {
            Some((results, terminator)) => Some(((), results, terminator)),
            None => None,
        },
    };
    let Some(((), expected_results, terminator)) = decoded else {
        return Err(malformed(
            base + start,
            format!("unassigned TIR CODE opcode 0x{opcode:03X}"),
        ));
    };
    if result_count != expected_results {
        return Err(malformed(
            result_offset,
            "CODE instruction result count disagrees with opcode",
        ));
    }
    Ok(Some(BinaryInstruction {
        offset: (base + start) as u64,
        opcode,
        modifiers,
        result_count,
        terminator,
        operands,
    }))
}

fn decode_switch_int(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    block_count: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    read_values(bytes, cursor, base, current_value, 1, operands)?;
    let case_offset = *cursor;
    let case_count = read_index(bytes, cursor, base, "switch.int case count")?;
    if case_count >= MAX_LIST_COUNT || case_count > bytes.len().saturating_sub(*cursor) as u64 / 2 {
        return Err(malformed(
            case_offset,
            "switch.int case count exceeds block payload",
        ));
    }
    let capacity = usize::try_from(case_count)
        .map_err(|_| malformed(case_offset, "switch.int case count overflows memory"))?;
    let mut cases = Vec::new();
    cases
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(*cursor, "switch.int case allocation failed"))?;
    for _ in 0..case_count {
        let value = read_integer(bytes, cursor, base, "switch.int case value")?;
        let target = read_block_target(bytes, cursor, base, current_value, block_count)?;
        let BinaryOperand::BlockTarget { block, arguments } = target else {
            unreachable!();
        };
        cases.push((value, block, arguments));
    }
    operands.push(BinaryOperand::SwitchCases(cases));
    operands.push(read_block_target(
        bytes,
        cursor,
        base,
        current_value,
        block_count,
    )?);
    operands.push(BinaryOperand::Index(read_merge_target(
        bytes,
        cursor,
        base,
        block_count,
    )?));
    Ok(())
}
