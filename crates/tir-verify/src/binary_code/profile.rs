use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryCodeTable, BinaryEngine, BinaryFunctionBody, BinaryGenericTable, BinaryGlobalTable,
    BinaryGpuEntry, BinaryGpuTable, BinaryImport, BinaryOperand, BinarySignature,
    BinarySignatureTable, BinaryTarget, BinaryType, BinaryTypeTable, StringTable,
};
use tir_target::Profile;

mod gpu_capabilities;
mod gpu_memory;
mod register_only;
mod vft_targets;
mod witness_targets;

const V13: Code = Code::new(Dialect::Tir, Area::Verifier, 13);
const V9: Code = Code::new(Dialect::Tir, Area::Verifier, 9);
const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);
const MAX_STACK_ALLOCATION_COUNT: u64 = 1 << 62;

#[derive(Clone, Copy)]
struct CallEdge {
    target: usize,
    offset: u64,
}

pub(super) fn verify(
    code: &BinaryCodeTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
    gpu: Option<&BinaryGpuTable>,
    module_profile: Profile,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for signature in &signatures.records {
        if is_gpu_target(signature.target) && !signature.generic_parameters.is_empty() {
            diagnostics.push(Diagnostic::error(
                V15,
                format!(
                    "GPU function @{} must be fully specialized before GPU lowering",
                    signature.name
                ),
            ));
        }
        if matches!(module_profile, Profile::Driver | Profile::Kernel)
            && matches!(
                signature.engine,
                Some(BinaryEngine::Runtime | BinaryEngine::Any)
            )
        {
            diagnostics.push(Diagnostic::error(
                V13,
                format!(
                    "function @{} uses an engine forbidden by the {:?} profile",
                    signature.name, module_profile
                ),
            ));
        }
        if module_profile == Profile::Kernel {
            if signature.entry_kind.is_some() {
                diagnostics.push(Diagnostic::error(
                    V13,
                    format!(
                        "GPU entry @{} is forbidden in the kernel profile",
                        signature.name
                    ),
                ));
            }
            if signature.modifiers & (1 << 42) == 0 {
                diagnostics.push(Diagnostic::error(
                    V13,
                    format!(
                        "kernel function @{} must be marked nounwind",
                        signature.name
                    ),
                ));
            }
            if !signature.kernel_fp
                && (function_has_float_or_vector(types, signature.function_type)
                    || signature
                        .instance_types
                        .iter()
                        .any(|type_index| type_has_float_or_vector(types, *type_index)))
            {
                diagnostics.push(Diagnostic::error(
                    V13,
                    format!(
                        "function @{} uses floating-point or vector types without kernel.fp",
                        signature.name
                    ),
                ));
            }
        } else if signature.kernel_fp {
            diagnostics.push(Diagnostic::error(
                V13,
                format!(
                    "kernel.fp is only permitted in the kernel profile (@{})",
                    signature.name
                ),
            ));
        }
    }

    let mut graph = vec![Vec::new(); signatures.records.len()];
    let mut bodies = vec![None; signatures.records.len()];
    for body in &code.functions {
        let Some(caller_index) = usize::try_from(body.function_index).ok() else {
            continue;
        };
        let Some(caller) = signatures.records.get(caller_index) else {
            continue;
        };
        bodies[caller_index] = Some(body);
        verify_profile_body(
            body,
            caller,
            signatures,
            types,
            module_profile,
            &mut diagnostics,
        );
        if matches!(
            caller.engine,
            Some(BinaryEngine::Runtime | BinaryEngine::Any)
        ) {
            for instruction in body
                .blocks
                .iter()
                .flat_map(|block| block.instructions.iter().flatten())
                .filter(|instruction| matches!(instruction.opcode, 0x220..=0x222 | 0x270..=0x273))
            {
                diagnostics.push(Diagnostic::error(
                    V9,
                    format!(
                        "function @{} CODE offset {}: runtime and any engines forbid assembly, target intrinsics, and device I/O",
                        caller.name, instruction.offset
                    ),
                ));
            }
        }
        if !is_gpu_target(caller.target) {
            continue;
        }
        let value_types = super::infer_value_types(body, signatures, types);
        for instruction in body
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter().flatten())
        {
            if let Some(message) = forbidden_instruction(instruction) {
                diagnostics.push(Diagnostic::error(
                    V15,
                    format!(
                        "function @{} CODE offset {}: {message}",
                        caller.name, instruction.offset
                    ),
                ));
            }
            for callee_index in vft_targets::resolve(instruction, &value_types, signatures, types) {
                graph[caller_index].push(CallEdge {
                    target: callee_index,
                    offset: instruction.offset,
                });
            }
            if let Some(resolution) = witness_targets::resolve_gpu_call(
                instruction,
                generics,
                strings,
                signatures,
                &caller.name,
            ) {
                match resolution {
                    Ok(callee_index) => graph[caller_index].push(CallEdge {
                        target: callee_index,
                        offset: instruction.offset,
                    }),
                    Err(diagnostic) => diagnostics.push(diagnostic),
                }
            }
            let Some(target) = direct_target(instruction) else {
                continue;
            };
            let Some(callee_index) = usize::try_from(target).ok() else {
                continue;
            };
            let Some(callee) = signatures.records.get(callee_index) else {
                continue;
            };
            if !is_gpu_target(callee.target) {
                diagnostics.push(Diagnostic::error(
                    V15,
                    format!(
                        "function @{} CODE offset {}: GPU code directly calls CPU-only function @{}",
                        caller.name, instruction.offset, callee.name
                    ),
                ));
                continue;
            }
            graph[caller_index].push(CallEdge {
                target: callee_index,
                offset: instruction.offset,
            });
        }
    }

    verify_acyclic(&graph, signatures, &mut diagnostics);
    register_only::verify(&bodies, signatures, types, &mut diagnostics);
    gpu_memory::verify(
        code,
        signatures,
        types,
        imports,
        globals,
        gpu,
        &mut diagnostics,
    );
    if let Some(gpu) = gpu {
        verify_vft_stages(gpu, &graph, &bodies, signatures, &mut diagnostics);
        gpu_capabilities::verify(
            gpu,
            &graph,
            &bodies,
            signatures,
            types,
            strings,
            &mut diagnostics,
        );
    }
    diagnostics
}

