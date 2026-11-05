use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::StringTable;
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, GpuEntryKind, GpuEntryRecord, TvisaImage,
    VariantIndex,
};

const VFY22: Code = Code::new(Dialect::Tvisa, Area::Verifier, 22);

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
    let bodies: HashMap<_, _> = bodies
        .iter()
        .map(|body| (body.header.function_index, body))
        .collect();
    let visible_targets = visible_targets(&entries, &functions, imports.functions);
    let mut diagnostics = Vec::new();
    verify_call_graph_acyclic(
        &bodies,
        &functions,
        imports.functions,
        &variants,
        &visible_targets,
        &mut diagnostics,
    );
    for entry in &entries {
        verify_entry(
            entry,
            &bodies,
            imports.functions,
            &visible_targets,
            &variants,
            &strings,
            &mut diagnostics,
        );
    }
    diagnostics
}

fn verify_entry(
    entry: &GpuEntryRecord,
    bodies: &HashMap<u64, &BinaryFunctionBody>,
    import_count: u64,
    visible_targets: &HashMap<u64, Vec<u64>>,
    variants: &VariantIndex,
    strings: &StringTable,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(stage) = stage_name(entry.kind) else {
        return;
    };
    let mut pending = vec![entry.function_index];
    let mut visited = HashSet::new();
    let mut required = HashSet::new();
    let mut specializations = HashMap::new();
    let mut conflicting_specializations = HashSet::new();
    while let Some(function) = pending.pop() {
        if !visited.insert(function) {
            continue;
        }
        let Some(body) = bodies.get(&function) else {
            continue;
        };
        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            if instruction.opcode == 0x401 {
                collect_specialization(
                    instruction,
                    &mut specializations,
                    &mut conflicting_specializations,
                );
            }
            verify_stage_instruction(instruction, stage, strings, &mut required, diagnostics);
            let mut callees = direct_callees(instruction, variants);
            callees.extend(vft_callees(instruction, visible_targets));
            pending.extend(
                callees
                    .into_iter()
                    .filter(|callee| *callee >= import_count && bodies.contains_key(callee)),
            );
        }
    }
    for (bit, name) in required {
        if entry.capabilities & (1 << bit) == 0 {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!("GPU {stage} entry lacks the '{name}' capability used by its code"),
            ));
        }
    }
    for id in conflicting_specializations {
        diagnostics.push(Diagnostic::error(
            VFY22,
            format!(
                "GPU {stage} entry has conflicting declarations for specialization constant {id}"
            ),
        ));
    }
    let mut recorded = HashMap::new();
    for constant in &entry.specialization_constants {
        if recorded
            .insert(
                constant.id,
                (constant.type_index, constant.default_constant),
            )
            .is_some()
        {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "GPU {stage} entry repeats specialization constant {}",
                    constant.id
                ),
            ));
        }
    }
    for (id, constant) in &specializations {
        let recorded_constant = u32::try_from(*id).ok().and_then(|id| recorded.get(&id));
        if recorded_constant != Some(constant) {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "GPU {stage} entry does not match specialization constant {id} used by its code"
                ),
            ));
        }
    }
    for id in recorded.keys() {
        if !specializations.contains_key(&u64::from(*id)) {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!("GPU {stage} entry records unused specialization constant {id}"),
            ));
        }
    }
}

fn collect_specialization(
    instruction: &BinaryInstruction,
    specializations: &mut HashMap<u64, (u64, u64)>,
    conflicts: &mut HashSet<u64>,
) {
    let mut indexes = instruction
        .operands
        .iter()
        .filter_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        });
    let Some(id) = indexes.next() else {
        return;
    };
    let Some(default) = indexes.next() else {
        return;
    };
    let type_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        });
    let Some(type_index) = type_index else {
        return;
    };
    if let Some(previous) = specializations.insert(id, (type_index, default))
        && previous != (type_index, default)
    {
        conflicts.insert(id);
    }
}

