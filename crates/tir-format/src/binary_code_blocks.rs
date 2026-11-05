use crate::FormatError;
use crate::binary_code_instructions::decode_instructions;
use crate::leb::read_uleb;
use crate::strings::StringTable;
use crate::types::{
    BinaryBlock, BinaryBlockParameter, BinaryConstantTable, BinaryFunctionBody, BinaryFunctionType,
    BinarySignatureTable, BinaryType, BinaryTypeTable,
};

const MAX_BLOCK_COUNT: u64 = 1 << 24;
const MAX_INSTRUCTION_COUNT: u64 = 1 << 24;

pub(crate) fn decode_function_body(
    bytes: &[u8],
    base: usize,
    function_index: u64,
    body_byte_size: u64,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    strings: &StringTable,
    assembly_count: u64,
) -> Result<BinaryFunctionBody, FormatError> {
    let signature = signatures
        .records
        .get(
            usize::try_from(function_index)
                .map_err(|_| malformed(base, "CODE function index does not fit in memory"))?,
        )
        .ok_or_else(|| malformed(base, "CODE function index is out of range"))?;
    let function_type_index = usize::try_from(signature.function_type)
        .map_err(|_| malformed(base, "CODE function type index does not fit in memory"))?;
    let function_type = match types.entries.get(function_type_index) {
        Some(BinaryType::Function(function)) => function,
        _ => {
            return Err(malformed(
                base,
                "CODE signature type is not a function type",
            ));
        }
    };

    let mut cursor = 0;
    let count_offset = cursor;
    let block_count = read_index(bytes, &mut cursor, base, "block count")?;
    if block_count == 0 || block_count >= MAX_BLOCK_COUNT {
        return Err(malformed(
            base + count_offset,
            "CODE block count must be in 1..2^24",
        ));
    }
    let block_capacity = usize::try_from(block_count).map_err(|_| {
        malformed(
            base + count_offset,
            "CODE block count does not fit in memory",
        )
    })?;
    if block_capacity > bytes.len().saturating_sub(cursor) / 4 {
        return Err(malformed(
            base + cursor,
            "CODE block count exceeds the bounded function body",
        ));
    }
    let mut blocks = Vec::new();
    blocks
        .try_reserve_exact(block_capacity)
        .map_err(|_| malformed(base + cursor, "CODE block allocation failed"))?;
    let mut current_value = Some(u64::try_from(function_type.parameters.len()).map_err(|_| {
        malformed(
            base + cursor,
            "CODE parameter count overflows SSA numbering",
        )
    })?);

    for block_index in 0..block_count {
        let size_offset = cursor;
        let byte_size = read_index(bytes, &mut cursor, base, "block byte size")?;
        let payload_size = usize::try_from(byte_size)
            .map_err(|_| malformed(base + size_offset, "CODE block size overflows memory"))?;
        let block_end = cursor
            .checked_add(payload_size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| malformed(base + size_offset, "CODE block exceeds its function body"))?;
        let block_start = cursor;
        let block_bytes = &bytes[block_start..block_end];
        let block = decode_block(
            block_bytes,
            base + block_start,
            block_index,
            block_count,
            &function_type,
            types,
            constants,
            strings,
            assembly_count,
            current_value,
        )?;
        current_value = block.1;
        cursor = block_end;
        blocks.push(block.0);
    }
    if cursor != bytes.len() {
        return Err(malformed(
            base + cursor,
            "trailing bytes in CODE function body",
        ));
    }
    Ok(BinaryFunctionBody {
        function_index,
        body_byte_size,
        block_count,
        blocks,
    })
}

