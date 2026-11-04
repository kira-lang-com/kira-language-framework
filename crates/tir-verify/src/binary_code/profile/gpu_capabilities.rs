use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryConstantTable, BinaryEntryKind, BinaryFpMode, BinaryGpuEntry, BinaryGpuNodeOutputKind,
    BinaryGpuTable, BinaryImport, BinaryOperand, BinarySignatureTable, BinaryType, BinaryTypeTable,
    StringTable,
};
use tir_types::FloatType;

use super::CallEdge;

mod atomic_width;
mod builtin_rules;
mod specialization;
mod type_rules;

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);
const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const CAPABILITY_NAMES: [&str; 19] = [
    "float64",
    "atomic128",
    "float16.atomics",
    "tile",
    "framebuffer_fetch",
    "raster_order",
    "work_graphs",
    "work_graphs.mesh",
    "coop.fp8",
    "coop.blockscaled",
    "coop.reduce",
    "compute_derivatives",
    "float32.denorm_preserve",
    "sparse_residency",
    "sampler_feedback",
    "subgroup_size_control",
    "triangle_vertex_positions",
    "shading_rate",
    "printf",
];

pub(super) fn verify(
    gpu: &BinaryGpuTable,
    graph: &[Vec<CallEdge>],
    bodies: &[Option<&tir_format::BinaryFunctionBody>],
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    strings: Option<&StringTable>,
    constants: Option<&BinaryConstantTable>,
    imports: Option<&[BinaryImport]>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for entry in &gpu.entries {
        let Ok(start) = usize::try_from(entry.function_index) else {
            continue;
        };
        let Some(signature) = signatures.records.get(start) else {
            continue;
        };
        let mut required = entry_requirements(entry);
        let mut checked_types = HashSet::new();
        type_rules::verify(
            types,
            signature.function_type,
            &signature.name,
            &mut checked_types,
            diagnostics,
        );
        let mut visited = vec![false; graph.len()];
        let mut pending = vec![start];
        while let Some(function_index) = pending.pop() {
            let Some(seen) = visited.get_mut(function_index) else {
                continue;
            };
            if *seen {
                continue;
            }
            *seen = true;
            let Some(function) = signatures.records.get(function_index) else {
                continue;
            };
            add_type(
                types,
                function.function_type,
                &mut Vec::new(),
                &mut required,
            );
            type_rules::verify(
                types,
                function.function_type,
                &signature.name,
                &mut checked_types,
                diagnostics,
            );
            for type_index in &function.instance_types {
                add_type(types, *type_index, &mut Vec::new(), &mut required);
                type_rules::verify(
                    types,
                    *type_index,
                    &signature.name,
                    &mut checked_types,
                    diagnostics,
                );
            }
            if function.fp_mode == Some(BinaryFpMode::IeeeDenormals) {
                required |= 1 << 12;
            }
            if let Some(body) = bodies.get(function_index).copied().flatten() {
                if entry.subgroup_size != 0 {
                    verify_subgroup_clusters(
                        body,
                        function,
                        signature,
                        entry.subgroup_size,
                        diagnostics,
                    );
                }
                add_body_requirements(
                    entry.kind,
                    body,
                    signatures,
                    types,
                    strings,
                    imports,
                    &signature.name,
                    &mut checked_types,
                    diagnostics,
                    &mut required,
                );
            }
            pending.extend(graph[function_index].iter().map(|edge| edge.target));
        }
        for interface in &entry.interfaces {
            add_type(types, interface.ty, &mut Vec::new(), &mut required);
            type_rules::verify(
                types,
                interface.ty,
                &signature.name,
                &mut checked_types,
                diagnostics,
            );
        }
        for constant in &entry.specialization_constants {
            add_type(types, constant.ty, &mut Vec::new(), &mut required);
            type_rules::verify(
                types,
                constant.ty,
                &signature.name,
                &mut checked_types,
                diagnostics,
            );
        }
        specialization::verify_entry(
            entry,
            &visited,
            bodies,
            signatures,
            types,
            constants,
            diagnostics,
        );
        let missing = required & !entry.capabilities;
        for (bit, name) in CAPABILITY_NAMES.iter().enumerate() {
            if missing & (1 << bit) != 0 {
                diagnostics.push(Diagnostic::error(
                    V15,
                    format!(
                        "GPU entry @{} uses undeclared capability '{name}'",
                        signature.name
                    ),
                ));
            }
            if entry.capabilities & (1 << bit) != 0 && required & (1 << bit) == 0 {
                diagnostics.push(Diagnostic::error(
                    V15,
                    format!(
                        "GPU entry @{} declares unused capability '{name}'",
                        signature.name
                    ),
                ));
            }
        }
    }
}

