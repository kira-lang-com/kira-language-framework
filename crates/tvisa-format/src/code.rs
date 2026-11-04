mod encode;
mod instructions;
pub use encode::encode_function_bodies;

use crate::binary::{FormatError, Section, TvisaImage};
use crate::{FunctionBodyHeader, ImportCounts, TypeSection, VariantIndex};
use tir_format::leb::{IntegerImmediate, read_uleb};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryFunctionBody {
    pub header: FunctionBodyHeader,
    pub blocks: Vec<BinaryBlock>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryBlock {
    pub parameter_types: Vec<u64>,
    pub loop_merge: Option<u64>,
    pub loop_continue: Option<u64>,
    pub cold: bool,
    pub instructions: Vec<BinaryInstruction>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryInstruction {
    pub opcode: u64,
    pub modifiers: u64,
    pub offset: u64,
    pub result_count: u8,
    pub normal_edge_result_count: u8,
    pub terminator: bool,
    pub operands: Vec<BinaryOperand>,
    /// Original canonical instruction bytes, including opcode and modifiers.
    pub wire_bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BinaryOperand {
    Value(u64),
    Type(u64),
    Index(u64),
    Integer(IntegerImmediate),
    IntegerBits(u128),
    TypeTag(u32),
    Symbol(u64),
    Bytes(Vec<u8>),
    Values(Vec<u64>),
    Integers(Vec<IntegerImmediate>),
    Block(u64),
    BlockTarget { block: u64, arguments: Vec<u64> },
    Blocks(Vec<u64>),
    BlockTargets(Vec<(u64, Vec<u64>)>),
    SwitchCases(Vec<(IntegerImmediate, u64, Vec<u64>)>),
    Clauses(Vec<(u8, u64)>),
}

impl TvisaImage {
    pub fn function_bodies(
        &self,
        memory_limit: u64,
    ) -> Result<Vec<BinaryFunctionBody>, FormatError> {
        let headers = self.function_body_headers(memory_limit)?;
        if headers.is_empty() {
            return Ok(Vec::new());
        }
        let type_section = self.type_section(memory_limit)?;
        let functions = self.function_records(memory_limit)?;
        let imports = self.import_counts(memory_limit)?;
        let variants = self.variant_index(memory_limit)?;
        let string_count = self.string_table(memory_limit)?.len() as u64;
        let assembly_count = self.assembly_records(memory_limit)?.len() as u64;
        let calls = CallSignatures::new(&functions, &imports, &variants);
        let code = section(&self.sections, 9, "CODE")?.read_uncompressed(memory_limit)?;
        if functions.len() != headers.len() {
            return Err(malformed(
                20,
                "defined function records do not match FUNCINDEX count",
            ));
        }
        let mut bodies = Vec::new();
        bodies
            .try_reserve_exact(headers.len())
            .map_err(|_| malformed(20, "function body allocation failed"))?;
        for (header, function) in headers.into_iter().zip(functions) {
            let start = usize::try_from(header.offset)
                .map_err(|_| malformed(0, "function body offset does not fit in memory"))?;
            let end = start
                .checked_add(header.size as usize)
                .filter(|end| *end <= code.len())
                .ok_or_else(|| malformed(start, "function body extends beyond CODE"))?;
            bodies.push(decode_body(
                &code[start..end],
                start,
                header,
                &type_section,
                &calls,
                function.target != 0,
                string_count,
                assembly_count,
            )?);
        }
        validate_assembly_references(&bodies, assembly_count)?;
        Ok(bodies)
    }
}

fn validate_assembly_references(
    bodies: &[BinaryFunctionBody],
    assembly_count: u64,
) -> Result<(), FormatError> {
    let mut next_record = 0u64;
    for body in bodies {
        for instruction in body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|instruction| matches!(instruction.opcode, 0x220 | 0x221))
        {
            let instruction_offset = body
                .header
                .offset
                .checked_add(instruction.offset)
                .and_then(|offset| usize::try_from(offset).ok())
                .ok_or_else(|| {
                    FormatError::malformed(None, "assembly instruction offset overflows")
                })?;
            let record = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Index(index) => Some(*index),
                    _ => None,
                });
            let record = record.ok_or_else(|| {
                malformed(
                    instruction_offset,
                    "assembly instruction has no ASM record index",
                )
            })?;
            if record > next_record {
                return Err(malformed(
                    instruction_offset,
                    "ASM records are not ordered by first CODE reference",
                ));
            }
            if record == next_record {
                next_record = next_record
                    .checked_add(1)
                    .ok_or_else(|| malformed(instruction_offset, "ASM index overflows"))?;
            }
        }
    }
    if next_record != assembly_count {
        return Err(FormatError::malformed(
            None,
            "ASM section contains an unreferenced record",
        ));
    }
    Ok(())
}