fn decode_block(
    bytes: &[u8],
    base: usize,
    block_index: u64,
    block_count: u64,
    function_type: &BinaryFunctionType,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    strings: &StringTable,
    assembly_count: u64,
    current_value: Option<u64>,
) -> Result<(BinaryBlock, Option<u64>), FormatError> {
    let mut cursor = 0;
    let flags = read_byte(bytes, &mut cursor, base, "block flags")?;
    if flags & !0x03 != 0 {
        return Err(malformed(base, "reserved CODE block flag bits are set"));
    }
    let parameter_offset = cursor;
    let parameter_count = read_index(bytes, &mut cursor, base, "block parameter count")?;
    let parameter_capacity = usize::try_from(parameter_count).map_err(|_| {
        malformed(
            base + parameter_offset,
            "CODE parameter count does not fit in memory",
        )
    })?;
    if parameter_capacity > bytes.len().saturating_sub(cursor) / 2 {
        return Err(malformed(
            base + cursor,
            "CODE parameter count exceeds block size",
        ));
    }
    if block_index == 0 && parameter_capacity != function_type.parameters.len() {
        return Err(malformed(
            base + parameter_offset,
            "entry block parameters do not match function signature arity",
        ));
    }
    let mut parameters = Vec::new();
    parameters
        .try_reserve_exact(parameter_capacity)
        .map_err(|_| malformed(base + cursor, "CODE parameter allocation failed"))?;
    for parameter_index in 0..parameter_capacity {
        let parameter_offset = cursor;
        let convention_context = read_byte(bytes, &mut cursor, base, "parameter convention")?;
        let convention = convention_context & 0x07;
        if convention > 4 || convention_context & 0x78 != 0 {
            return Err(malformed(
                base + parameter_offset,
                "invalid CODE parameter convention/context byte",
            ));
        }
        let ty_offset = cursor;
        let ty = read_index(bytes, &mut cursor, base, "parameter type")?;
        if usize::try_from(ty).map_or(true, |index| index >= types.entries.len()) {
            return Err(malformed(
                base + ty_offset,
                "CODE parameter type index is invalid",
            ));
        }
        let parameter = BinaryBlockParameter {
            convention,
            context: convention_context & 0x80 != 0,
            ty,
        };
        if block_index == 0 {
            let signature_parameter = function_type.parameters[parameter_index];
            if convention != signature_parameter.convention
                || parameter.context != signature_parameter.context
                || ty != signature_parameter.ty
            {
                return Err(malformed(
                    base + parameter_offset,
                    "entry block parameter does not match function signature",
                ));
            }
        }
        parameters.push(parameter);
    }

    let (loop_merge, loop_continue) = if flags & 0x02 != 0 {
        let merge_offset = cursor;
        let merge = read_index(bytes, &mut cursor, base, "loop merge block")?;
        let continue_offset = cursor;
        let continue_block = read_index(bytes, &mut cursor, base, "loop continue block")?;
        if merge >= block_count || continue_block >= block_count {
            return Err(malformed(
                base + merge_offset.min(continue_offset),
                "CODE loop target is out of range",
            ));
        }
        (Some(merge), Some(continue_block))
    } else {
        (None, None)
    };

    let instruction_count_offset = cursor;
    let instruction_count = read_index(bytes, &mut cursor, base, "instruction count")?;
    if instruction_count == 0 || instruction_count >= MAX_INSTRUCTION_COUNT {
        return Err(malformed(
            base + instruction_count_offset,
            "CODE instruction count must be in 1..2^24",
        ));
    }
    let instruction_bytes = &bytes[cursor..];
    if instruction_count > (instruction_bytes.len() / 2) as u64 {
        return Err(malformed(
            base + cursor,
            "CODE instruction count exceeds block payload",
        ));
    }
    let mut stored_instruction_bytes = Vec::new();
    stored_instruction_bytes
        .try_reserve_exact(instruction_bytes.len())
        .map_err(|_| malformed(base + cursor, "CODE instruction-byte allocation failed"))?;
    stored_instruction_bytes.extend_from_slice(instruction_bytes);
    let current_value =
        if block_index == 0 {
            current_value
        } else if let Some(current_value) = current_value {
            Some(current_value.checked_add(parameter_count).ok_or_else(|| {
                malformed(base + parameter_offset, "CODE SSA value number overflows")
            })?)
        } else {
            None
        };
    let decoded = if let Some(current_value) = current_value {
        decode_instructions(
            instruction_bytes,
            base + cursor,
            instruction_count,
            current_value,
            block_count,
            function_type,
            types,
            constants,
            strings,
            assembly_count,
        )?
    } else {
        None
    };
    let (instructions, next_value) = match decoded {
        Some((instructions, next_value)) => (Some(instructions), Some(next_value)),
        None => (None, None),
    };
    Ok((
        BinaryBlock {
            cold: flags & 0x01 != 0,
            loop_merge,
            loop_continue,
            parameters,
            instruction_count,
            instruction_bytes: stored_instruction_bytes,
            instructions,
        },
        next_value,
    ))
}

fn read_index(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    field: &str,
) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor).map_err(|error| {
        malformed(
            base + offset,
            format!("invalid CODE {field} ULEB128: {error:?}"),
        )
    })
}

fn read_byte(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    field: &str,
) -> Result<u8, FormatError> {
    let offset = *cursor;
    let value = *bytes
        .get(offset)
        .ok_or_else(|| malformed(base + offset, format!("truncated CODE {field}")))?;
    *cursor += 1;
    Ok(value)
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
