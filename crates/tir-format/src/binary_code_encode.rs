use crate::leb::write_uleb;
use crate::types::{BinaryCodeTable, BinaryConstantTable, BinarySignatureTable, BinaryTypeTable};
use crate::{FormatError, StringTable};

const MAX_FUNCTION_COUNT: usize = 1 << 32;
const MAX_BLOCK_COUNT: usize = 1 << 24;
const MAX_INSTRUCTION_COUNT: u64 = 1 << 24;

impl BinaryCodeTable {
    /// Encodes function bodies in strictly increasing SIGNATURES order.
    pub fn encode(
        &self,
        signatures: &BinarySignatureTable,
        types: &BinaryTypeTable,
        constants: &BinaryConstantTable,
        strings: &StringTable,
        assembly_count: u64,
    ) -> Result<Vec<u8>, FormatError> {
        if self.functions.len() >= MAX_FUNCTION_COUNT {
            return Err(malformed("CODE function count must be less than 2^32"));
        }
        let mut bytes = Vec::new();
        write_count(&mut bytes, self.functions.len())?;
        let mut previous_function = None;
        for function in &self.functions {
            if usize::try_from(function.function_index)
                .map_or(true, |index| index >= signatures.records.len())
                || previous_function.is_some_and(|previous| function.function_index <= previous)
            {
                return Err(malformed(
                    "CODE function indexes must be in increasing SIGNATURES order",
                ));
            }
            previous_function = Some(function.function_index);
            let body = encode_function_body(function)?;
            crate::binary_code_blocks::decode_function_body(
                &body,
                0,
                function.function_index,
                u64::try_from(body.len()).map_err(|_| malformed("CODE body size overflows"))?,
                signatures,
                types,
                constants,
                strings,
                assembly_count,
            )?;
            write_uleb(function.function_index, &mut bytes).map_err(io_error)?;
            write_count(&mut bytes, body.len())?;
            bytes.extend_from_slice(&body);
        }
        Ok(bytes)
    }
}

pub(crate) fn encode_function_body(
    function: &crate::types::BinaryFunctionBody,
) -> Result<Vec<u8>, FormatError> {
    if function.block_count == 0
        || function.block_count >= MAX_BLOCK_COUNT as u64
        || usize::try_from(function.block_count).ok() != Some(function.blocks.len())
    {
        return Err(malformed("CODE block count does not match its body"));
    }
    let mut bytes = Vec::new();
    write_uleb(function.block_count, &mut bytes).map_err(io_error)?;
    for block in &function.blocks {
        if block.instruction_count == 0 || block.instruction_count >= MAX_INSTRUCTION_COUNT {
            return Err(malformed("CODE instruction count must be in 1..2^24"));
        }
        if block
            .parameters
            .iter()
            .any(|parameter| parameter.convention > 4)
        {
            return Err(malformed("invalid CODE parameter convention"));
        }
        if block.loop_merge.is_some() != block.loop_continue.is_some() {
            return Err(malformed("CODE loop header is missing a loop target"));
        }
        let loop_header = block.loop_merge.is_some();
        if block
            .loop_merge
            .into_iter()
            .chain(block.loop_continue)
            .any(|target| target >= function.block_count)
        {
            return Err(malformed("CODE loop target is out of range"));
        }
        let mut payload = Vec::new();
        payload.push(u8::from(block.cold) | (u8::from(loop_header) << 1));
        write_count(&mut payload, block.parameters.len())?;
        for parameter in &block.parameters {
            payload.push(parameter.convention | (u8::from(parameter.context) << 7));
            write_uleb(parameter.ty, &mut payload).map_err(io_error)?;
        }
        if let (Some(merge), Some(continue_block)) = (block.loop_merge, block.loop_continue) {
            write_uleb(merge, &mut payload).map_err(io_error)?;
            write_uleb(continue_block, &mut payload).map_err(io_error)?;
        }
        write_uleb(block.instruction_count, &mut payload).map_err(io_error)?;
        if block.instruction_bytes.is_empty() {
            return Err(malformed("CODE block has no instruction bytes"));
        }
        payload.extend_from_slice(&block.instruction_bytes);
        write_count(&mut bytes, payload.len())?;
        bytes.extend_from_slice(&payload);
    }
    Ok(bytes)
}

fn write_count(bytes: &mut Vec<u8>, count: usize) -> Result<(), FormatError> {
    write_uleb(
        u64::try_from(count).map_err(|_| malformed("CODE count overflows ULEB128"))?,
        bytes,
    )
    .map_err(io_error)
}

fn io_error(error: std::io::Error) -> FormatError {
    malformed(format!("CODE encoding failed: {error}"))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
