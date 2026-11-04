use crate::binary_code_blocks::decode_function_body;
use crate::leb::write_uleb;
use crate::types::{BinaryCodeTable, BinaryConstantTable, BinarySignatureTable, BinaryTypeTable};
use crate::{FormatError, StringTable};

const MAX_FUNCTION_COUNT: usize = 1 << 32;
const MAX_BLOCK_COUNT: usize = 1 << 24;
const MAX_INSTRUCTION_COUNT: u64 = 1 << 24;

pub(super) fn encode_code_table(
    table: &BinaryCodeTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    strings: &StringTable,
    assembly_count: u64,
) -> Result<Vec<u8>, FormatError> {
    if table.functions.len() >= MAX_FUNCTION_COUNT {
        return Err(malformed("CODE function count exceeds the limit"));
    }

    let mut output = Vec::new();
    write_index(table.functions.len() as u64, "function count", &mut output)?;
    let mut previous_function = None;
    for function in &table.functions {
        if usize::try_from(function.function_index)
            .map_or(true, |index| index >= signatures.records.len())
            || previous_function.is_some_and(|previous| function.function_index <= previous)
        {
            return Err(malformed(
                "CODE function indexes are invalid or not strictly ordered",
            ));
        }
        previous_function = Some(function.function_index);
        if usize::try_from(function.block_count).ok() != Some(function.blocks.len())
            || function.blocks.is_empty()
            || function.blocks.len() >= MAX_BLOCK_COUNT
        {
            return Err(malformed("CODE block count does not match its body"));
        }

        let body = encode_function_body(
            function,
            signatures,
            types,
            constants,
            strings,
            assembly_count,
        )?;
        write_index(function.function_index, "function index", &mut output)?;
        write_index(body.len() as u64, "function body size", &mut output)?;
        output.extend_from_slice(&body);
    }
    Ok(output)
}

fn encode_function_body(
    function: &crate::types::BinaryFunctionBody,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    strings: &StringTable,
    assembly_count: u64,
) -> Result<Vec<u8>, FormatError> {
    let mut body = Vec::new();
    write_index(function.blocks.len() as u64, "block count", &mut body)?;
    for block in &function.blocks {
        if block.instruction_count == 0 || block.instruction_count >= MAX_INSTRUCTION_COUNT {
            return Err(malformed("CODE instruction count must be in 1..2^24"));
        }
        if block
            .instructions
            .as_ref()
            .is_some_and(|instructions| instructions.len() as u64 != block.instruction_count)
        {
            return Err(malformed(
                "CODE decoded instruction count does not match its block",
            ));
        }
        let mut payload = Vec::new();
        let mut flags = u8::from(block.cold);
        match (block.loop_merge, block.loop_continue) {
            (Some(merge), Some(continue_block)) => {
                flags |= 0x02;
                write_index(merge, "loop merge block", &mut payload)?;
                write_index(continue_block, "loop continue block", &mut payload)?;
            }
            (None, None) => {}
            _ => return Err(malformed("CODE loop targets must appear as a pair")),
        }
        if block.parameters.len() >= MAX_FUNCTION_COUNT {
            return Err(malformed("CODE block parameter count exceeds the limit"));
        }
        let mut record = vec![flags];
        write_index(
            block.parameters.len() as u64,
            "block parameter count",
            &mut record,
        )?;
        for parameter in &block.parameters {
            if parameter.convention > 4 {
                return Err(malformed("invalid CODE parameter convention"));
            }
            let convention = parameter.convention | (u8::from(parameter.context) << 7);
            record.push(convention);
            write_index(parameter.ty, "parameter type", &mut record)?;
        }
        record.extend_from_slice(&payload);
        write_index(block.instruction_count, "instruction count", &mut record)?;
        record.extend_from_slice(&block.instruction_bytes);
        write_index(record.len() as u64, "block size", &mut body)?;
        body.extend_from_slice(&record);
    }

    let decoded = decode_function_body(
        &body,
        0,
        function.function_index,
        body.len() as u64,
        signatures,
        types,
        constants,
        strings,
        assembly_count,
    )?;
    if decoded.blocks.len() != function.blocks.len() {
        return Err(malformed("encoded CODE function body did not round-trip"));
    }
    for (encoded, decoded) in function.blocks.iter().zip(&decoded.blocks) {
        let Some(expected) = &encoded.instructions else {
            continue;
        };
        let Some(actual) = &decoded.instructions else {
            return Err(malformed(
                "typed CODE instructions disagree with the encoded instruction bytes",
            ));
        };
        let same_instructions = expected.len() == actual.len()
            && expected.iter().zip(actual).all(|(left, right)| {
                left.opcode == right.opcode
                    && left.modifiers == right.modifiers
                    && left.result_count == right.result_count
                    && left.terminator == right.terminator
                    && left.operands == right.operands
            });
        if !same_instructions {
            return Err(malformed(
                "typed CODE instructions disagree with the encoded instruction bytes",
            ));
        }
    }
    Ok(body)
}

fn write_index(value: u64, field: &str, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, output)
        .map_err(|error| FormatError::malformed(None, format!("CODE {field}: {error}")))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
