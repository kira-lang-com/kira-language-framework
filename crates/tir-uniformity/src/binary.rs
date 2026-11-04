use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryCodeTable, BinaryGenericTable, BinaryGpuEntry, BinaryGpuInterfaceRole, BinaryGpuTable,
    BinaryImport, BinaryImportEntityKind, BinaryLinkage, BinaryOperand, BinarySignature,
    BinarySignatureTable, BinaryTarget, StringTable,
};

use super::binary_cfg::{
    control_flow_edges, last_index, successors, update_block_parameters, update_control,
    value_operands, value_uniformity,
};
use crate::Uniformity;

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);
const SCOPE_MASK: u64 = 7 << 26;
const SELECTOR_MASK: u64 = 0xf << 48;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;

struct FunctionInfo {
    signature_index: usize,
    name: String,
    control: Vec<Uniformity>,
    required_on_entry: Option<Uniformity>,
    calls: Vec<CallSite>,
}

struct CallSite {
    block: usize,
    offset: u64,
    targets: Vec<usize>,
}

pub fn verify_binary_gpu_module(
    code: &BinaryCodeTable,
    signatures: &BinarySignatureTable,
    gpu: Option<&BinaryGpuTable>,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    imports: Option<&[BinaryImport]>,
) -> Vec<Diagnostic> {
    let imported_function_count = imports
        .unwrap_or_default()
        .iter()
        .flat_map(|import| &import.entities)
        .filter(|entity| entity.kind == BinaryImportEntityKind::Function)
        .count() as u64;
    let gpu_entries = gpu.map_or(&[][..], |gpu| gpu.entries.as_slice());
    let bodies = code
        .functions
        .iter()
        .filter_map(|body| {
            let index = usize::try_from(body.function_index).ok()?;
            let signature = signatures.records.get(index)?;
            (matches!(signature.target, BinaryTarget::Gpu | BinaryTarget::Both)
                && body.blocks.iter().all(|block| block.instructions.is_some()))
            .then_some((index, body, signature))
        })
        .collect::<Vec<_>>();
    let body_indexes = bodies
        .iter()
        .map(|(index, _, _)| *index)
        .collect::<HashSet<_>>();
    let mut functions = Vec::with_capacity(bodies.len());
    let mut diagnostics = Vec::new();

    for (signature_index, body, signature) in bodies {
        let entry = gpu_entries
            .iter()
            .find(|entry| entry.function_index == signature_index as u64);
        let entry_parameters = entry.map(|entry| entry_parameter_uniformity(entry, body));
        let (control, required_on_entry, local_diagnostics) =
            analyze_function(body, signature, entry_parameters.as_deref(), strings);
        diagnostics.extend(local_diagnostics);
        functions.push(FunctionInfo {
            signature_index,
            name: signature.name.clone(),
            control,
            required_on_entry,
            calls: collect_calls(
                body,
                signatures,
                generics,
                strings,
                &body_indexes,
                imported_function_count,
            ),
        });
    }

    let function_by_signature = functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.signature_index, index))
        .collect::<HashMap<_, _>>();
    let summaries = call_summaries(&functions, &function_by_signature);
    diagnostics.extend(verify_call_sites(
        &functions,
        &function_by_signature,
        &summaries,
    ));
    diagnostics
}

fn entry_parameter_uniformity(
    entry: &BinaryGpuEntry,
    body: &tir_format::BinaryFunctionBody,
) -> Vec<Uniformity> {
    let count = body
        .blocks
        .first()
        .map_or(0, |block| block.parameters.len());
    let mut parameters = vec![Uniformity::Divergent; count];
    for interface in &entry.interfaces {
        let Some(index) = interface.parameter_index else {
            continue;
        };
        let uniformity = match interface.role {
            BinaryGpuInterfaceRole::Root | BinaryGpuInterfaceRole::Immediate => {
                Uniformity::Dispatch
            }
            BinaryGpuInterfaceRole::StageParameter
            | BinaryGpuInterfaceRole::IntersectionAttributes => Uniformity::Divergent,
        };
        if let Ok(index) = usize::try_from(index)
            && let Some(parameter) = parameters.get_mut(index)
        {
            *parameter = uniformity;
        }
    }
    parameters
}