fn verify_profile_body(
    body: &BinaryFunctionBody,
    signature: &BinarySignature,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    profile: Profile,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let fully_decoded = body.blocks.iter().all(|block| block.instructions.is_some());
    let value_types = fully_decoded.then(|| super::infer_value_types(body, signatures, types));
    let static_integers = if profile == Profile::Kernel && fully_decoded {
        kernel_static_integers(body)
    } else {
        HashMap::new()
    };
    let mut value_cursor = 0usize;
    for block in &body.blocks {
        for parameter in &block.parameters {
            if profile == Profile::Kernel
                && !signature.kernel_fp
                && type_has_float_or_vector(types, parameter.ty)
            {
                diagnostics.push(Diagnostic::error(
                    V13,
                    format!(
                        "function @{} has a floating-point or vector block parameter without kernel.fp",
                        signature.name
                    ),
                ));
            }
            value_cursor = value_cursor.saturating_add(1);
        }
        let Some(instructions) = &block.instructions else {
            continue;
        };
        for instruction in instructions {
            let opcode = instruction.opcode;
            let profile_error = match profile {
                Profile::App | Profile::System if (0x270..=0x272).contains(&opcode) => {
                    Some("device I/O is forbidden in the app and system profiles")
                }
                Profile::Kernel if matches!(opcode, 0x1aa | 0x1d0..=0x1d4) => {
                    Some("unwinding instructions are forbidden in the kernel profile")
                }
                _ if opcode == 0x273 && profile != Profile::Kernel => {
                    Some("kernel instructions are only permitted in the kernel profile")
                }
                _ => None,
            };
            if let Some(message) = profile_error {
                diagnostics.push(Diagnostic::error(
                    V13,
                    format!(
                        "function @{} CODE offset {}: {message}",
                        signature.name, instruction.offset
                    ),
                ));
            }

            if profile == Profile::Kernel
                && !signature.kernel_fp
                && fully_decoded
                && (instruction_uses_float_or_vector(
                    instruction,
                    value_types.as_deref().unwrap_or_default(),
                    types,
                ) || inferred_results_use_float_or_vector(
                    instruction,
                    value_cursor,
                    value_types.as_deref().unwrap_or_default(),
                    types,
                ))
            {
                diagnostics.push(Diagnostic::error(
                    V13,
                    format!(
                        "function @{} CODE offset {} uses floating-point or vector data without kernel.fp",
                        signature.name, instruction.offset
                    ),
                ));
            }

            if profile == Profile::Kernel && opcode == 0x120 && fully_decoded {
                let dynamic_count = instruction
                    .operands
                    .iter()
                    .find_map(|operand| match operand {
                        BinaryOperand::OptionalValue(value) => *value,
                        _ => None,
                    });
                if let Some(value) = dynamic_count {
                    match static_integers.get(&value) {
                        None => diagnostics.push(Diagnostic::error(
                            V13,
                            format!(
                                "function @{} CODE offset {} has a dynamic kernel stack.alloc size",
                                signature.name, instruction.offset
                            ),
                        )),
                        Some(Some(count)) if *count < MAX_STACK_ALLOCATION_COUNT => {}
                        Some(_) => diagnostics.push(Diagnostic::error(
                            V13,
                            format!(
                                "function @{} CODE offset {} has an invalid kernel stack.alloc count",
                                signature.name, instruction.offset
                            ),
                        )),
                    }
                }
            }
            value_cursor = value_cursor.saturating_add(instruction.result_count as usize);
        }
    }
}