fn decode_body(
    bytes: &[u8],
    base: usize,
    header: FunctionBodyHeader,
    types: &TypeSection,
    calls: &CallSignatures,
    gpu: bool,
    string_count: u64,
    assembly_count: u64,
) -> Result<BinaryFunctionBody, FormatError> {
    let mut cursor = 0;
    let function_index = read_index(bytes, &mut cursor, base, "body function index")?;
    let block_count = read_index(bytes, &mut cursor, base, "body block count")?;
    let value_count = read_index(bytes, &mut cursor, base, "body value count")?;
    if function_index != header.function_index
        || block_count != header.block_count
        || value_count != header.value_count
    {
        return Err(malformed(base, "CODE body header changed during decoding"));
    }
    let minimum_block_size = block_count
        .checked_mul(3)
        .ok_or_else(|| malformed(base + cursor, "block count overflows"))?;
    if minimum_block_size > bytes.len().saturating_sub(cursor) as u64 {
        return Err(malformed(
            base + cursor,
            "block count exceeds the function body",
        ));
    }
    let capacity = usize::try_from(block_count)
        .map_err(|_| malformed(base + cursor, "block count does not fit in memory"))?;
    let mut blocks = Vec::new();
    blocks
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(base + cursor, "block allocation failed"))?;
    let mut value_number = 0u64;
    let type_count = types.value_types.len() as u64;
    for block_index in 0..block_count {
        let parameter_count = read_index(bytes, &mut cursor, base, "block parameter count")?;
        if parameter_count > bytes.len().saturating_sub(cursor) as u64 {
            return Err(malformed(
                base + cursor,
                "block parameter count exceeds the function body",
            ));
        }
        let parameter_capacity = usize::try_from(parameter_count).map_err(|_| {
            malformed(
                base + cursor,
                "block parameter count does not fit in memory",
            )
        })?;
        let mut parameter_types = Vec::new();
        parameter_types
            .try_reserve_exact(parameter_capacity)
            .map_err(|_| malformed(base + cursor, "block parameter allocation failed"))?;
        for _ in 0..parameter_count {
            let type_index = read_index(bytes, &mut cursor, base, "block parameter type")?;
            if type_index >= type_count {
                return Err(malformed(
                    base + cursor,
                    "block parameter type index is invalid",
                ));
            }
            parameter_types.push(type_index);
        }
        value_number = value_number
            .checked_add(parameter_count)
            .ok_or_else(|| malformed(base + cursor, "function value count overflows"))?;

        let flags = read_byte(bytes, &mut cursor, base, "block flags")?;
        if flags & !3 != 0 {
            return Err(malformed(base + cursor - 1, "reserved block flags are set"));
        }
        let (loop_merge, loop_continue) = if flags & 1 != 0 {
            let merge = read_index(bytes, &mut cursor, base, "loop merge block")?;
            let continuation = read_index(bytes, &mut cursor, base, "loop continue block")?;
            if merge >= block_count || continuation >= block_count {
                return Err(malformed(
                    base + cursor,
                    "loop target block index is invalid",
                ));
            }
            (Some(merge), Some(continuation))
        } else {
            (None, None)
        };
        let instruction_count = read_index(bytes, &mut cursor, base, "block instruction count")?;
        if instruction_count == 0 {
            return Err(malformed(
                base + cursor,
                "block does not contain a terminator",
            ));
        }
        if instruction_count > bytes.len().saturating_sub(cursor) as u64 / 2 {
            return Err(malformed(
                base + cursor,
                "instruction count exceeds the function body",
            ));
        }
        let instruction_capacity = usize::try_from(instruction_count)
            .map_err(|_| malformed(base + cursor, "instruction count does not fit in memory"))?;
        let mut instructions = Vec::new();
        instructions
            .try_reserve_exact(instruction_capacity)
            .map_err(|_| malformed(base + cursor, "instruction allocation failed"))?;
        for instruction_index in 0..instruction_count {
            let instruction = instructions::decode_instruction(
                bytes,
                &mut cursor,
                base,
                value_number,
                block_count,
                types,
                calls,
                gpu,
                string_count,
                assembly_count,
            )?;
            if instruction.terminator && instruction_index + 1 != instruction_count {
                return Err(malformed(
                    base + instruction.offset as usize,
                    "terminator is not the last instruction in its block",
                ));
            }
            if instruction_index + 1 == instruction_count && !instruction.terminator {
                return Err(malformed(
                    base + instruction.offset as usize,
                    "block does not end in a terminator",
                ));
            }
            value_number = value_number
                .checked_add(u64::from(instruction.result_count))
                .ok_or_else(|| malformed(base + cursor, "function value count overflows"))?;
            instructions.push(instruction);
        }
        blocks.push(BinaryBlock {
            parameter_types,
            loop_merge,
            loop_continue,
            cold: flags & 2 != 0,
            instructions,
        });
        if block_index + 1 == block_count && cursor != bytes.len() {
            return Err(malformed(base + cursor, "trailing bytes in CODE body"));
        }
    }
    if value_number != value_count {
        return Err(malformed(
            base + cursor,
            "CODE value count does not match the decoded body",
        ));
    }
    validate_targets(&blocks, block_count, base)?;
    Ok(BinaryFunctionBody { header, blocks })
}