fn verify_subgroup_clusters(
    body: &tir_format::BinaryFunctionBody,
    function: &tir_format::BinarySignature,
    entry: &tir_format::BinarySignature,
    subgroup_size: u64,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
        .filter(|instruction| matches!(instruction.opcode, 0x450..=0x452))
    {
        let indices = instruction
            .operands
            .iter()
            .filter_map(|operand| match operand {
                BinaryOperand::Index(index) => Some(*index),
                _ => None,
            });
        let Some(cluster) = indices.skip(1).next() else {
            continue;
        };
        if cluster != 0 && cluster > subgroup_size {
            diagnostics.push(Diagnostic::error(
                V2,
                format!(
                    "GPU entry @{} selects subgroup_size {subgroup_size}, smaller than subgroup cluster {cluster} in @{} at CODE+0x{:x}",
                    entry.name, function.name, instruction.offset
                ),
            ));
        }
    }
}

fn entry_requirements(entry: &BinaryGpuEntry) -> u64 {
    let mut required = 0;
    if entry.kind == BinaryEntryKind::Tile {
        required |= 1 << 3;
    }
    if entry.kind == BinaryEntryKind::Node {
        required |= 1 << 6;
        if entry.node.as_ref().is_some_and(|node| {
            node.outputs
                .iter()
                .any(|output| output.kind == BinaryGpuNodeOutputKind::Mesh)
        }) {
            required |= 1 << 7;
        }
    }
    if entry.subgroup_size != 0 {
        required |= 1 << 15;
    }
    if entry.floating_point_mode == Some(BinaryFpMode::IeeeDenormals) {
        required |= 1 << 12;
    }
    required
}

fn add_body_requirements(
    entry_kind: BinaryEntryKind,
    body: &tir_format::BinaryFunctionBody,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    strings: Option<&StringTable>,
    imports: Option<&[BinaryImport]>,
    entry_name: &str,
    checked_types: &mut HashSet<u64>,
    diagnostics: &mut Vec<Diagnostic>,
    required: &mut u64,
) {
    let value_types = super::super::infer_value_types(body, signatures, types, imports);
    let function_name = usize::try_from(body.function_index)
        .ok()
        .and_then(|index| signatures.records.get(index))
        .map_or("<unknown>", |signature| signature.name.as_str());
    builtin_rules::verify(entry_kind, body, types, strings, function_name, diagnostics);
    for type_index in value_types.iter().flatten() {
        add_type(types, *type_index, &mut Vec::new(), required);
        type_rules::verify(types, *type_index, entry_name, checked_types, diagnostics);
    }
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
    {
        for operand in &instruction.operands {
            match operand {
                BinaryOperand::Type(type_index) => {
                    add_type(types, *type_index, &mut Vec::new(), required);
                    type_rules::verify(types, *type_index, entry_name, checked_types, diagnostics);
                }
                BinaryOperand::Types(type_indices) => {
                    for type_index in type_indices {
                        add_type(types, *type_index, &mut Vec::new(), required);
                        type_rules::verify(
                            types,
                            *type_index,
                            entry_name,
                            checked_types,
                            diagnostics,
                        );
                    }
                }
                _ => {}
            }
        }
        add_instruction_requirements(
            entry_kind,
            instruction,
            &value_types,
            types,
            strings,
            required,
        );
        if atomic_width::uses_atomic128(instruction, &value_types, types) {
            *required |= 1 << 1;
        }
    }
}

fn add_instruction_requirements(
    entry_kind: BinaryEntryKind,
    instruction: &tir_format::BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    strings: Option<&StringTable>,
    required: &mut u64,
) {
    match instruction.opcode {
        0x400 if builtin_is_shading_rate(instruction, strings) => *required |= 1 << 17,
        0x406 => *required |= 1 << 4,
        0x407 | 0x408 => *required |= 1 << 5,
        0x409 => *required |= 1 << 3,
        0x402 | 0x403
            if instruction.operands.iter().any(
                |operand| matches!(operand, BinaryOperand::Index(mask) if mask & (1 << 3) != 0),
            ) =>
        {
            *required |= 1 << 3;
        }
        0x40a => *required |= 1 << 18,
        0x48d => *required |= 1 << 14,
        0x568 => *required |= 1 << 10,
        0x50b
            if instruction
                .operands
                .iter()
                .any(|operand| matches!(operand, BinaryOperand::Index(16))) =>
        {
            *required |= 1 << 16;
        }
        _ => {}
    }
    if entry_kind == BinaryEntryKind::Compute
        && matches!(
            instruction.opcode,
            0x480 | 0x483 | 0x484 | 0x486 | 0x487 | 0x48c | 0x48d
        )
    {
        *required |= 1 << 11;
    }
    if instruction.modifiers & (1 << 35) != 0 {
        *required |= 1 << 13;
    }
    if matches!(instruction.opcode, 0x200..=0x203)
        && instruction_uses_float16(instruction, value_types, types)
    {
        *required |= 1 << 2;
    }
    if matches!(instruction.opcode, 0x560..=0x56b)
        && instruction_uses_float_kind(instruction, value_types, types, FloatType::E8M0)
    {
        *required |= 1 << 9;
    }
}