fn function_has_float_or_vector(types: &BinaryTypeTable, function_type: u64) -> bool {
    let Some(BinaryType::Function(function)) = usize::try_from(function_type)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return false;
    };
    function
        .parameters
        .iter()
        .any(|parameter| type_has_float_or_vector(types, parameter.ty))
        || type_has_float_or_vector(types, function.result)
        || function
            .error
            .is_some_and(|type_index| type_has_float_or_vector(types, type_index))
}

fn type_has_float_or_vector(types: &BinaryTypeTable, type_index: u64) -> bool {
    binary_type_has_float_or_vector(types, type_index, &mut Vec::new())
}

fn binary_type_has_float_or_vector(
    types: &BinaryTypeTable,
    type_index: u64,
    seen: &mut Vec<u64>,
) -> bool {
    if seen.contains(&type_index) {
        return false;
    }
    seen.push(type_index);
    let Some(ty) = usize::try_from(type_index)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return false;
    };
    match ty {
        BinaryType::Float(_)
        | BinaryType::FixedVector { .. }
        | BinaryType::ScalableVector { .. } => true,
        BinaryType::Tuple(elements) => elements
            .iter()
            .any(|element| binary_type_has_float_or_vector(types, *element, seen)),
        BinaryType::Address { pointee, .. }
        | BinaryType::Array {
            element: pointee, ..
        }
        | BinaryType::Reference { pointee }
        | BinaryType::Unique { pointee }
        | BinaryType::Weak { pointee }
        | BinaryType::Tensor {
            element: pointee, ..
        }
        | BinaryType::CooperativeMatrix {
            element: pointee, ..
        }
        | BinaryType::Texture {
            component: pointee, ..
        } => binary_type_has_float_or_vector(types, *pointee, seen),
        BinaryType::Nominal { arguments, .. } => arguments
            .iter()
            .any(|argument| binary_type_has_float_or_vector(types, *argument, seen)),
        BinaryType::Function(function) => {
            function
                .parameters
                .iter()
                .any(|parameter| binary_type_has_float_or_vector(types, parameter.ty, seen))
                || binary_type_has_float_or_vector(types, function.result, seen)
                || function
                    .error
                    .is_some_and(|error| binary_type_has_float_or_vector(types, error, seen))
        }
        BinaryType::Closure { function_type, .. }
        | BinaryType::VisibleFunctionTable { function_type } => {
            binary_type_has_float_or_vector(types, *function_type, seen)
        }
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            binary_type_has_float_or_vector(types, *yield_type, seen)
                || binary_type_has_float_or_vector(types, *resume_type, seen)
                || binary_type_has_float_or_vector(types, *result_type, seen)
        }
        BinaryType::Entry {
            root, immediate, ..
        } => {
            binary_type_has_float_or_vector(types, *root, seen)
                || binary_type_has_float_or_vector(types, *immediate, seen)
        }
        _ => false,
    }
}