fn analyze_function(
    body: &tir_format::BinaryFunctionBody,
    signature: &BinarySignature,
    entry_parameters: Option<&[Uniformity]>,
    strings: Option<&StringTable>,
) -> (Vec<Uniformity>, Option<Uniformity>, Vec<Diagnostic>) {
    let mut block_starts = Vec::with_capacity(body.blocks.len());
    let mut instruction_starts = Vec::with_capacity(body.blocks.len());
    let mut value_count = 0usize;
    for block in &body.blocks {
        block_starts.push(value_count);
        value_count = value_count.saturating_add(block.parameters.len());
        let mut starts = Vec::new();
        if let Some(instructions) = &block.instructions {
            starts.reserve(instructions.len());
            for instruction in instructions {
                starts.push(value_count);
                value_count = value_count.saturating_add(
                    usize::try_from(instruction.result_count).unwrap_or(usize::MAX),
                );
            }
        }
        instruction_starts.push(starts);
    }

    let mut values = vec![Uniformity::Dispatch; value_count];
    if let Some(parameters) = entry_parameters {
        for (index, uniformity) in parameters.iter().copied().enumerate() {
            if let Some(value) = values.get_mut(index) {
                *value = uniformity;
            }
        }
    } else if let Some(first) = body.blocks.first() {
        for index in 0..first.parameters.len() {
            if let Some(value) = values.get_mut(index) {
                *value = Uniformity::Divergent;
            }
        }
    }

    let edges = control_flow_edges(body);
    let successors = successors(body.blocks.len(), &edges);
    let mut control = vec![Uniformity::Dispatch; body.blocks.len()];
    loop {
        let mut changed = false;
        for (block_index, block) in body.blocks.iter().enumerate() {
            let Some(instructions) = &block.instructions else {
                continue;
            };
            for (instruction_index, instruction) in instructions.iter().enumerate() {
                let candidate = instruction_uniformity(instruction, &values, strings);
                let start = instruction_starts[block_index][instruction_index];
                for result_index in 0..usize::try_from(instruction.result_count).unwrap_or(0) {
                    if let Some(value) = values.get_mut(start + result_index) {
                        let raised = value.join(candidate);
                        if raised != *value {
                            *value = raised;
                            changed = true;
                        }
                    }
                }
            }
        }
        changed |= update_control(body, &successors, &values, &mut control);
        let previous_values = values.clone();
        changed |= update_block_parameters(
            body,
            &edges,
            &block_starts,
            &previous_values,
            &control,
            &mut values,
        );
        if !changed {
            break;
        }
    }

    let mut diagnostics = Vec::new();
    let mut required_on_entry = None;
    for (block_index, block) in body.blocks.iter().enumerate() {
        let Some(instructions) = &block.instructions else {
            continue;
        };
        for instruction in instructions {
            if let Some((operand_index, required, operation)) =
                dynamic_uniform_operand(instruction.opcode)
            {
                let selector = value_operands(&instruction.operands)
                    .get(operand_index)
                    .and_then(|value| value_uniformity(*value, &values))
                    .unwrap_or(Uniformity::Divergent);
                if !selector.satisfies(required) {
                    diagnostics.push(Diagnostic::error(
                        V15,
                        format!(
                            "{operation} lane selector must be dynamically {required:?}-uniform in GPU function @{} at CODE offset {}",
                            signature.name, instruction.offset
                        ),
                    ));
                }
            }
            let Some((required, description)) = required_uniformity(instruction) else {
                continue;
            };
            required_on_entry = Some(
                required_on_entry.map_or(required, |current: Uniformity| current.min(required)),
            );
            if instruction.modifiers & UNCHECKED_UNIFORMITY != 0
                || control
                    .get(block_index)
                    .is_some_and(|current| current.satisfies(required))
            {
                continue;
            }
            diagnostics.push(Diagnostic::error(
                V15,
                format!(
                    "{description} requires {required:?}-uniform control flow in GPU function @{} at CODE offset {}",
                    signature.name, instruction.offset
                ),
            ));
        }
    }
    (control, required_on_entry, diagnostics)
}