struct CallSignatures {
    function_space_signatures: Vec<u64>,
    variant_signatures: Vec<Option<u64>>,
}

impl CallSignatures {
    fn new(
        functions: &[crate::FunctionRecordSummary],
        imports: &ImportCounts,
        variants: &VariantIndex,
    ) -> Self {
        let mut function_space = imports.function_signatures.clone();
        function_space.extend(functions.iter().map(|function| function.signature_index));
        let variant_signatures = variants
            .sets
            .iter()
            .map(|set| {
                set.members
                    .first()
                    .and_then(|member| function_space.get(member.function_index as usize))
                    .copied()
            })
            .collect();
        Self {
            function_space_signatures: function_space,
            variant_signatures,
        }
    }

    fn symbol_signature(&self, symbol: u64) -> Option<u64> {
        let index = symbol >> 2;
        match symbol & 3 {
            0 => self.function_space_signatures.get(index as usize).copied(),
            2 => self
                .variant_signatures
                .get(index as usize)
                .copied()
                .flatten(),
            _ => None,
        }
    }
}

fn validate_targets(
    blocks: &[BinaryBlock],
    block_count: u64,
    base: usize,
) -> Result<(), FormatError> {
    for block in blocks {
        for instruction in &block.instructions {
            for operand in &instruction.operands {
                match operand {
                    BinaryOperand::Block(target) if *target >= block_count => {
                        return Err(malformed(
                            base + instruction.offset as usize,
                            "block target index is invalid",
                        ));
                    }
                    BinaryOperand::BlockTarget { block: target, .. } if *target >= block_count => {
                        return Err(malformed(
                            base + instruction.offset as usize,
                            "block target index is invalid",
                        ));
                    }
                    BinaryOperand::Blocks(targets)
                        if targets.iter().any(|target| *target >= block_count) =>
                    {
                        return Err(malformed(
                            base + instruction.offset as usize,
                            "block target index is invalid",
                        ));
                    }
                    BinaryOperand::BlockTargets(targets)
                        if targets.iter().any(|(target, _)| *target >= block_count) =>
                    {
                        return Err(malformed(
                            base + instruction.offset as usize,
                            "block target index is invalid",
                        ));
                    }
                    BinaryOperand::SwitchCases(cases)
                        if cases.iter().any(|(_, target, _)| *target >= block_count) =>
                    {
                        return Err(malformed(
                            base + instruction.offset as usize,
                            "block target index is invalid",
                        ));
                    }
                    _ => {}
                }
            }
            if instruction.opcode == 0x221
                && instruction.operands.iter().any(|operand| {
                    let BinaryOperand::Blocks(targets) = operand else {
                        return false;
                    };
                    targets.iter().any(|target| {
                        usize::try_from(*target)
                            .ok()
                            .and_then(|index| blocks.get(index))
                            .is_some_and(|target| !target.parameter_types.is_empty())
                    })
                })
            {
                return Err(malformed(
                    base + instruction.offset as usize,
                    "asm.goto destination blocks cannot have parameters",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn read_index(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    field: &str,
) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(base + offset, format!("invalid {field} ULEB128: {error:?}")))
}

pub(super) fn read_byte(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    field: &str,
) -> Result<u8, FormatError> {
    let byte = bytes
        .get(*cursor)
        .copied()
        .ok_or_else(|| malformed(base + *cursor, format!("truncated {field}")))?;
    *cursor += 1;
    Ok(byte)
}

fn section<'a>(sections: &'a [Section], kind: u32, name: &str) -> Result<&'a Section, FormatError> {
    sections
        .iter()
        .find(|section| section.kind == kind)
        .ok_or_else(|| malformed(0, format!("{name} is missing")))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