fn instruction_uses_float_or_vector(
    instruction: &tir_format::BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> bool {
    if matches!(
        instruction.opcode,
        0x001 | 0x060..=0x092 | 0x0a3..=0x0a8 | 0x0ac | 0x0c0..=0x0d7
    ) {
        return true;
    }
    instruction.operands.iter().any(|operand| match operand {
        BinaryOperand::Type(type_index) => type_has_float_or_vector(types, *type_index),
        BinaryOperand::Types(type_indices) => type_indices
            .iter()
            .any(|type_index| type_has_float_or_vector(types, *type_index)),
        BinaryOperand::Value(value) => value_has_float_or_vector(*value, value_types, types),
        BinaryOperand::Values(values) => values
            .iter()
            .any(|value| value_has_float_or_vector(*value, value_types, types)),
        _ => false,
    })
}

fn inferred_results_use_float_or_vector(
    instruction: &tir_format::BinaryInstruction,
    first_result: usize,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> bool {
    (0..instruction.result_count).any(|offset| {
        first_result
            .checked_add(usize::try_from(offset).unwrap_or(usize::MAX))
            .and_then(|index| value_types.get(index))
            .copied()
            .flatten()
            .is_some_and(|type_index| type_has_float_or_vector(types, type_index))
    })
}

fn value_has_float_or_vector(
    value: u64,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> bool {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
        .is_some_and(|type_index| type_has_float_or_vector(types, type_index))
}

fn kernel_static_integers(body: &BinaryFunctionBody) -> HashMap<u64, Option<u64>> {
    let mut result = HashMap::new();
    let mut next_value = 0u64;
    for block in &body.blocks {
        next_value = next_value.saturating_add(block.parameters.len() as u64);
        if let Some(instructions) = &block.instructions {
            for instruction in instructions {
                if instruction.opcode == 0x000 && instruction.result_count == 1 {
                    result.insert(next_value, integer_constant(instruction));
                }
                next_value = next_value.saturating_add(instruction.result_count);
            }
        }
    }
    result
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

fn verify_acyclic(
    graph: &[Vec<CallEdge>],
    signatures: &BinarySignatureTable,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut state = vec![0_u8; graph.len()];
    let mut reported = HashSet::new();
    for start in 0..graph.len() {
        if state[start] != 0 {
            continue;
        }
        state[start] = 1;
        let mut stack = vec![(start, 0_usize)];
        while let Some((caller_index, edge_index)) = stack.last().copied() {
            let Some(edge) = graph[caller_index].get(edge_index).copied() else {
                state[caller_index] = 2;
                stack.pop();
                continue;
            };
            stack.last_mut().expect("active graph node").1 += 1;
            match state[edge.target] {
                0 => {
                    state[edge.target] = 1;
                    stack.push((edge.target, 0));
                }
                1 if reported.insert((caller_index, edge.offset)) => {
                    let caller = &signatures.records[caller_index].name;
                    let callee = &signatures.records[edge.target].name;
                    diagnostics.push(Diagnostic::error(
                        V15,
                        format!(
                            "function @{caller} CODE offset {}: GPU call graph contains a cycle through @{callee}",
                            edge.offset
                        ),
                    ));
                }
                _ => {}
            }
        }
    }
}

fn verify_vft_stages(
    gpu: &BinaryGpuTable,
    graph: &[Vec<CallEdge>],
    bodies: &[Option<&BinaryFunctionBody>],
    signatures: &BinarySignatureTable,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for entry in &gpu.entries {
        if vft_allowed(entry) {
            continue;
        }
        let Ok(start) = usize::try_from(entry.function_index) else {
            continue;
        };
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
            let Some(body) = bodies.get(function_index).copied().flatten() else {
                continue;
            };
            let name = &signatures.records[function_index].name;
            for instruction in body
                .blocks
                .iter()
                .flat_map(|block| block.instructions.iter().flatten())
                .filter(|instruction| instruction.opcode == 0x404)
            {
                diagnostics.push(Diagnostic::error(
                    V15,
                    format!(
                        "function @{name} CODE offset {}: gpu.call.vft is not permitted in the {:?} stage",
                        instruction.offset, entry.kind
                    ),
                ));
            }
            pending.extend(graph[function_index].iter().map(|edge| edge.target));
        }
    }
}

fn vft_allowed(entry: &BinaryGpuEntry) -> bool {
    matches!(
        entry.kind,
        tir_format::BinaryEntryKind::Compute
            | tir_format::BinaryEntryKind::Raygen
            | tir_format::BinaryEntryKind::Intersection
            | tir_format::BinaryEntryKind::Anyhit
            | tir_format::BinaryEntryKind::Closesthit
            | tir_format::BinaryEntryKind::Miss
            | tir_format::BinaryEntryKind::Callable
    )
}

fn direct_target(instruction: &tir_format::BinaryInstruction) -> Option<u64> {
    if !matches!(instruction.opcode, 0x1a0 | 0x1a7 | 0x1aa | 0x1e7) {
        return None;
    }
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => Some(*index),
            _ => None,
        })
}

fn forbidden_instruction(instruction: &tir_format::BinaryInstruction) -> Option<&'static str> {
    match instruction.opcode {
        0x160..=0x16a => Some("heap and reference instructions are forbidden in GPU code"),
        0x1a1 | 0x1a2..=0x1a4 | 0x1a6 => {
            Some("indirect, closure, and vtable calls are forbidden in GPU code")
        }
        0x1a7 | 0x1aa | 0x1d0..=0x1d4 => Some("unwinding instructions are forbidden in GPU code"),
        0x1a8 => Some("foreign calls are forbidden in GPU code"),
        0x1a9 | 0x210..=0x213 => Some("variadic instructions are forbidden in GPU code"),
        0x214 | 0x215 => Some("setjmp and longjmp are forbidden in GPU code"),
        0x205 | 0x206 => Some("atomic.wait and atomic.notify are forbidden in GPU code"),
        0x1c0..=0x1c9 => Some("coroutine and async instructions are forbidden in GPU code"),
        0x220 | 0x221 => Some("assembly instructions are forbidden in GPU code"),
        0x222 => Some("target intrinsics are forbidden in GPU code"),
        0x230
            if !matches!(
                instruction
                    .operands
                    .iter()
                    .find_map(|operand| match operand {
                        BinaryOperand::Index(index) => Some(*index),
                        _ => None,
                    }),
                Some(15 | 16)
            ) =>
        {
            Some("only ct.select and ct.eq portable intrinsics are allowed in GPU code")
        }
        _ => None,
    }
}

fn is_gpu_target(target: BinaryTarget) -> bool {
    matches!(target, BinaryTarget::Gpu | BinaryTarget::Both)
}
