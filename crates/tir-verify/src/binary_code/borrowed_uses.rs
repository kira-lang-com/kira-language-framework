use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryGenericTable, BinaryGlobalTable, BinaryImport, BinaryInstruction,
    BinaryOperand, BinarySignatureTable, BinaryType, BinaryTypeTable, StringTable,
};

use super::types::aggregate_instructions;

const V5: Code = Code::new(Dialect::Tir, Area::Verifier, 5);

pub(super) fn verify(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
    function_name: &str,
) -> Vec<Diagnostic> {
    let borrowed = aggregate_instructions::borrowed_values(body);
    let mut diagnostics = Vec::new();

    for block in &body.blocks {
        let Some(instructions) = &block.instructions else {
            continue;
        };
        for instruction in instructions {
            if !consumes_borrowed_value(instruction.opcode) {
                continue;
            }
            for value in instruction_values(instruction) {
                if is_borrowed(value, &borrowed, value_types, types) {
                    diagnostics.push(Diagnostic::error(
                        V5,
                        format!(
                            "function @{function_name} CODE offset {}: borrowed value {value} is used by consuming opcode 0x{:03X}",
                            instruction.offset, instruction.opcode
                        ),
                    ));
                }
            }
        }
    }

    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
    {
        match instruction.opcode {
            0x1a0 | 0x1a1 | 0x1a3 | 0x1a5 | 0x1a6 | 0x1a7 | 0x1aa | 0x1e7 | 0x1a9 => {
                let Some(function_type) = call_function_type(
                    instruction,
                    value_types,
                    signatures,
                    types,
                    generics,
                    strings,
                    imports,
                    globals,
                ) else {
                    continue;
                };
                let Some(BinaryType::Function(function)) = usize::try_from(function_type)
                    .ok()
                    .and_then(|index| types.entries.get(index))
                else {
                    continue;
                };
                let arguments = call_arguments(instruction);
                for (argument, parameter) in arguments.iter().zip(&function.parameters) {
                    if is_borrowed(*argument, &borrowed, value_types, types)
                        && !matches!(parameter.convention, 1 | 2 | 3)
                    {
                        diagnostics.push(Diagnostic::error(
                            V5,
                            format!(
                                "function @{function_name} CODE offset {}: borrowed value {argument} is passed to an owned call parameter",
                                instruction.offset
                            ),
                        ));
                    }
                }
                if instruction.opcode == 0x1a9 {
                    for value in variadic_arguments(instruction) {
                        if is_borrowed(value, &borrowed, value_types, types) {
                            diagnostics.push(Diagnostic::error(
                                V5,
                                format!(
                                    "function @{function_name} CODE offset {}: borrowed value {value} is passed to an untyped variadic argument",
                                    instruction.offset
                                ),
                            ));
                        }
                    }
                }
            }
            0x1a8 => {
                for value in instruction_values(instruction) {
                    if is_borrowed(value, &borrowed, value_types, types) {
                        diagnostics.push(Diagnostic::error(
                            V5,
                            format!(
                                "function @{function_name} CODE offset {}: borrowed value {value} is passed to a foreign call",
                                instruction.offset
                            ),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    for (source, block) in body.blocks.iter().enumerate() {
        let Some(terminator) = block.instructions.as_ref().and_then(|items| items.last()) else {
            continue;
        };
        super::structure::visit_edges(terminator, |target, arguments| {
            let Some(target_block) = usize::try_from(target)
                .ok()
                .and_then(|index| body.blocks.get(index))
            else {
                return;
            };
            for (argument, parameter) in arguments.iter().zip(&target_block.parameters) {
                if is_borrowed(*argument, &borrowed, value_types, types)
                    && !matches!(parameter.convention, 1 | 2)
                    && !aggregate_instructions::is_trivial_type(types, parameter.ty)
                {
                    diagnostics.push(Diagnostic::error(
                        V5,
                        format!(
                            "function @{function_name} block {source}: borrowed value {argument} is transferred to an owned parameter of block {target}"
                        ),
                    ));
                }
            }
        });
    }

    diagnostics
}

fn is_borrowed(
    value: u64,
    borrowed: &HashSet<u64>,
    _value_types: &[Option<u64>],
    _types: &BinaryTypeTable,
) -> bool {
    borrowed.contains(&value)
}

fn instruction_values(instruction: &BinaryInstruction) -> Vec<u64> {
    instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        })
        .copied()
        .collect()
}

fn consumes_borrowed_value(opcode: u64) -> bool {
    matches!(
        opcode,
        0x142 | 0x181 | 0x182 | 0x185 | 0x1a2 | 0x1e5 | 0x1e6
    )
}

fn call_function_type(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
) -> Option<u64> {
    match instruction.opcode {
        0x1a0 | 0x1a7 | 0x1aa | 0x1e7 => {
            let signature = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Symbol { kind: 0, index } => Some(*index),
                    _ => None,
                })?;
            usize::try_from(signature)
                .ok()
                .and_then(|index| signatures.records.get(index))
                .map(|signature| signature.function_type)
        }
        0x1a1 | 0x1a9 => instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Type(index) => Some(*index),
                _ => None,
            }),
        0x1a3 => {
            let closure = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Value(value) => Some(*value),
                    _ => None,
                })?;
            let ty = usize::try_from(closure)
                .ok()
                .and_then(|index| value_types.get(index))
                .copied()
                .flatten()?;
            match usize::try_from(ty)
                .ok()
                .and_then(|index| types.entries.get(index))?
            {
                BinaryType::Closure { function_type, .. } => Some(*function_type),
                _ => None,
            }
        }
        0x1a5 | 0x1a6 => super::owned_values::witness_or_vtable_function_type(
            instruction,
            signatures,
            generics,
            strings,
            imports,
            globals,
        ),
        _ => None,
    }
}

fn call_arguments(instruction: &BinaryInstruction) -> Vec<u64> {
    if instruction.opcode == 0x1a9 {
        let fixed = fixed_argument_count(instruction).unwrap_or(0);
        return instruction_values(instruction)
            .into_iter()
            .take(fixed)
            .collect();
    }
    let explicit = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Values(values) => Some(values.clone()),
            _ => None,
        })
        .unwrap_or_default();
    if instruction.opcode == 0x1a6 {
        let Some(receiver) = instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Value(value) => Some(*value),
                _ => None,
            })
        else {
            return Vec::new();
        };
        let mut arguments = Vec::with_capacity(explicit.len() + 1);
        arguments.push(receiver);
        arguments.extend(explicit);
        arguments
    } else {
        explicit
    }
}

fn variadic_arguments(instruction: &BinaryInstruction) -> Vec<u64> {
    let fixed = fixed_argument_count(instruction).unwrap_or(0);
    instruction_values(instruction)
        .into_iter()
        .skip(fixed)
        .collect()
}

fn fixed_argument_count(instruction: &BinaryInstruction) -> Option<usize> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(count) => usize::try_from(*count).ok(),
            _ => None,
        })
}
