use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryAddressSpace, BinaryCodeTable, BinaryGlobalTable, BinaryGpuTable, BinaryImport,
    BinaryImportEntityKind, BinaryOperand, BinarySignature, BinarySignatureTable, BinaryTarget,
    BinaryTypeTable,
};
use tir_layout::LayoutEngine;

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);
const MAX_OBJECT_SIZE: u64 = 1 << 62;

pub(super) fn verify(
    code: &BinaryCodeTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
    gpu: Option<&BinaryGpuTable>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut layouts = crate::binary_constants::binary_layout_engine(types).ok();
    let imported_data_count = imports.map(imported_data_count);
    for body in &code.functions {
        let Some(signature) = usize::try_from(body.function_index)
            .ok()
            .and_then(|index| signatures.records.get(index))
        else {
            continue;
        };
        if !matches!(signature.target, BinaryTarget::Gpu | BinaryTarget::Both) {
            continue;
        }
        let static_integers = static_integers(body);
        let mut threadgroup_size = 0;
        let mut threadgroup_alignment = 1;
        let mut threadgroup_layout_valid = true;
        for (block_index, block) in body.blocks.iter().enumerate() {
            let Some(instructions) = &block.instructions else {
                continue;
            };
            for instruction in instructions {
                match instruction.opcode {
                    0x00b => verify_global_address(
                        instruction,
                        signature,
                        imported_data_count,
                        imports,
                        globals,
                        diagnostics,
                    ),
                    0x00c => report(
                        diagnostics,
                        signature,
                        instruction.offset,
                        "GPU code cannot access thread-local globals with tls.addr",
                    ),
                    0x120 => {
                        let selected_space = stack_allocation_space(instruction, signature.target);
                        let count = allocation_count(instruction, &static_integers);
                        match count {
                            AllocationCount::Dynamic => report(
                                diagnostics,
                                signature,
                                instruction.offset,
                                "GPU stack.alloc sizes must be static",
                            ),
                            AllocationCount::InvalidStatic => report(
                                diagnostics,
                                signature,
                                instruction.offset,
                                "GPU stack.alloc count must be below 2^62",
                            ),
                            AllocationCount::Static(_) => {}
                        }
                        if selected_space != Some(3) {
                            continue;
                        }
                        if signature.entry_kind.is_none() || block_index != 0 {
                            report(
                                diagnostics,
                                signature,
                                instruction.offset,
                                "threadgroup storage must be allocated in an entry's entry block",
                            );
                            threadgroup_layout_valid = false;
                            continue;
                        }
                        let Some(count) = count.value() else {
                            threadgroup_layout_valid = false;
                            continue;
                        };
                        if count >= MAX_OBJECT_SIZE {
                            report(
                                diagnostics,
                                signature,
                                instruction.offset,
                                "GPU stack.alloc count exceeds the maximum object size",
                            );
                            threadgroup_layout_valid = false;
                            continue;
                        }
                        let Some(type_index) = explicit_type(instruction) else {
                            report(
                                diagnostics,
                                signature,
                                instruction.offset,
                                "threadgroup stack.alloc has no valid allocation type",
                            );
                            threadgroup_layout_valid = false;
                            continue;
                        };
                        let layout = layout_for_type(types, &mut layouts, type_index);
                        let Ok(layout) = layout else {
                            report(
                                diagnostics,
                                signature,
                                instruction.offset,
                                "threadgroup stack.alloc type has no static layout",
                            );
                            threadgroup_layout_valid = false;
                            continue;
                        };
                        let Some(bytes) = layout.size.checked_mul(count) else {
                            report(
                                diagnostics,
                                signature,
                                instruction.offset,
                                "threadgroup stack.alloc size overflows",
                            );
                            threadgroup_layout_valid = false;
                            continue;
                        };
                        if bytes >= MAX_OBJECT_SIZE {
                            report(
                                diagnostics,
                                signature,
                                instruction.offset,
                                "threadgroup stack.alloc exceeds the maximum object size",
                            );
                            threadgroup_layout_valid = false;
                            continue;
                        }
                        let Some(aligned) = align_up(threadgroup_size, layout.align) else {
                            report(
                                diagnostics,
                                signature,
                                instruction.offset,
                                "threadgroup storage alignment overflows",
                            );
                            threadgroup_layout_valid = false;
                            continue;
                        };
                        let Some(size) = aligned.checked_add(bytes) else {
                            report(
                                diagnostics,
                                signature,
                                instruction.offset,
                                "threadgroup storage size overflows",
                            );
                            threadgroup_layout_valid = false;
                            continue;
                        };
                        if size >= MAX_OBJECT_SIZE {
                            report(
                                diagnostics,
                                signature,
                                instruction.offset,
                                "threadgroup storage exceeds the maximum object size",
                            );
                            threadgroup_layout_valid = false;
                            continue;
                        }
                        threadgroup_size = size;
                        threadgroup_alignment = threadgroup_alignment.max(layout.align);
                    }
                    _ => {}
                }
            }
        }
        if !threadgroup_layout_valid || signature.entry_kind.is_none() {
            continue;
        }
        let Some(entry) = gpu.and_then(|table| {
            table
                .entries
                .iter()
                .find(|entry| entry.function_index == body.function_index)
        }) else {
            continue;
        };
        let Some(required) = align_up(threadgroup_size, threadgroup_alignment) else {
            report(
                diagnostics,
                signature,
                0,
                "threadgroup storage size overflows",
            );
            continue;
        };
        if required != entry.threadgroup_memory {
            diagnostics.push(Diagnostic::error(
                V15,
                format!(
                    "GPU entry @{} declares {} bytes of threadgroup memory, but its stack.alloc instructions require {required} bytes",
                    signature.name, entry.threadgroup_memory
                ),
            ));
        }
    }
}