fn builtin_is_shading_rate(
    instruction: &tir_format::BinaryInstruction,
    strings: Option<&StringTable>,
) -> bool {
    let Some(strings) = strings else {
        return false;
    };
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => usize::try_from(*index)
                .ok()
                .and_then(|index| strings.get(index)),
            _ => None,
        })
        == Some("shading_rate")
}

fn instruction_uses_float16(
    instruction: &tir_format::BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> bool {
    instruction.operands.iter().any(|operand| match operand {
        BinaryOperand::Type(type_index) => is_float16(types, *type_index),
        BinaryOperand::Types(type_indices) => type_indices
            .iter()
            .any(|type_index| is_float16(types, *type_index)),
        BinaryOperand::Value(value) => {
            value_type(*value, value_types).is_some_and(|type_index| is_float16(types, type_index))
        }
        BinaryOperand::Values(values) => values.iter().any(|value| {
            value_type(*value, value_types).is_some_and(|type_index| is_float16(types, type_index))
        }),
        _ => false,
    })
}

fn is_float16(types: &BinaryTypeTable, type_index: u64) -> bool {
    matches!(
        types
            .entries
            .get(usize::try_from(type_index).unwrap_or(usize::MAX)),
        Some(BinaryType::Float(FloatType::F16))
    )
}

fn value_type(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn instruction_uses_float_kind(
    instruction: &tir_format::BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    kind: FloatType,
) -> bool {
    instruction.operands.iter().any(|operand| match operand {
        BinaryOperand::Type(type_index) => {
            contains_float(types, *type_index, kind, &mut Vec::new())
        }
        BinaryOperand::Types(type_indices) => type_indices
            .iter()
            .any(|type_index| contains_float(types, *type_index, kind, &mut Vec::new())),
        BinaryOperand::Value(value) => value_type(*value, value_types)
            .is_some_and(|index| contains_float(types, index, kind, &mut Vec::new())),
        BinaryOperand::Values(values) => values.iter().any(|value| {
            value_type(*value, value_types)
                .is_some_and(|index| contains_float(types, index, kind, &mut Vec::new()))
        }),
        _ => false,
    })
}

fn contains_float(
    types: &BinaryTypeTable,
    type_index: u64,
    kind: FloatType,
    seen: &mut Vec<u64>,
) -> bool {
    if seen.contains(&type_index) {
        return false;
    }
    seen.push(type_index);
    let Some(ty) = types
        .entries
        .get(usize::try_from(type_index).unwrap_or(usize::MAX))
    else {
        return false;
    };
    if matches!(ty, BinaryType::Float(actual) if *actual == kind) {
        return true;
    }
    let children = match ty {
        BinaryType::Address { pointee, .. }
        | BinaryType::FixedVector {
            element: pointee, ..
        }
        | BinaryType::ScalableVector { element: pointee }
        | BinaryType::Array {
            element: pointee, ..
        }
        | BinaryType::Reference { pointee }
        | BinaryType::Unique { pointee }
        | BinaryType::Weak { pointee }
        | BinaryType::Texture {
            component: pointee, ..
        }
        | BinaryType::Tensor {
            element: pointee, ..
        }
        | BinaryType::CooperativeMatrix {
            element: pointee, ..
        } => vec![*pointee],
        BinaryType::Tuple(elements) => elements.clone(),
        BinaryType::Nominal {
            declaration,
            arguments,
        } => {
            let mut children = arguments.clone();
            if let Some(definition) = types
                .definitions
                .iter()
                .find(|definition| definition.declaration == *declaration)
            {
                for field_type in definition.fields.iter().map(|field| field.ty).chain(
                    definition
                        .variants
                        .iter()
                        .flat_map(|variant| variant.payload.iter().copied()),
                ) {
                    if let Some(field_type) =
                        super::super::types::specialize_nominal_type(types, field_type, arguments)
                    {
                        children.push(field_type);
                    }
                }
            }
            children
        }
        BinaryType::Function(function) => function
            .parameters
            .iter()
            .map(|parameter| parameter.ty)
            .chain(std::iter::once(function.result))
            .chain(function.error)
            .collect(),
        BinaryType::Closure { function_type, .. }
        | BinaryType::VisibleFunctionTable { function_type } => vec![*function_type],
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => vec![*yield_type, *resume_type, *result_type],
        BinaryType::Entry {
            root, immediate, ..
        } => vec![*root, *immediate],
        _ => Vec::new(),
    };
    children
        .into_iter()
        .any(|child| contains_float(types, child, kind, seen))
}

fn add_type(types: &BinaryTypeTable, type_index: u64, seen: &mut Vec<u64>, required: &mut u64) {
    if seen.contains(&type_index) {
        return;
    }
    seen.push(type_index);
    let Some(ty) = types
        .entries
        .get(usize::try_from(type_index).unwrap_or(usize::MAX))
    else {
        return;
    };
    match ty {
        BinaryType::Float(FloatType::F64) => *required |= 1 << 0,
        BinaryType::Pointer(8) | BinaryType::Address { space: 8, .. } => *required |= 1 << 3,
        BinaryType::Pointer(9) | BinaryType::Address { space: 9, .. } => *required |= 1 << 6,
        BinaryType::CooperativeMatrix { element, .. } => {
            match types
                .entries
                .get(usize::try_from(*element).unwrap_or(usize::MAX))
            {
                Some(BinaryType::Float(FloatType::F8E4M3 | FloatType::F8E5M2)) => {
                    *required |= 1 << 8;
                }
                Some(BinaryType::Float(
                    FloatType::F4E2M1 | FloatType::F6E2M3 | FloatType::F6E3M2,
                )) => *required |= 1 << 9,
                _ => {}
            }
            add_type(types, *element, seen, required);
        }
        _ => {}
    }
    match ty {
        BinaryType::Address { pointee, .. }
        | BinaryType::FixedVector {
            element: pointee, ..
        }
        | BinaryType::ScalableVector { element: pointee }
        | BinaryType::Array {
            element: pointee, ..
        }
        | BinaryType::Reference { pointee }
        | BinaryType::Unique { pointee }
        | BinaryType::Weak { pointee }
        | BinaryType::Texture {
            component: pointee, ..
        }
        | BinaryType::Tensor {
            element: pointee, ..
        }
        | BinaryType::CooperativeMatrix {
            element: pointee, ..
        } => add_type(types, *pointee, seen, required),
        BinaryType::Tuple(elements) => {
            for element in elements {
                add_type(types, *element, seen, required);
            }
        }
        BinaryType::Nominal {
            declaration,
            arguments,
        } => {
            for argument in arguments {
                add_type(types, *argument, seen, required);
            }
            if let Some(definition) = types
                .definitions
                .iter()
                .find(|definition| definition.declaration == *declaration)
            {
                let fields = definition
                    .fields
                    .iter()
                    .map(|field| field.ty)
                    .chain(
                        definition
                            .variants
                            .iter()
                            .flat_map(|variant| variant.payload.iter().copied()),
                    )
                    .collect::<Vec<_>>();
                for field_type in fields {
                    if let Some(field_type) =
                        super::super::types::specialize_nominal_type(types, field_type, arguments)
                    {
                        add_type(types, field_type, seen, required);
                    }
                }
            }
        }
        BinaryType::Function(function) => {
            for parameter in &function.parameters {
                add_type(types, parameter.ty, seen, required);
            }
            add_type(types, function.result, seen, required);
            if let Some(error) = function.error {
                add_type(types, error, seen, required);
            }
        }
        BinaryType::Closure { function_type, .. }
        | BinaryType::VisibleFunctionTable { function_type } => {
            add_type(types, *function_type, seen, required);
        }
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            add_type(types, *yield_type, seen, required);
            add_type(types, *resume_type, seen, required);
            add_type(types, *result_type, seen, required);
        }
        BinaryType::Entry {
            kind,
            root,
            immediate,
            ..
        } => {
            if *kind == 2 {
                *required |= 1 << 7;
            }
            add_type(types, *root, seen, required);
            add_type(types, *immediate, seen, required);
        }
        _ => {}
    }
}
