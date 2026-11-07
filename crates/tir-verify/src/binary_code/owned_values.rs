use std::collections::{HashSet, VecDeque};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryGenericTable, BinaryGlobalTable, BinaryImport,
    BinaryImportEntityKind, BinaryInstruction, BinaryOperand, BinarySignatureTable, BinaryType,
    BinaryTypeTable, StringTable,
};

use super::types::aggregate_instructions;

const V4: Code = Code::new(Dialect::Tir, Area::Verifier, 4);

#[derive(Clone, Copy)]
struct DefinitionSite {
    block: usize,
    instruction: usize,
    parameter: bool,
}

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
    let definitions = definition_sites(body, value_types.len());
    let mut diagnostics = Vec::new();
    for (value, definition) in definitions.into_iter().enumerate() {
        let Some(definition) = definition else {
            continue;
        };
        let Some(ty) = value_types.get(value).copied().flatten() else {
            continue;
        };
        if borrowed.contains(&(value as u64))
            || matches!(
                types.entries.get(ty as usize),
                Some(BinaryType::Address { .. })
            )
            || aggregate_instructions::is_trivial_type(types, ty)
        {
            continue;
        }
        verify_value(
            value as u64,
            definition,
            body,
            value_types,
            signatures,
            types,
            generics,
            strings,
            imports,
            globals,
            function_name,
            &mut diagnostics,
        );
    }
    diagnostics
}

fn definition_sites(body: &BinaryFunctionBody, value_count: usize) -> Vec<Option<DefinitionSite>> {
    let mut sites = vec![None; value_count];
    let mut cursor = 0_usize;
    for (block_index, block) in body.blocks.iter().enumerate() {
        for _ in &block.parameters {
            if let Some(site) = sites.get_mut(cursor) {
                *site = Some(DefinitionSite {
                    block: block_index,
                    instruction: 0,
                    parameter: true,
                });
            }
            cursor = cursor.saturating_add(1);
        }
        if let Some(instructions) = &block.instructions {
            for (instruction_index, instruction) in instructions.iter().enumerate() {
                for _ in 0..instruction.result_count {
                    if let Some(site) = sites.get_mut(cursor) {
                        *site = Some(DefinitionSite {
                            block: block_index,
                            instruction: instruction_index + 1,
                            parameter: false,
                        });
                    }
                    cursor = cursor.saturating_add(1);
                }
            }
        } else {
            cursor = cursor.saturating_add(usize::try_from(block.instruction_count).unwrap_or(0));
        }
    }
    sites
}