fn verify_global_address(
    instruction: &tir_format::BinaryInstruction,
    signature: &BinarySignature,
    imported_data_count: Option<u64>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(index) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 1, index } => Some(*index),
            _ => None,
        })
    else {
        return;
    };
    let Some(imported_data_count) = imported_data_count else {
        report(
            diagnostics,
            signature,
            instruction.offset,
            "GPU global.addr cannot be checked because IMPORTS metadata is unavailable",
        );
        return;
    };
    if imports.is_none() || index < imported_data_count {
        report(
            diagnostics,
            signature,
            instruction.offset,
            "GPU code may access only initialized constant-space globals",
        );
        return;
    }
    let Some(global) = globals.and_then(|table| {
        index
            .checked_sub(imported_data_count)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| table.records.get(index))
    }) else {
        report(
            diagnostics,
            signature,
            instruction.offset,
            "GPU global.addr references an unknown data symbol",
        );
        return;
    };
    if global.address_space != BinaryAddressSpace::Constant
        || global.initializer.is_none()
        || global.thread_local
    {
        report(
            diagnostics,
            signature,
            instruction.offset,
            "GPU code may access only initialized constant-space globals",
        );
    }
}

fn imported_data_count(imports: &[BinaryImport]) -> u64 {
    imports
        .iter()
        .flat_map(|import| &import.entities)
        .filter(|entity| {
            matches!(
                entity.kind,
                BinaryImportEntityKind::Global | BinaryImportEntityKind::PlatformData
            )
        })
        .count() as u64
}

fn static_integers(body: &tir_format::BinaryFunctionBody) -> HashMap<u64, Option<u64>> {
    let mut values = HashMap::new();
    let mut next_value = 0u64;
    for block in &body.blocks {
        next_value = next_value.saturating_add(block.parameters.len() as u64);
        if let Some(instructions) = &block.instructions {
            for instruction in instructions {
                if instruction.opcode == 0x000 && instruction.result_count == 1 {
                    values.insert(next_value, integer_constant(instruction));
                }
                next_value = next_value.saturating_add(instruction.result_count);
            }
        }
    }
    values
}

fn integer_constant(instruction: &tir_format::BinaryInstruction) -> Option<u64> {
    let bytes = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Bytes(bytes) => Some(bytes),
            _ => None,
        })?;
    if bytes.iter().skip(8).any(|byte| *byte != 0) {
        return None;
    }
    Some(
        bytes
            .iter()
            .take(8)
            .enumerate()
            .fold(0u64, |value, (shift, byte)| {
                value | (u64::from(*byte) << (shift * 8))
            }),
    )
}

#[derive(Clone, Copy)]
enum AllocationCount {
    Static(u64),
    Dynamic,
    InvalidStatic,
}

impl AllocationCount {
    fn value(self) -> Option<u64> {
        match self {
            Self::Static(value) => Some(value),
            Self::Dynamic | Self::InvalidStatic => None,
        }
    }
}

fn allocation_count(
    instruction: &tir_format::BinaryInstruction,
    static_integers: &HashMap<u64, Option<u64>>,
) -> AllocationCount {
    let Some(value) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::OptionalValue(value) => *value,
            _ => None,
        })
    else {
        return AllocationCount::Static(1);
    };
    match static_integers.get(&value) {
        None => AllocationCount::Dynamic,
        Some(Some(value)) if *value < MAX_OBJECT_SIZE => AllocationCount::Static(*value),
        Some(_) => AllocationCount::InvalidStatic,
    }
}

fn stack_allocation_space(
    instruction: &tir_format::BinaryInstruction,
    target: BinaryTarget,
) -> Option<u64> {
    let selector = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })?;
    match selector {
        0 => Some(if target == BinaryTarget::Gpu { 4 } else { 0 }),
        1..=10 => Some(if selector == 10 { 0 } else { selector }),
        _ => None,
    }
}

fn explicit_type(instruction: &tir_format::BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })
}

fn layout_for_type(
    types: &BinaryTypeTable,
    layouts: &mut Option<LayoutEngine>,
    type_index: u64,
) -> Result<tir_layout::Layout, String> {
    let engine = layouts
        .as_mut()
        .ok_or_else(|| "binary type layouts are invalid".to_owned())?;
    let ty = crate::binary_constants::binary_layout_type(types, type_index)?;
    engine.layout(&ty).map_err(|error| error.to_string())
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    if alignment == 0 {
        return None;
    }
    let remainder = value % alignment;
    if remainder == 0 {
        Some(value)
    } else {
        value.checked_add(alignment - remainder)
    }
}

fn report(
    diagnostics: &mut Vec<Diagnostic>,
    signature: &BinarySignature,
    offset: u64,
    message: &str,
) {
    diagnostics.push(Diagnostic::error(
        V15,
        format!(
            "GPU function @{} CODE offset {offset}: {message}",
            signature.name
        ),
    ));
}