fn instruction_uniformity(
    instruction: &tir_format::BinaryInstruction,
    values: &[Uniformity],
    strings: Option<&StringTable>,
) -> Uniformity {
    let operands = value_operands(&instruction.operands);
    let joined = join_values(&operands, values);
    match instruction.opcode {
        0x400 => builtin_uniformity(instruction, strings),
        0x401 | 0x00a | 0x00b | 0x00d | 0x00e | 0x00f | 0x010 | 0x223 | 0x224 => {
            Uniformity::Dispatch
        }
        0x00c | 0x440 | 0x447 => Uniformity::Divergent,
        0x120 => {
            if (instruction.modifiers & SELECTOR_MASK) >> 48 == 3 {
                Uniformity::Threadgroup
            } else {
                Uniformity::Divergent
            }
        }
        0x140 | 0x141 | 0x146 => operands
            .first()
            .and_then(|value| value_uniformity(*value, values))
            .unwrap_or(Uniformity::Divergent),
        0x1a0..=0x1a9 | 0x1e7 | 0x404 | 0x200..=0x204 | 0x48a => Uniformity::Divergent,
        0x423 => operands
            .get(1)
            .and_then(|lane| value_uniformity(*lane, values))
            .filter(|uniformity| uniformity.satisfies(Uniformity::Quad))
            .map_or(Uniformity::Divergent, |_| Uniformity::Quad),
        0x425 | 0x426 => Uniformity::Quad,
        0x441..=0x443 | 0x445 | 0x446 | 0x448..=0x44a => Uniformity::Subgroup,
        0x444 => operands
            .get(1)
            .and_then(|lane| value_uniformity(*lane, values))
            .filter(|uniformity| uniformity.satisfies(Uniformity::Subgroup))
            .map_or(Uniformity::Divergent, |_| Uniformity::Subgroup),
        0x450 => {
            if last_index(&instruction.operands) == Some(0) {
                Uniformity::Subgroup
            } else if joined.satisfies(Uniformity::Subgroup) {
                joined
            } else {
                Uniformity::Divergent
            }
        }
        0x451 | 0x452 => Uniformity::Divergent,
        0x000..=0x009 | 0x011 => Uniformity::Dispatch,
        _ if operands.is_empty() => Uniformity::Divergent,
        _ => joined,
    }
}

fn builtin_uniformity(
    instruction: &tir_format::BinaryInstruction,
    strings: Option<&StringTable>,
) -> Uniformity {
    let name = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => usize::try_from(*index)
                .ok()
                .and_then(|index| strings.and_then(|strings| strings.get(index))),
            _ => None,
        });
    match name {
        Some("threadgroup_position_in_grid") => Uniformity::Threadgroup,
        Some("subgroup_index_in_threadgroup") => Uniformity::Subgroup,
        Some(
            "threads_per_threadgroup"
            | "threadgroups_per_grid"
            | "subgroups_per_threadgroup"
            | "subgroup_size"
            | "base_vertex"
            | "base_instance"
            | "draw_index",
        ) => Uniformity::Dispatch,
        _ => Uniformity::Divergent,
    }
}

fn required_uniformity(
    instruction: &tir_format::BinaryInstruction,
) -> Option<(Uniformity, &'static str)> {
    match instruction.opcode {
        0x402 => match (instruction.modifiers & SCOPE_MASK) >> 26 {
            0 | 3 => Some((Uniformity::Threadgroup, "threadgroup barrier")),
            2 => Some((Uniformity::Subgroup, "subgroup barrier")),
            _ => None,
        },
        0x420..=0x426 | 0x480 | 0x483 | 0x484 | 0x486 | 0x487 | 0x48c | 0x48d => Some((
            Uniformity::Quad,
            "derivative, implicit sampling, or quad operation",
        )),
        0x4c0 | 0x4c1 => Some((Uniformity::Threadgroup, "mesh dispatch operation")),
        _ => None,
    }
}

fn dynamic_uniform_operand(opcode: u64) -> Option<(usize, Uniformity, &'static str)> {
    match opcode {
        0x423 => Some((1, Uniformity::Quad, "quad.broadcast")),
        0x444 => Some((1, Uniformity::Subgroup, "subgroup.broadcast")),
        _ => None,
    }
}

fn collect_calls(
    body: &tir_format::BinaryFunctionBody,
    signatures: &BinarySignatureTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    gpu_body_indexes: &HashSet<usize>,
    imported_function_count: u64,
) -> Vec<CallSite> {
    body.blocks
        .iter()
        .enumerate()
        .flat_map(|(block, basic_block)| {
            basic_block
                .instructions
                .as_ref()
                .into_iter()
                .flatten()
                .filter_map(move |instruction| {
                    let targets = match instruction.opcode {
                        0x1a0 => direct_target(instruction, imported_function_count)
                            .filter(|target| gpu_body_indexes.contains(target))
                            .into_iter()
                            .collect(),
                        0x1e7 if (instruction.modifiers >> 48) & 0xf == 0 => {
                            direct_target(instruction, imported_function_count)
                                .filter(|target| gpu_body_indexes.contains(target))
                                .into_iter()
                                .collect()
                        }
                        0x1a5 => witness_target(instruction, generics, strings, gpu_body_indexes),
                        0x404 => visible_targets(instruction, signatures, gpu_body_indexes),
                        _ => return None,
                    };
                    Some(CallSite {
                        block,
                        offset: instruction.offset,
                        targets,
                    })
                })
        })
        .collect()
}

