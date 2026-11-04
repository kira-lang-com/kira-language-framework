use tir_format::leb::{read_uleb, write_uleb};

use crate::binary::{FormatError, Section, TvisaImage};
use crate::function_index::{FunctionIndexRecord, encode_function_index};

use super::{BinaryFunctionBody, BinaryInstruction};

impl TvisaImage {
    /// Re-encode decoded CODE bodies and their FUNCINDEX ranges.
    pub fn encode_code_sections(
        &self,
        memory_limit: u64,
    ) -> Result<(Section, Section), FormatError> {
        let bodies = self.function_bodies(memory_limit)?;
        if bodies.len() != self.header.function_count as usize {
            return Err(malformed(
                0,
                "function body count does not match the image header",
            ));
        }
        let (code_bytes, records) = encode_function_bodies(&bodies)?;
        let code_size = u64::try_from(code_bytes.len())
            .map_err(|_| malformed(0, "CODE size does not fit in the image"))?;
        let index_bytes =
            encode_function_index(&records, code_size, self.header.claims_canonical())?;
        let function_index_section = Section::from_uncompressed(8, index_bytes);
        let code_section = Section::from_uncompressed(9, code_bytes);

        let mut redecoded = self.clone();
        replace_section(&mut redecoded.sections, function_index_section.clone());
        replace_section(&mut redecoded.sections, code_section.clone());
        let round_trip = redecoded.function_bodies(memory_limit)?;
        if round_trip.len() != bodies.len()
            || round_trip
                .iter()
                .zip(&bodies)
                .any(|(left, right)| !same_body_program(left, right))
        {
            return Err(malformed(
                0,
                "re-encoded CODE does not preserve the decoded body records",
            ));
        }
        Ok((function_index_section, code_section))
    }
}

/// Encode decoded TVISA bodies while retaining the original instruction bytes.
/// Returns concatenated CODE bytes and their FUNCINDEX ranges in body order.
pub fn encode_function_bodies(
    bodies: &[BinaryFunctionBody],
) -> Result<(Vec<u8>, Vec<FunctionIndexRecord>), FormatError> {
    if bodies.len() >= 1 << 28 {
        return Err(malformed(0, "function count exceeds the TVISA limit"));
    }
    let mut code = Vec::new();
    let mut index = Vec::new();
    index
        .try_reserve_exact(bodies.len())
        .map_err(|_| malformed(0, "FUNCINDEX allocation failed"))?;

    let mut previous_function = None;
    for body in bodies {
        if previous_function.is_some_and(|previous: u64| {
            previous.checked_add(1) != Some(body.header.function_index)
        }) {
            return Err(malformed(
                code.len(),
                "function bodies are not in global function order",
            ));
        }
        previous_function = Some(body.header.function_index);

        let bytes = encode_body(body)?;
        let offset = u64::try_from(code.len())
            .map_err(|_| malformed(code.len(), "CODE offset overflows"))?;
        let size = u32::try_from(bytes.len())
            .map_err(|_| malformed(code.len(), "function body exceeds the FUNCINDEX size limit"))?;
        let end = code
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| malformed(code.len(), "CODE size overflows"))?;
        code.try_reserve(bytes.len())
            .map_err(|_| malformed(code.len(), "CODE allocation failed"))?;
        code.extend_from_slice(&bytes);
        index.push(FunctionIndexRecord { offset, size });
        if code.len() != end {
            return Err(malformed(code.len(), "CODE size changed while encoding"));
        }
    }
    Ok((code, index))
}

fn replace_section(sections: &mut Vec<Section>, replacement: Section) {
    sections.retain(|section| section.kind != replacement.kind);
    sections.push(replacement);
}

fn same_body_program(left: &BinaryFunctionBody, right: &BinaryFunctionBody) -> bool {
    left.header.function_index == right.header.function_index
        && left.header.block_count == right.header.block_count
        && left.header.value_count == right.header.value_count
        && left.blocks == right.blocks
}