#[allow(clippy::too_many_arguments)]
fn verify_value(
    value: u64,
    definition: DefinitionSite,
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut pending = VecDeque::from([(definition.block, definition.instruction, false)]);
    let mut visited = HashSet::new();
    while let Some((block_index, start, initially_consumed)) = pending.pop_front() {
        if !visited.insert((block_index, start, initially_consumed)) {
            continue;
        }
        let Some(block) = body.blocks.get(block_index) else {
            continue;
        };
        let Some(instructions) = &block.instructions else {
            continue;
        };
        let mut consumed = initially_consumed;
        for (instruction_index, instruction) in instructions.iter().enumerate().skip(start) {
            if !definition.parameter
                && start == 0
                && block_index == definition.block
                && instruction_index + 1 == definition.instruction
            {
                if !consumed {
                    report(
                        diagnostics,
                        function_name,
                        value,
                        "not consumed before its next loop definition",
                        instruction.offset,
                    );
                    return;
                }
                if all_values(instruction).contains(&value) {
                    report(
                        diagnostics,
                        function_name,
                        value,
                        "used after it was consumed",
                        instruction.offset,
                    );
                    return;
                }
                consumed = false;
            } else if consumed && all_values(instruction).contains(&value) {
                report(
                    diagnostics,
                    function_name,
                    value,
                    "used after it was consumed",
                    instruction.offset,
                );
                return;
            }
            if instruction.opcode == 0x260 {
                let moved = consumed_values(
                    instruction,
                    value_types,
                    signatures,
                    types,
                    generics,
                    strings,
                    imports,
                    globals,
                );
                let Some(deopt_state) = consume(value, consumed, &moved) else {
                    report(
                        diagnostics,
                        function_name,
                        value,
                        "consumed more than once on the guard deoptimization path",
                        instruction.offset,
                    );
                    return;
                };
                if !deopt_state {
                    report(
                        diagnostics,
                        function_name,
                        value,
                        "not consumed on the guard deoptimization path",
                        instruction.offset,
                    );
                    return;
                }
                continue;
            }

            let moved = consumed_values(
                instruction,
                value_types,
                signatures,
                types,
                generics,
                strings,
                imports,
                globals,
            );
            let Some(next_consumed) = consume(value, consumed, &moved) else {
                report(
                    diagnostics,
                    function_name,
                    value,
                    "consumed more than once",
                    instruction.offset,
                );
                return;
            };
            consumed = next_consumed;

            if !instruction.terminator {
                continue;
            }
            match instruction.opcode {
                0x1e9 => return,
                0x1e8 if ((instruction.modifiers >> 48) & 0xf) == 0 => return,
                0x1e5 | 0x1e6 | 0x1e7 | 0x1d1 | 0x1d2 | 0x1c3 | 0x261 => {
                    if !consumed {
                        report(
                            diagnostics,
                            function_name,
                            value,
                            "not consumed on a path to a function exit",
                            instruction.offset,
                        );
                        return;
                    }
                    return;
                }
                0x1e8 => {
                    if !consumed {
                        report(
                            diagnostics,
                            function_name,
                            value,
                            "not consumed on a path to `unreachable [ub]`",
                            instruction.offset,
                        );
                        return;
                    }
                    return;
                }
                _ => {
                    let mut invalid_transfer = false;
                    let mut edge_use_after = false;
                    let mut invalid_redefinition = false;
                    super::structure::visit_edges(instruction, |target, arguments| {
                        let Ok(target_index) = usize::try_from(target) else {
                            return;
                        };
                        let Some(target_block) = body.blocks.get(target_index) else {
                            return;
                        };
                        if consumed && arguments.contains(&value) {
                            edge_use_after = true;
                            invalid_transfer = true;
                            return;
                        }
                        let transferred = arguments
                            .iter()
                            .zip(&target_block.parameters)
                            .filter(|(argument, parameter)| {
                                **argument == value
                                    && !matches!(parameter.convention, 1 | 2 | 3)
                                    && !aggregate_instructions::is_trivial_type(types, parameter.ty)
                            })
                            .count();
                        if transferred > 1 || (transferred == 1 && consumed) {
                            invalid_transfer = true;
                        } else if definition.parameter && target_index == definition.block {
                            if !consumed && transferred == 0 {
                                invalid_transfer = true;
                                invalid_redefinition = true;
                            } else {
                                pending.push_back((target_index, 0, false));
                            }
                        } else {
                            pending.push_back((target_index, 0, consumed || transferred == 1));
                        }
                    });
                    if invalid_transfer {
                        report(
                            diagnostics,
                            function_name,
                            value,
                            if edge_use_after {
                                "used after it was consumed on a block edge"
                            } else if invalid_redefinition {
                                "not consumed before its next loop parameter definition"
                            } else {
                                "consumed more than once while transferring to a block parameter"
                            },
                            instruction.offset,
                        );
                        return;
                    }
                    break;
                }
            }
        }
    }
}

fn consumed_values(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
) -> Vec<u64> {
    match instruction.opcode {
        0x181 | 0x182 | 0x185 | 0x103 | 0x106 | 0x10c | 0x1e3 | 0x1e5 | 0x1e6 | 0x1d1 | 0x1d2
        | 0x1c1 | 0x1c4 | 0x1c8 => first_value(instruction).into_iter().collect(),
        0x142 if ((instruction.modifiers >> 48) & 0xf) != 0 => first_values(instruction)
            .into_iter()
            .next()
            .into_iter()
            .collect(),
        0x100 | 0x104 | 0x107 | 0x10a => all_values(instruction),
        0x1a2 => all_values(instruction)
            .into_iter()
            .nth(1)
            .into_iter()
            .collect(),
        0x260 => all_values(instruction).into_iter().skip(1).collect(),
        0x261 => all_values(instruction),
        0x1a0 | 0x1a7 | 0x1a8 | 0x1aa | 0x1e7 | 0x1c0 => {
            let arguments = all_values(instruction);
            owned_call_arguments(
                &arguments,
                direct_function_type(instruction, signatures),
                types,
            )
        }
        0x1a1 => {
            let arguments = all_values(instruction)
                .into_iter()
                .skip(1)
                .collect::<Vec<_>>();
            owned_call_arguments(&arguments, type_operand(instruction), types)
        }
        0x1a3 => {
            let arguments = all_values(instruction)
                .into_iter()
                .skip(1)
                .collect::<Vec<_>>();
            let closure_type = first_value(instruction)
                .and_then(|value| value_type(value, value_types))
                .and_then(|ty| match types.entries.get(ty as usize) {
                    Some(BinaryType::Closure { function_type, .. }) => Some(*function_type),
                    _ => None,
                });
            owned_call_arguments(&arguments, closure_type, types)
        }
        0x1a9 => {
            let fixed_arguments = all_values(instruction);
            owned_call_arguments(&fixed_arguments, type_operand(instruction), types)
        }
        0x1a5 | 0x1a6 => owned_call_arguments(
            &all_values(instruction),
            witness_or_vtable_function_type(
                instruction,
                signatures,
                generics,
                strings,
                imports,
                globals,
            ),
            types,
        ),
        _ => Vec::new(),
    }
}