fn verify_stage_instruction(
    instruction: &BinaryInstruction,
    stage: &str,
    strings: &StringTable,
    required: &mut HashSet<(u8, &'static str)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let required_stage = match instruction.opcode {
        0x405..=0x409 => Some("fragment"),
        0x507 => Some("intersection"),
        0x508 | 0x509 => Some("anyhit"),
        0x4c0 => Some("object"),
        0x4c1..=0x4c4 => Some("mesh"),
        0x5a0 => Some("tile"),
        0x5c0..=0x5c5 => Some("node"),
        _ => None,
    };
    if required_stage.is_some_and(|required| required != stage) {
        diagnostics.push(Diagnostic::error(
            VFY22,
            format!(
                "GPU opcode 0x{:03X} requires the {} stage, not {stage}",
                instruction.opcode,
                required_stage.unwrap_or("unknown")
            ),
        ));
    }
    if is_implicit_derivative(instruction.opcode) && !matches!(stage, "fragment" | "compute") {
        diagnostics.push(Diagnostic::error(
            VFY22,
            format!(
                "GPU opcode 0x{:03X} with implicit derivatives requires the fragment or compute stage, not {stage}",
                instruction.opcode
            ),
        ));
    }
    if instruction.opcode == 0x400 {
        let name = instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Index(index) => usize::try_from(*index)
                    .ok()
                    .and_then(|index| strings.get(index)),
                _ => None,
            });
        if let Some(name) = name
            && crate::gpu::builtin_stages(name).is_some_and(|stages| !stages.contains(&stage))
        {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!("GPU builtin '{name}' is not available in {stage} entries"),
            ));
        }
        if name == Some("shading_rate") {
            required.insert((17, "shading_rate"));
        }
    }
    if instruction.opcode == 0x404
        && !matches!(
            stage,
            "compute" | "raygen" | "intersection" | "anyhit" | "closesthit" | "miss" | "callable"
        )
    {
        diagnostics.push(Diagnostic::error(
            VFY22,
            format!("gpu.call.vft is not available in {stage} entries"),
        ));
    }

    let capability = match instruction.opcode {
        0x40a => Some((18, "printf")),
        0x406 => Some((4, "framebuffer_fetch")),
        0x407 | 0x408 => Some((5, "raster_order")),
        0x409 => Some((3, "tile")),
        0x5a0 => Some((3, "tile")),
        0x420..=0x422 if stage == "compute" => Some((11, "compute_derivatives")),
        opcode if is_implicit_derivative(opcode) && stage == "compute" => {
            Some((11, "compute_derivatives"))
        }
        0x568 => Some((10, "coop.reduce")),
        0x5c0..=0x5c5 => Some((6, "work_graphs")),
        _ => None,
    };
    if let Some(capability) = capability {
        required.insert(capability);
    }
    if instruction.opcode == 0x48d {
        required.insert((14, "sampler_feedback"));
    }
    if instruction.opcode == 0x50b
        && instruction
            .operands
            .iter()
            .any(|operand| matches!(operand, BinaryOperand::Index(attribute) if *attribute == 16))
    {
        required.insert((16, "triangle_vertex_positions"));
    }
    if matches!(instruction.opcode, 0x480..=0x488 | 0x48d) && instruction.modifiers & (1 << 35) != 0
    {
        required.insert((13, "sparse_residency"));
    }
}

fn is_implicit_derivative(opcode: u64) -> bool {
    matches!(
        opcode,
        0x480 | 0x483 | 0x484 | 0x486 | 0x487 | 0x48c | 0x48d
    )
}

fn direct_callees(instruction: &BinaryInstruction, variants: &VariantIndex) -> Vec<u64> {
    let direct = matches!(instruction.opcode, 0x1a0 | 0x1a9)
        || matches!(instruction.opcode, 0x1aa | 0x1e7) && (instruction.modifiers >> 48) & 0xf == 0;
    if !direct {
        return Vec::new();
    }
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
        let Some(function_index) = entry.function_index.checked_sub(import_count) else {
            continue;
        };
        let Some(function) = usize::try_from(function_index)
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

fn vft_callees(
    instruction: &BinaryInstruction,
    visible_targets: &HashMap<u64, Vec<u64>>,
) -> Vec<u64> {
    if instruction.opcode != 0x404 {
        return Vec::new();
    }
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(signature) => visible_targets.get(signature),
            _ => None,
        })
        .cloned()
        .unwrap_or_default()
}