fn encode_body(body: &BinaryFunctionBody) -> Result<Vec<u8>, FormatError> {
    let block_count = u64::try_from(body.blocks.len())
        .map_err(|_| malformed(0, "function block count overflows"))?;
    if body.header.function_index >= 1 << 28 {
        return Err(malformed(0, "function index exceeds the TVISA limit"));
    }
    if block_count == 0 || block_count >= 1 << 24 || block_count != body.header.block_count {
        return Err(malformed(0, "function block count does not match its body"));
    }

    let mut value_count = 0u64;
    for block in &body.blocks {
        value_count = value_count
            .checked_add(
                u64::try_from(block.parameter_types.len())
                    .map_err(|_| malformed(0, "block parameter count overflows"))?,
            )
            .ok_or_else(|| malformed(0, "function value count overflows"))?;
        for instruction in &block.instructions {
            value_count = value_count
                .checked_add(u64::from(instruction.result_count))
                .ok_or_else(|| malformed(0, "function value count overflows"))?;
        }
    }
    if value_count >= 1 << 32 || value_count != body.header.value_count {
        return Err(malformed(0, "function value count does not match its body"));
    }

    let mut output = Vec::new();
    write_index(body.header.function_index, &mut output)?;
    write_index(block_count, &mut output)?;
    write_index(value_count, &mut output)?;
    for block in &body.blocks {
        if block.instructions.len() >= 1 << 24
            || block.loop_merge.is_some_and(|target| target >= block_count)
            || block
                .loop_continue
                .is_some_and(|target| target >= block_count)
        {
            return Err(malformed(
                output.len(),
                "block instruction count or loop target exceeds the TVISA limit",
            ));
        }
        write_index(
            u64::try_from(block.parameter_types.len())
                .map_err(|_| malformed(output.len(), "block parameter count overflows"))?,
            &mut output,
        )?;
        for type_index in &block.parameter_types {
            write_index(*type_index, &mut output)?;
        }
        let mut flags = u8::from(block.cold) << 1;
        match (block.loop_merge, block.loop_continue) {
            (Some(merge), Some(continuation)) => {
                flags |= 1;
                output.push(flags);
                write_index(merge, &mut output)?;
                write_index(continuation, &mut output)?;
            }
            (None, None) => output.push(flags),
            _ => return Err(malformed(output.len(), "incomplete loop-header targets")),
        }
        if block.instructions.is_empty() {
            return Err(malformed(output.len(), "block has no terminator"));
        }
        write_index(
            u64::try_from(block.instructions.len())
                .map_err(|_| malformed(output.len(), "instruction count overflows"))?,
            &mut output,
        )?;
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            let is_last = instruction_index + 1 == block.instructions.len();
            if instruction.terminator != is_last
                || instruction.normal_edge_result_count > instruction.result_count
            {
                return Err(malformed(
                    output.len(),
                    "terminator placement does not match the decoded body",
                ));
            }
            if instruction.offset != output.len() as u64 {
                return Err(malformed(
                    output.len(),
                    "retained instruction offset does not match encoded body framing",
                ));
            }
            validate_instruction_prefix(instruction, output.len())?;
            output.extend_from_slice(&instruction.wire_bytes);
        }
    }
    Ok(output)
}

fn validate_instruction_prefix(
    instruction: &BinaryInstruction,
    offset: usize,
) -> Result<(), FormatError> {
    let mut cursor = 0;
    let opcode = read_uleb(&instruction.wire_bytes, &mut cursor)
        .map_err(|error| malformed(offset, format!("invalid retained opcode: {error:?}")))?;
    let modifiers = read_uleb(&instruction.wire_bytes, &mut cursor)
        .map_err(|error| malformed(offset, format!("invalid retained modifiers: {error:?}")))?;
    if opcode != instruction.opcode || modifiers != instruction.modifiers {
        return Err(malformed(
            offset,
            "retained instruction bytes disagree with decoded opcode or modifiers",
        ));
    }
    Ok(())
}

fn write_index(value: u64, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, output).map_err(|error| malformed(output.len(), error.to_string()))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