pub(super) fn witness_or_vtable_function_type(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
) -> Option<u64> {
    match instruction.opcode {
        0x1a5 => witness_function_type(instruction, signatures, generics, strings),
        0x1a6 => vtable_function_type(instruction, signatures, generics, imports, globals),
        _ => None,
    }
}

fn witness_function_type(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
) -> Option<u64> {
    let type_index = type_operand(instruction)?;
    let method_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })?;
    let method_path = strings?.get(usize::try_from(method_index).ok()?)?;
    let (trait_path, method) = method_path.rsplit_once('.')?;
    let generics = generics?;
    let conformances = generics
        .conformances
        .iter()
        .filter(|conformance| {
            conformance.type_index == type_index && conformance.trait_path == trait_path
        })
        .collect::<Vec<_>>();
    if let [conformance] = conformances.as_slice() {
        let targets = conformance
            .witnesses
            .iter()
            .filter(|witness| witness.method == method)
            .map(|witness| witness.function_index)
            .collect::<Vec<_>>();
        if let [target] = targets.as_slice() {
            return signatures
                .records
                .get(usize::try_from(*target).ok()?)
                .map(|signature| signature.function_type);
        }
    }
    let methods = generics
        .traits
        .iter()
        .find(|definition| definition.name == trait_path)?
        .methods
        .iter()
        .filter(|required| required.name == method)
        .map(|required| required.function_type)
        .collect::<Vec<_>>();
    match methods.as_slice() {
        [function_type] => Some(*function_type),
        _ => None,
    }
}

fn vtable_function_type(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
    generics: Option<&BinaryGenericTable>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
) -> Option<u64> {
    let vtable_symbol = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 1, index } => Some(*index),
            _ => None,
        })?;
    let slot = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })?;
    let imported_names = imports
        .into_iter()
        .flat_map(|imports| imports.iter())
        .flat_map(|import| &import.entities)
        .filter(|entity| {
            matches!(
                entity.kind,
                BinaryImportEntityKind::Global | BinaryImportEntityKind::PlatformData
            )
        })
        .map(|entity| entity.name.as_str())
        .collect::<Vec<_>>();
    let vtable_name: &str = if vtable_symbol < imported_names.len() as u64 {
        *imported_names.get(usize::try_from(vtable_symbol).ok()?)?
    } else {
        let global_index = vtable_symbol.checked_sub(imported_names.len() as u64)?;
        globals?
            .records
            .get(usize::try_from(global_index).ok()?)?
            .name
            .as_str()
    };
    let vtable = generics?
        .vtables
        .iter()
        .find(|vtable| vtable.name == vtable_name)?;
    let target = vtable
        .slots
        .iter()
        .find(|entry| entry.slot == slot)?
        .function_index;
    signatures
        .records
        .get(usize::try_from(target).ok()?)
        .map(|signature| signature.function_type)
}

fn direct_function_type(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
) -> Option<u64> {
    let index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => Some(*index),
            _ => None,
        })?;
    signatures
        .records
        .get(usize::try_from(index).ok()?)
        .map(|signature| signature.function_type)
}

fn type_operand(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })
}

fn owned_call_arguments(
    arguments: &[u64],
    function_type: Option<u64>,
    types: &BinaryTypeTable,
) -> Vec<u64> {
    let Some(BinaryType::Function(function)) = function_type
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| types.entries.get(index))
    else {
        return Vec::new();
    };
    arguments
        .iter()
        .zip(&function.parameters)
        .filter_map(|(argument, parameter)| {
            (!matches!(parameter.convention, 1 | 2 | 3)).then_some(*argument)
        })
        .collect()
}

fn first_value(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Value(value) => Some(*value),
            BinaryOperand::Values(values) => values.first().copied(),
            BinaryOperand::OptionalValue(Some(value)) => Some(*value),
            _ => None,
        })
}

fn first_values(instruction: &BinaryInstruction) -> Vec<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Value(value) => Some(vec![*value]),
            BinaryOperand::Values(values) => Some(values.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn all_values(instruction: &BinaryInstruction) -> Vec<u64> {
    instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            BinaryOperand::OptionalValue(Some(value)) => std::slice::from_ref(value),
            _ => &[],
        })
        .copied()
        .collect()
}

fn value_type(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn consume(value: u64, consumed: bool, operands: &[u64]) -> Option<bool> {
    let count = operands.iter().filter(|operand| **operand == value).count();
    if count > 1 || (count == 1 && consumed) {
        None
    } else {
        Some(consumed || count == 1)
    }
}

fn report(
    diagnostics: &mut Vec<Diagnostic>,
    function_name: &str,
    value: u64,
    issue: &str,
    offset: u64,
) {
    diagnostics.push(Diagnostic::error(
        V4,
        format!("function @{function_name} CODE offset {offset}: owned value {value} is {issue}"),
    ));
}
