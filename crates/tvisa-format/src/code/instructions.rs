mod constants;
mod extra;
mod gpu_advanced;
mod gpu_collectives;
mod gpu_control;
mod gpu_geometry;
mod gpu_ray;
mod gpu_texture;
mod helpers;

use crate::TypeSection;
use crate::binary::FormatError;

use self::helpers::*;
use super::{BinaryInstruction, BinaryOperand, CallSignatures, read_byte, read_index};

pub(super) fn decode_instruction(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    block_count: u64,
    types: &TypeSection,
    calls: &CallSignatures,
    gpu: bool,
    string_count: u64,
    assembly_count: u64,
) -> Result<BinaryInstruction, FormatError> {
    let start = *cursor;
    let opcode = read_index(bytes, cursor, base, "instruction opcode")?;
    let modifiers = read_index(bytes, cursor, base, "instruction modifiers")?;
    if modifiers >> 52 != 0 {
        return Err(malformed(
            base + start,
            "reserved instruction modifier bits are set",
        ));
    }
    let mut operands = Vec::new();
    let mut normal_edge_result_count = 0;
    let (result_count, terminator) = match opcode {
        0x020..=0x045 => {
            let relative = opcode - 0x020;
            let value_count = match relative {
                14 | 15 | 16 | 30 | 31 | 32 | 33 | 34 | 37 => 1,
                22 | 23 | 29 => 3,
                _ => 2,
            };
            read_values(
                bytes,
                cursor,
                base,
                current_value,
                value_count,
                &mut operands,
            )?;
            (if (3..=5).contains(&relative) { 2 } else { 1 }, false)
        }
        0x060..=0x092 => {
            let relative = opcode - 0x060;
            let value_count = match relative {
                5 | 6 | 7 | 15..=19 | 21 | 22 | 26..=31 | 33..=46 | 48 | 50 => 1,
                8 | 9 => 3,
                24 | 25 => 0,
                _ => 2,
            };
            read_values(
                bytes,
                cursor,
                base,
                current_value,
                value_count,
                &mut operands,
            )?;
            let result_count = match relative {
                22 => 2,
                25 => 0,
                _ => 1,
            };
            (result_count, false)
        }
        0x0a0..=0x0ac => {
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            (1, false)
        }
        0x0c0..=0x0d7 => {
            let relative = opcode - 0x0c0;
            match relative {
                0 => {
                    read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
                    operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
                }
                1 => read_values(bytes, cursor, base, current_value, 2, &mut operands)?,
                2 => read_values(bytes, cursor, base, current_value, 3, &mut operands)?,
                3 => {
                    read_values(bytes, cursor, base, current_value, 2, &mut operands)?;
                    operands.push(BinaryOperand::Integers(read_integer_list(
                        bytes,
                        cursor,
                        base,
                        "vector shuffle index",
                    )?));
                }
                4 => operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?)),
                5 => {}
                6 => read_values(
                    bytes,
                    cursor,
                    base,
                    current_value,
                    1 + u64::from(modifiers & (1 << 32) != 0),
                    &mut operands,
                )?,
                7 => {
                    operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
                    read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
                    operands.push(BinaryOperand::Index(read_index(
                        bytes,
                        cursor,
                        base,
                        "vector load alignment",
                    )?));
                    if modifiers & (1 << 32) != 0 {
                        read_values(bytes, cursor, base, current_value, 2, &mut operands)?;
                    }
                }
                8 => {
                    read_values(bytes, cursor, base, current_value, 2, &mut operands)?;
                    operands.push(BinaryOperand::Index(read_index(
                        bytes,
                        cursor,
                        base,
                        "vector store alignment",
                    )?));
                    if modifiers & (1 << 32) != 0 {
                        read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
                    }
                }
                9 => {
                    operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
                    match (modifiers >> 48) & 0xf {
                        0 => read_values(bytes, cursor, base, current_value, 1, &mut operands)?,
                        1 => {
                            read_values(bytes, cursor, base, current_value, 2, &mut operands)?;
                            operands.push(BinaryOperand::Index(read_index(
                                bytes,
                                cursor,
                                base,
                                "vector gather scale",
                            )?));
                        }
                        _ => return Err(malformed(base + start, "invalid vector gather form")),
                    }
                    if modifiers & (1 << 32) != 0 {
                        read_values(bytes, cursor, base, current_value, 2, &mut operands)?;
                    }
                }
                10 => {
                    read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
                    match (modifiers >> 48) & 0xf {
                        0 => read_values(bytes, cursor, base, current_value, 1, &mut operands)?,
                        1 => {
                            read_values(bytes, cursor, base, current_value, 2, &mut operands)?;
                            operands.push(BinaryOperand::Index(read_index(
                                bytes,
                                cursor,
                                base,
                                "vector scatter scale",
                            )?));
                        }
                        _ => return Err(malformed(base + start, "invalid vector scatter form")),
                    }
                    if modifiers & (1 << 32) != 0 {
                        read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
                    }
                }
                11 | 12 | 13 | 17 => {
                    read_values(bytes, cursor, base, current_value, 3, &mut operands)?;
                }
                14 | 15 | 18 => {
                    read_values(bytes, cursor, base, current_value, 2, &mut operands)?;
                }
                16 | 19 | 21 | 22 => {
                    read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
                }
                20 => read_values(bytes, cursor, base, current_value, 2, &mut operands)?,
                23 => read_values(bytes, cursor, base, current_value, 2, &mut operands)?,
                _ => return Err(malformed(base + start, "invalid vector instruction opcode")),
            }
            (if matches!(relative, 8 | 10) { 0 } else { 1 }, false)
        }
        0x146 => {
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "raw load alignment",
            )?));
            (1, false)
        }
        0x147 => {
            read_values(bytes, cursor, base, current_value, 2, &mut operands)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "raw store alignment",
            )?));
            (0, false)
        }
        0x148 | 0x149 => {
            read_values(bytes, cursor, base, current_value, 2, &mut operands)?;
            (1, false)
        }
        0x14a..=0x14c => {
            read_values(bytes, cursor, base, current_value, 3, &mut operands)?;
            (0, false)
        }
        0x14d => {
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            (0, false)
        }
        0x1a0 => {
            let symbol = read_index(bytes, cursor, base, "direct call symbol")?;
            operands.push(BinaryOperand::Symbol(symbol));
            let signature = calls.symbol_signature(symbol).ok_or_else(|| {
                malformed(base + start, "direct call symbol has no function signature")
            })?;
            read_call_arguments(bytes, cursor, base, current_value, &mut operands)?;
            (signature_results(types, signature, base + start)?, false)
        }
        0x1a1 => {
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            let signature = read_signature(bytes, cursor, base, types)?;
            operands.push(BinaryOperand::Index(signature));
            operands.push(BinaryOperand::TypeTag(read_u32(bytes, cursor, base)?));
            read_call_arguments(bytes, cursor, base, current_value, &mut operands)?;
            (signature_results(types, signature, base + start)?, false)
        }
        0x1a9 => {
            let symbol = read_index(bytes, cursor, base, "variadic call symbol")?;
            operands.push(BinaryOperand::Symbol(symbol));
            let signature = read_signature(bytes, cursor, base, types)?;
            operands.push(BinaryOperand::Index(signature));
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "variadic fixed parameter count",
            )?));
            read_call_arguments(bytes, cursor, base, current_value, &mut operands)?;
            (signature_results(types, signature, base + start)?, false)
        }
        0x1aa => {
            let selector = (modifiers >> 48) & 0xf;
            let signature = decode_selected_callee(
                selector,
                bytes,
                cursor,
                base,
                start,
                current_value,
                types,
                calls,
                &mut operands,
            )?;
            normal_edge_result_count = match signature {
                Some(signature) => signature_results(types, signature, base + start)?,
                None => {
                    let Some(BinaryOperand::Index(id)) = operands.first() else {
                        return Err(malformed(base + start, "runtime callee has no entry ID"));
                    };
                    runtime_results(*id).ok_or_else(|| {
                        malformed(base + start, "runtime callee has no result signature")
                    })?
                }
            };
            read_call_arguments(bytes, cursor, base, current_value, &mut operands)?;
            operands.push(read_block_target(
                bytes,
                cursor,
                base,
                current_value,
                block_count,
            )?);
            let unwind = read_block_target(bytes, cursor, base, current_value, block_count)?;
            if matches!(&unwind, BinaryOperand::BlockTarget { arguments, .. } if !arguments.is_empty())
            {
                return Err(malformed(
                    base + start,
                    "invoke unwind target cannot have arguments",
                ));
            }
            operands.push(unwind);
            (0, true)
        }
        0x1e0 => {
            operands.push(read_block_target(
                bytes,
                cursor,
                base,
                current_value,
                block_count,
            )?);
            (0, true)
        }
        0x1e1 => {
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
            if gpu {
                operands.push(read_block_target(
                    bytes,
                    cursor,
                    base,
                    current_value,
                    block_count,
                )?);
            }
            (0, true)
        }
        0x1e2 => {
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            let case_count = read_index(bytes, cursor, base, "switch case count")?;
            if case_count > bytes.len().saturating_sub(*cursor) as u64 / 3 {
                return Err(malformed(
                    base + *cursor,
                    "switch case count exceeds the function body",
                ));
            }
            let mut cases = Vec::new();
            let capacity = usize::try_from(case_count).map_err(|_| {
                malformed(base + *cursor, "switch case count does not fit in memory")
            })?;
            cases
                .try_reserve_exact(capacity)
                .map_err(|_| malformed(base + *cursor, "switch case allocation failed"))?;
            for _ in 0..case_count {
                let value = read_integer(bytes, cursor, base, "switch case value")?;
                let target = read_block_target(bytes, cursor, base, current_value, block_count)?;
                let (block, arguments) = into_target(target);
                cases.push((value, block, arguments));
            }
            let default = read_block_target(bytes, cursor, base, current_value, block_count)?;
            let (default_block, default_args) = into_target(default);
            let merge = if gpu {
                Some(read_block_target(
                    bytes,
                    cursor,
                    base,
                    current_value,
                    block_count,
                )?)
            } else {
                None
            };
            operands.push(BinaryOperand::SwitchCases(cases));
            operands.push(BinaryOperand::BlockTarget {
                block: default_block,
                arguments: default_args,
            });
            if let Some(merge) = merge {
                operands.push(merge);
            }
            (0, true)
        }
        0x1e4 => {
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            let count = read_index(bytes, cursor, base, "indirect branch target count")?;
            let mut targets = Vec::new();
            let capacity = usize::try_from(count).map_err(|_| {
                malformed(
                    base + *cursor,
                    "indirect branch target count does not fit in memory",
                )
            })?;
            targets.try_reserve_exact(capacity).map_err(|_| {
                malformed(base + *cursor, "indirect branch target allocation failed")
            })?;
            for _ in 0..count {
                let (target, arguments) = into_target(read_block_target(
                    bytes,
                    cursor,
                    base,
                    current_value,
                    block_count,
                )?);
                if !arguments.is_empty() {
                    return Err(malformed(
                        base + *cursor,
                        "br.indirect targets cannot have arguments",
                    ));
                }
                targets.push(target);
            }
            operands.push(BinaryOperand::Blocks(targets));
            (0, true)
        }
        0x1e5 => {
            let count = read_index(bytes, cursor, base, "return value count")?;
            read_values(bytes, cursor, base, current_value, count, &mut operands)?;
            (0, true)
        }
        0x1e6 => {
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            (0, true)
        }
        0x1e7 => {
            let selector = (modifiers >> 48) & 0xf;
            decode_selected_callee(
                selector,
                bytes,
                cursor,
                base,
                start,
                current_value,
                types,
                calls,
                &mut operands,
            )?;
            read_call_arguments(bytes, cursor, base, current_value, &mut operands)?;
            (0, true)
        }
        0x1e8 => (0, true),
        0x1e9 => {
            operands.push(BinaryOperand::Integer(read_integer(
                bytes,
                cursor,
                base,
                "trap code",
            )?));
            (0, true)
        }
        0x700..=0x702 => {
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "frame slot",
            )?));
            (u8::from(opcode == 0x700), false)
        }
        0x703 => {
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "dynamic allocation alignment",
            )?));
            (1, false)
        }
        0x704 => (1, false),
        0x705 => {
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            (0, false)
        }
        0x706 => {
            let id = read_index(bytes, cursor, base, "runtime entry ID")?;
            if id > u64::from(u16::MAX) {
                return Err(malformed(base + start, "runtime entry ID exceeds 16 bits"));
            }
            operands.push(BinaryOperand::Index(id));
            read_call_arguments(bytes, cursor, base, current_value, &mut operands)?;
            (
                runtime_results(id).ok_or_else(|| {
                    malformed(base + start, "runtime entry has no binary signature")
                })?,
                false,
            )
        }
        0x707 => {
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            operands.push(BinaryOperand::Integer(read_integer(
                bytes,
                cursor,
                base,
                "conditional trap code",
            )?));
            (0, false)
        }
        0x708 | 0x70b => {
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            operands.push(BinaryOperand::TypeTag(read_u32(bytes, cursor, base)?));
            (1, false)
        }
        0x709 => {
            read_values(bytes, cursor, base, current_value, 2, &mut operands)?;
            operands.push(BinaryOperand::TypeTag(read_u32(bytes, cursor, base)?));
            (0, false)
        }
        0x70a => {
            read_values(bytes, cursor, base, current_value, 1, &mut operands)?;
            (1, false)
        }
        0x70c..=0x70f => (1, false),
        0x1d0 => {
            let count = read_index(bytes, cursor, base, "landingpad clause count")?;
            if count > bytes.len().saturating_sub(*cursor) as u64 {
                return Err(malformed(
                    base + *cursor,
                    "landingpad clause count exceeds the function body",
                ));
            }
            let capacity = usize::try_from(count).map_err(|_| {
                malformed(
                    base + *cursor,
                    "landingpad clause count does not fit in memory",
                )
            })?;
            let mut clauses = Vec::new();
            clauses
                .try_reserve_exact(capacity)
                .map_err(|_| malformed(base + *cursor, "landingpad clause allocation failed"))?;
            for _ in 0..count {
                let kind = read_byte(bytes, cursor, base, "landingpad clause kind")?;
                if kind > 3 {
                    return Err(malformed(
                        base + *cursor - 1,
                        "invalid landingpad clause kind",
                    ));
                }
                let item = match kind {
                    0 | 1 => read_index(bytes, cursor, base, "landingpad clause reference")?,
                    _ => 0,
                };
                clauses.push((kind, item));
            }
            operands.push(BinaryOperand::Clauses(clauses));
            (2, false)
        }
        _ => {
            let decoded = gpu_control::decode(
                opcode,
                bytes,
                cursor,
                base,
                start,
                current_value,
                modifiers,
                types,
                string_count,
                &mut operands,
            )?
            .or(gpu_collectives::decode(
                opcode,
                bytes,
                cursor,
                base,
                start,
                current_value,
                modifiers,
                &mut operands,
            )?)
            .or(gpu_geometry::decode(
                opcode,
                bytes,
                cursor,
                base,
                start,
                current_value,
                modifiers,
                &mut operands,
            )?)
            .or(gpu_ray::decode(
                opcode,
                bytes,
                cursor,
                base,
                start,
                current_value,
                modifiers,
                &mut operands,
            )?)
            .or(gpu_texture::decode(
                opcode,
                bytes,
                cursor,
                base,
                start,
                current_value,
                modifiers,
                &mut operands,
            )?)
            .or(gpu_advanced::decode(
                opcode,
                bytes,
                cursor,
                base,
                start,
                current_value,
                modifiers,
                types,
                &mut operands,
            )?)
            .or(constants::decode(
                opcode,
                bytes,
                cursor,
                base,
                start,
                types,
                &mut operands,
            )?)
            .or(extra::decode(
                opcode,
                bytes,
                cursor,
                base,
                current_value,
                types,
                string_count,
                assembly_count,
                modifiers,
                &mut operands,
            )?);
            match decoded {
                Some(decoded) => decoded,
                None => {
                    return Err(malformed(
                        base + start,
                        format!("unsupported or invalid TVISA instruction opcode 0x{opcode:X}"),
                    ));
                }
            }
        }
    };
    Ok(BinaryInstruction {
        opcode,
        modifiers,
        offset: start as u64,
        result_count,
        normal_edge_result_count,
        terminator,
        operands,
    })
}