fn witness_target(
    instruction: &tir_format::BinaryInstruction,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    gpu_body_indexes: &HashSet<usize>,
) -> Vec<usize> {
    let type_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        });
    let method_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        });
    let method_path = method_index
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| strings.and_then(|strings| strings.get(index)));
    let resolved = type_index
        .zip(method_path)
        .and_then(|(type_index, method_path)| {
            let (trait_path, method) = method_path.rsplit_once('.')?;
            let matching = generics?
                .conformances
                .iter()
                .filter(|conformance| {
                    conformance.type_index == type_index && conformance.trait_path == trait_path
                })
                .collect::<Vec<_>>();
            let [conformance] = matching.as_slice() else {
                return None;
            };
            let targets = conformance
                .witnesses
                .iter()
                .filter(|witness| witness.method == method)
                .map(|witness| witness.function_index)
                .collect::<Vec<_>>();
            let [target] = targets.as_slice() else {
                return None;
            };
            usize::try_from(*target)
                .ok()
                .filter(|target| gpu_body_indexes.contains(target))
        });
    resolved.map_or_else(
        || gpu_body_indexes.iter().copied().collect(),
        |target| vec![target],
    )
}

fn direct_target(
    instruction: &tir_format::BinaryInstruction,
    imported_function_count: u64,
) -> Option<usize> {
    let symbol_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => Some(*index),
            _ => None,
        })?;
    usize::try_from(symbol_index.checked_sub(imported_function_count)?).ok()
}

fn visible_targets(
    instruction: &tir_format::BinaryInstruction,
    signatures: &BinarySignatureTable,
    gpu_body_indexes: &HashSet<usize>,
) -> Vec<usize> {
    let Some(function_type) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })
    else {
        return Vec::new();
    };
    signatures
        .records
        .iter()
        .enumerate()
        .filter_map(|(index, signature)| {
            (gpu_body_indexes.contains(&index)
                && signature.function_type == function_type
                && signature.linkage == BinaryLinkage::Exported
                && signature.target == BinaryTarget::Gpu
                && signature.entry_kind.is_none())
            .then_some(index)
        })
        .collect()
}

fn call_summaries(
    functions: &[FunctionInfo],
    function_by_signature: &HashMap<usize, usize>,
) -> Vec<Option<Uniformity>> {
    let mut summaries = functions
        .iter()
        .map(|function| function.required_on_entry)
        .collect::<Vec<_>>();
    loop {
        let previous = summaries.clone();
        let mut changed = false;
        for (index, function) in functions.iter().enumerate() {
            for call in &function.calls {
                let requirement = call
                    .targets
                    .iter()
                    .filter_map(|target| {
                        function_by_signature
                            .get(target)
                            .and_then(|target| previous.get(*target))
                            .copied()
                            .flatten()
                    })
                    .min();
                let Some(requirement) = requirement else {
                    continue;
                };
                let raised =
                    previous[index].map_or(requirement, |current| current.min(requirement));
                if summaries[index] != Some(raised) {
                    summaries[index] = Some(raised);
                    changed = true;
                }
            }
        }
        if !changed {
            return summaries;
        }
    }
}

fn verify_call_sites(
    functions: &[FunctionInfo],
    function_by_signature: &HashMap<usize, usize>,
    summaries: &[Option<Uniformity>],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for function in functions {
        for call in &function.calls {
            let requirement = call
                .targets
                .iter()
                .filter_map(|target| {
                    function_by_signature
                        .get(target)
                        .and_then(|target| summaries.get(*target))
                        .copied()
                        .flatten()
                })
                .min();
            let Some(required) = requirement else {
                continue;
            };
            if function
                .control
                .get(call.block)
                .is_some_and(|control| control.satisfies(required))
            {
                continue;
            }
            diagnostics.push(Diagnostic::error(
                V15,
                format!(
                    "call requires {required:?}-uniform control flow in GPU function @{} at CODE offset {}",
                    function.name, call.offset
                ),
            ));
        }
    }
    diagnostics
}

fn join_values(values: &[u64], uniformities: &[Uniformity]) -> Uniformity {
    if values.is_empty() {
        return Uniformity::Divergent;
    }
    values.iter().fold(Uniformity::Dispatch, |current, value| {
        current.join(value_uniformity(*value, uniformities).unwrap_or(Uniformity::Divergent))
    })
}