fn verify_call_graph_acyclic(
    bodies: &HashMap<u64, &BinaryFunctionBody>,
    functions: &[tvisa_format::FunctionRecordSummary],
    import_count: u64,
    variants: &VariantIndex,
    visible_targets: &HashMap<u64, Vec<u64>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let gpu_functions = functions
        .iter()
        .enumerate()
        .filter(|(_, function)| function.target != 0)
        .map(|(index, _)| import_count + index as u64)
        .collect::<Vec<_>>();
    let gpu_set = gpu_functions.iter().copied().collect::<HashSet<_>>();
    let mut graph = HashMap::<u64, Vec<(u64, u64)>>::new();
    for function in &gpu_functions {
        let Some(body) = bodies.get(function) else {
            continue;
        };
        let edges = graph.entry(*function).or_default();
        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            let mut targets = direct_callees(instruction, variants);
            targets.extend(vft_callees(instruction, visible_targets));
            edges.extend(
                targets
                    .into_iter()
                    .filter(|target| gpu_set.contains(target) && bodies.contains_key(target))
                    .map(|target| (target, instruction.offset)),
            );
        }
    }
    let mut states = HashMap::<u64, u8>::new();
    let mut reported = HashSet::new();
    for start in gpu_functions {
        if states.get(&start).copied().unwrap_or(0) != 0 || !bodies.contains_key(&start) {
            continue;
        }
        states.insert(start, 1);
        let mut stack = vec![(start, 0usize)];
        while let Some((caller, edge_index)) = stack.last().copied() {
            let Some(edge) = graph
                .get(&caller)
                .and_then(|edges| edges.get(edge_index))
                .copied()
            else {
                states.insert(caller, 2);
                stack.pop();
                continue;
            };
            stack.last_mut().expect("active GPU call graph node").1 += 1;
            match states.get(&edge.0).copied().unwrap_or(0) {
                0 => {
                    states.insert(edge.0, 1);
                    stack.push((edge.0, 0));
                }
                1 if reported.insert((caller, edge.1)) => {
                    let caller_name = function_name(caller, import_count, functions);
                    let target_name = function_name(edge.0, import_count, functions);
                    diagnostics.push(Diagnostic::error(
                        VFY22,
                        format!(
                            "GPU function @{caller_name} CODE offset {} participates in a call-graph cycle through @{target_name}",
                            edge.1
                        ),
                    ));
                }
                _ => {}
            }
        }
    }
}

fn function_name(
    function_index: u64,
    import_count: u64,
    functions: &[tvisa_format::FunctionRecordSummary],
) -> &str {
    function_index
        .checked_sub(import_count)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| functions.get(index))
        .map_or("<unknown>", |function| function.name.as_str())
}

fn stage_name(kind: GpuEntryKind) -> Option<&'static str> {
    Some(match kind {
        GpuEntryKind::Compute => "compute",
        GpuEntryKind::Object => "object",
        GpuEntryKind::Mesh => "mesh",
        GpuEntryKind::Vertex => "vertex",
        GpuEntryKind::Fragment => "fragment",
        GpuEntryKind::Tile => "tile",
        GpuEntryKind::Raygen => "raygen",
        GpuEntryKind::Intersection => "intersection",
        GpuEntryKind::Anyhit => "anyhit",
        GpuEntryKind::Closesthit => "closesthit",
        GpuEntryKind::Miss => "miss",
        GpuEntryKind::Callable => "callable",
        GpuEntryKind::Node => "node",
        GpuEntryKind::Visible => return None,
    })
}
