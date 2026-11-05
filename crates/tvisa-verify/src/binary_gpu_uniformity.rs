use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::StringTable;
use tvisa_format::{
    BinaryFunctionBody, BinaryOperand, GpuEntryKind, GpuEntryRecord, TvisaImage, VariantIndex,
};

use crate::binary_gpu_uniformity_cfg::{
    control_flow_edges, successors, update_block_parameters, update_control, value_operands,
    value_uniformity,
};

const VFY22: Code = Code::new(Dialect::Tvisa, Area::Verifier, 22);
const SCOPE_MASK: u64 = 7 << 26;
const SELECTOR_MASK: u64 = 0xf << 48;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;

struct FunctionInfo {
    function_index: u64,
    name: String,
    control: Vec<Uniformity>,
    required_on_entry: Option<Uniformity>,
    calls: Vec<CallSite>,
}

struct CallSite {
    block: usize,
    offset: u64,
    targets: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Uniformity {
    Dispatch,
    Threadgroup,
    Subgroup,
    Quad,
    Divergent,
}

impl Uniformity {
    pub(super) fn join(self, other: Self) -> Self {
        self.max(other)
    }

    fn satisfies(self, required: Self) -> bool {
        self <= required
    }
}

pub(super) fn verify(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    if !image.sections.iter().any(|section| section.kind == 11) {
        return Vec::new();
    }
    let entries = match image.gpu_entry_records(memory_limit) {
        Ok(entries) => entries,
        Err(error) => return vec![error.diagnostic],
    };
    let bodies = match image.function_bodies(memory_limit) {
        Ok(bodies) => bodies,
        Err(error) => return vec![error.diagnostic],
    };
    let imports = match image.import_counts(memory_limit) {
        Ok(imports) => imports,
        Err(error) => return vec![error.diagnostic],
    };
    let functions = match image.function_records(memory_limit) {
        Ok(functions) => functions,
        Err(error) => return vec![error.diagnostic],
    };
    let variants = match image.variant_index(memory_limit) {
        Ok(variants) => variants,
        Err(error) => return vec![error.diagnostic],
    };
    let strings = match image.string_table(memory_limit) {
        Ok(strings) => strings,
        Err(error) => return vec![error.diagnostic],
    };
    let body_indexes = bodies
        .iter()
        .filter_map(|body| {
            let index = body.header.function_index.checked_sub(imports.functions)?;
            let function = usize::try_from(index)
                .ok()
                .and_then(|index| functions.get(index))?;
            (function.target != 0).then_some(body.header.function_index)
        })
        .collect::<HashSet<_>>();
    let visible = visible_targets(&entries, &functions, imports.functions);
    let mut analyzed = Vec::new();
    let mut diagnostics = Vec::new();
    for body in &bodies {
        if !body_indexes.contains(&body.header.function_index) {
            continue;
        }
        let Some(function_index) = body.header.function_index.checked_sub(imports.functions) else {
            continue;
        };
        let Some(function) = usize::try_from(function_index)
            .ok()
            .and_then(|index| functions.get(index))
        else {
            continue;
        };
        let (control, required_on_entry, local_diagnostics) =
            analyze_function(body, &function.name, &strings);
        diagnostics.extend(local_diagnostics);
        analyzed.push(FunctionInfo {
            function_index: body.header.function_index,
            name: function.name.clone(),
            control,
            required_on_entry,
            calls: collect_calls(body, &variants, &visible, &body_indexes),
        });
    }
    let function_by_index = analyzed
        .iter()
        .enumerate()
        .map(|(index, function)| (function.function_index, index))
        .collect::<HashMap<_, _>>();
    let summaries = call_summaries(&analyzed, &function_by_index);
    diagnostics.extend(verify_call_sites(&analyzed, &function_by_index, &summaries));
    diagnostics
}

fn visible_targets(
    entries: &[GpuEntryRecord],
    functions: &[tvisa_format::FunctionRecordSummary],
    import_count: u64,
) -> HashMap<u64, Vec<u64>> {
    let mut targets = HashMap::new();
    for entry in entries
        .iter()
        .filter(|entry| entry.kind == GpuEntryKind::Visible)
    {
        let Some(index) = entry.function_index.checked_sub(import_count) else {
            continue;
        };
        let Some(function) = usize::try_from(index)
            .ok()
            .and_then(|index| functions.get(index))
        else {
            continue;
        };
        targets
            .entry(function.signature_index)
            .or_insert_with(Vec::new)
            .push(entry.function_index);
    }
    targets
}

fn analyze_function(
    body: &BinaryFunctionBody,
    name: &str,
    strings: &StringTable,
) -> (Vec<Uniformity>, Option<Uniformity>, Vec<Diagnostic>) {
    let mut block_starts = Vec::with_capacity(body.blocks.len());
    let mut instruction_starts = Vec::with_capacity(body.blocks.len());
    let mut value_count = 0usize;
    for block in &body.blocks {
        block_starts.push(value_count);
        value_count = value_count.saturating_add(block.parameter_types.len());
        let mut starts = Vec::with_capacity(block.instructions.len());
        for instruction in &block.instructions {
            starts.push(value_count);
            value_count = value_count.saturating_add(usize::from(instruction.result_count));
        }
        instruction_starts.push(starts);
    }
    let mut values = vec![Uniformity::Dispatch; value_count];
    if let Some(first) = body.blocks.first() {
        for value in values.iter_mut().take(first.parameter_types.len()) {
            *value = Uniformity::Divergent;
        }
    }
    let edges = control_flow_edges(body);
    let successors = successors(body.blocks.len(), &edges);
    let mut control = vec![Uniformity::Dispatch; body.blocks.len()];
    loop {
        let mut changed = false;
        for (block_index, block) in body.blocks.iter().enumerate() {
            for (instruction_index, instruction) in block.instructions.iter().enumerate() {
                let candidate = instruction_uniformity(instruction, &values, strings);
                let start = instruction_starts[block_index][instruction_index];
                for result_index in 0..usize::from(instruction.result_count) {
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
        for instruction in &block.instructions {
            if let Some((operand_index, required, operation)) =
                dynamic_uniform_operand(instruction.opcode)
            {
                let selector = value_operands(&instruction.operands)
                    .get(operand_index)
                    .and_then(|value| value_uniformity(*value, &values))
                    .unwrap_or(Uniformity::Divergent);
                if !selector.satisfies(required) {
                    diagnostics.push(Diagnostic::error(
                        VFY22,
                        format!(
                            "{operation} lane selector must be dynamically {required:?}-uniform in GPU function @{name} at CODE offset {}",
                            body.header.offset.saturating_add(instruction.offset)
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
            let code_offset = body.header.offset.saturating_add(instruction.offset);
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "{description} requires {required:?}-uniform control flow in GPU function @{name} at CODE offset {code_offset}"
                ),
            ));
        }
    }
    (control, required_on_entry, diagnostics)
}

fn instruction_uniformity(
    instruction: &tvisa_format::BinaryInstruction,
    values: &[Uniformity],
    strings: &StringTable,
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
    instruction: &tvisa_format::BinaryInstruction,
    strings: &StringTable,
) -> Uniformity {
    let name = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => usize::try_from(*index)
                .ok()
                .and_then(|index| strings.get(index)),
            _ => None,
        });
    match name {
        Some("threadgroup_position_in_grid") => Uniformity::Threadgroup,
        Some("subgroup_index_in_threadgroup") => Uniformity::Subgroup,
        Some(
            "threads_per_threadgroup"
            | "threadgroups_per_grid"
            | "subgroups_per_threadgroup"
            | "subgroup_size",
        ) => Uniformity::Dispatch,
        _ => Uniformity::Divergent,
    }
}

fn required_uniformity(
    instruction: &tvisa_format::BinaryInstruction,
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
    body: &BinaryFunctionBody,
    variants: &VariantIndex,
    visible: &HashMap<u64, Vec<u64>>,
    gpu_body_indexes: &HashSet<u64>,
) -> Vec<CallSite> {
    body.blocks
        .iter()
        .enumerate()
        .flat_map(|(block, basic_block)| {
            basic_block
                .instructions
                .iter()
                .filter_map(move |instruction| {
                    let targets = match instruction.opcode {
                        0x1a0 => direct_targets(instruction, variants)
                            .into_iter()
                            .filter(|target| gpu_body_indexes.contains(target))
                            .collect(),
                        0x1e7 if (instruction.modifiers >> 48) & 0xf == 0 => {
                            direct_targets(instruction, variants)
                                .into_iter()
                                .filter(|target| gpu_body_indexes.contains(target))
                                .collect()
                        }
                        0x404 => visible_call_targets(instruction, visible, gpu_body_indexes),
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

fn direct_targets(
    instruction: &tvisa_format::BinaryInstruction,
    variants: &VariantIndex,
) -> Vec<u64> {
    let Some(symbol) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol(symbol) => Some(*symbol),
            _ => None,
        })
    else {
        return Vec::new();
    };
    let index = symbol >> 2;
    match symbol & 3 {
        0 => vec![index],
        2 => usize::try_from(index)
            .ok()
            .and_then(|index| variants.sets.get(index))
            .map(|set| {
                set.members
                    .iter()
                    .map(|member| member.function_index)
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn visible_call_targets(
    instruction: &tvisa_format::BinaryInstruction,
    visible: &HashMap<u64, Vec<u64>>,
    gpu_body_indexes: &HashSet<u64>,
) -> Vec<u64> {
    if instruction.opcode != 0x404 {
        return Vec::new();
    }
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(signature) => visible.get(signature),
            _ => None,
        })
        .into_iter()
        .flatten()
        .copied()
        .filter(|target| gpu_body_indexes.contains(target))
        .collect()
}

fn call_summaries(
    functions: &[FunctionInfo],
    function_by_index: &HashMap<u64, usize>,
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
                        function_by_index
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
    function_by_index: &HashMap<u64, usize>,
    summaries: &[Option<Uniformity>],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for function in functions {
        for call in &function.calls {
            let requirement = call
                .targets
                .iter()
                .filter_map(|target| {
                    function_by_index
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
                VFY22,
                format!(
                    "GPU call requires {required:?}-uniform control flow in function @{} at CODE offset {}",
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

fn last_index(operands: &[BinaryOperand]) -> Option<u64> {
    operands.iter().rev().find_map(|operand| match operand {
        BinaryOperand::Index(index) => Some(*index),
        _ => None,
    })
}
