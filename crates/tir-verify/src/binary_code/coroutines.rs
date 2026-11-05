use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryFunctionType, BinaryInstruction, BinaryOperand, BinarySignature,
    BinarySignatureTable, BinaryType, BinaryTypeTable,
};

#[path = "coroutines/v17.rs"]
mod v17;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify_body(
    body: &BinaryFunctionBody,
    caller: &BinarySignature,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    value_types: &[Option<u64>],
) -> Vec<Diagnostic> {
    let Some(caller_function) = function_type(types, caller.function_type) else {
        return Vec::new();
    };
    let mut diagnostics = Vec::new();
    for block in &body.blocks {
        let Some(instructions) = &block.instructions else {
            continue;
        };
        for instruction in instructions {
            match instruction.opcode {
                0x1c0 => verify_start(
                    instruction,
                    signatures,
                    types,
                    value_types,
                    &mut diagnostics,
                ),
                0x1a0 | 0x1a7 | 0x1a8 | 0x1a9 | 0x1aa | 0x1e7 => {
                    verify_coroutine_call(instruction, signatures, &mut diagnostics)
                }
                0x1c1 => verify_yield(
                    instruction,
                    body,
                    caller,
                    caller_function.asynchronous,
                    types,
                    value_types,
                    &mut diagnostics,
                ),
                0x1c2 => verify_resume(
                    instruction,
                    body,
                    &caller.name,
                    types,
                    value_types,
                    &mut diagnostics,
                ),
                0x1c4 | 0x1c5 => verify_coroutine_handle(
                    instruction,
                    &caller.name,
                    value_types,
                    types,
                    &mut diagnostics,
                ),
                0x1c6 | 0x1c7 => {
                    verify_frame_query(instruction, signatures, types, &mut diagnostics)
                }
                0x1c8 => verify_await(
                    instruction,
                    &caller.name,
                    caller_function,
                    signatures,
                    types,
                    value_types,
                    &mut diagnostics,
                ),
                0x1c9 if !caller_function.asynchronous => report(
                    &mut diagnostics,
                    &caller.name,
                    instruction,
                    "async.waker is used outside an async function",
                ),
                _ => {}
            }
        }
    }
    v17::verify_body(body, caller, caller_function.asynchronous, &mut diagnostics);
    diagnostics
}

fn verify_coroutine_call(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some((_, callee)) = direct_callee(instruction, signatures)
        && callee.coroutine.is_some()
    {
        report(
            diagnostics,
            &callee.name,
            instruction,
            "coroutine function must be started with coro.start",
        );
    }
}

fn verify_start(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    value_types: &[Option<u64>],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some((function_index, callee)) = direct_callee(instruction, signatures) else {
        return;
    };
    let Some(function) = function_type(types, callee.function_type) else {
        return;
    };
    if callee.coroutine.is_none() || function.asynchronous {
        report(
            diagnostics,
            &callee.name,
            instruction,
            "coro.start requires a non-async coroutine function",
        );
    }

    let arguments = value_ids(instruction);
    let inline = ((instruction.modifiers >> 48) & 0xf) == 1;
    let expected_count = function.parameters.len() + usize::from(inline);
    if arguments.len() != expected_count {
        report(
            diagnostics,
            &callee.name,
            instruction,
            format!(
                "coro.start passes {} values; expected {expected_count}",
                arguments.len()
            ),
        );
    }
    if callee.generic_parameters.is_empty() {
        for (index, (argument, parameter)) in arguments
            .iter()
            .take(function.parameters.len())
            .zip(&function.parameters)
            .enumerate()
        {
            if let Some(actual) = value_type_id(*argument, value_types)
                && actual != parameter.ty
            {
                report(
                    diagnostics,
                    &callee.name,
                    instruction,
                    format!(
                        "coro.start argument {} has type {actual}; expected {}",
                        index + 1,
                        parameter.ty
                    ),
                );
            }
        }
    }
    if inline
        && let Some(slot) = arguments.get(function.parameters.len())
        && let Some(actual) = value_type_id(*slot, value_types)
        && !matches!(
            type_at(types, actual),
            Some(BinaryType::Address { pointee, space: 0 })
                if matches!(type_at(types, *pointee), Some(BinaryType::Frame { function_index: frame }) if *frame == function_index)
        )
    {
        report(
            diagnostics,
            &callee.name,
            instruction,
            "inline coro.start storage must have type *frame for its target function",
        );
    }
}

fn verify_yield(
    instruction: &BinaryInstruction,
    body: &BinaryFunctionBody,
    caller: &BinarySignature,
    caller_is_async: bool,
    types: &BinaryTypeTable,
    value_types: &[Option<u64>],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some((yield_type, resume_type)) = caller.coroutine else {
        report(
            diagnostics,
            &caller.name,
            instruction,
            "coro.yield is used outside a coroutine function",
        );
        return;
    };
    if caller_is_async {
        report(
            diagnostics,
            &caller.name,
            instruction,
            "async functions cannot use coro.yield",
        );
    }
    if let Some(actual) = value_type(instruction, 0, value_types)
        && actual != yield_type
    {
        report(
            diagnostics,
            &caller.name,
            instruction,
            format!("coro.yield value has type {actual}; expected {yield_type}"),
        );
    }
    let targets = index_values(instruction);
    if let Some(target) = targets.first() {
        verify_implicit_edge(
            instruction,
            body,
            *target,
            &value_edge_types(types, resume_type),
            &caller.name,
            diagnostics,
        );
    }
    if let Some(target) = targets.get(1) {
        verify_implicit_edge(instruction, body, *target, &[], &caller.name, diagnostics);
    }
}

fn verify_resume(
    instruction: &BinaryInstruction,
    body: &BinaryFunctionBody,
    caller: &str,
    types: &BinaryTypeTable,
    value_types: &[Option<u64>],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(handle_type) = value_type(instruction, 0, value_types) else {
        return;
    };
    let Some(BinaryType::Coroutine {
        yield_type,
        resume_type,
        result_type,
    }) = type_at(types, handle_type)
    else {
        report(
            diagnostics,
            caller,
            instruction,
            "coro.resume first operand is not a coroutine handle",
        );
        return;
    };
    if let Some(actual) = value_type(instruction, 1, value_types)
        && actual != *resume_type
    {
        report(
            diagnostics,
            caller,
            instruction,
            format!("coro.resume value has type {actual}; expected {resume_type}"),
        );
    }
    let targets = index_values(instruction);
    if let Some(target) = targets.first() {
        verify_implicit_edge(
            instruction,
            body,
            *target,
            &value_edge_types(types, *yield_type),
            caller,
            diagnostics,
        );
    }
    if let Some(target) = targets.get(1) {
        verify_implicit_edge(
            instruction,
            body,
            *target,
            &value_edge_types(types, *result_type),
            caller,
            diagnostics,
        );
    }
}

fn verify_coroutine_handle(
    instruction: &BinaryInstruction,
    caller: &str,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(handle_type) = value_type(instruction, 0, value_types)
        && !matches!(
            type_at(types, handle_type),
            Some(BinaryType::Coroutine { .. })
        )
    {
        report(
            diagnostics,
            caller,
            instruction,
            "coroutine operation requires a coroutine handle",
        );
    }
}

fn verify_frame_query(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some((_, signature)) = direct_callee(instruction, signatures) else {
        return;
    };
    let is_async =
        function_type(types, signature.function_type).is_some_and(|function| function.asynchronous);
    if signature.coroutine.is_none() && !is_async {
        report(
            diagnostics,
            &signature.name,
            instruction,
            "coroutine frame query requires a coroutine or async function",
        );
    }
}

fn verify_await(
    instruction: &BinaryInstruction,
    caller_name: &str,
    caller: &BinaryFunctionType,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    value_types: &[Option<u64>],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !caller.asynchronous {
        report(
            diagnostics,
            caller_name,
            instruction,
            "await is used outside an async function",
        );
    }
    let Some(future_type) = value_type(instruction, 0, value_types) else {
        return;
    };
    let Some(BinaryType::Frame { function_index }) = type_at(types, future_type) else {
        return;
    };
    let Some(signature) = signatures
        .records
        .get(usize::try_from(*function_index).unwrap_or(usize::MAX))
    else {
        return;
    };
    let Some(function) = function_type(types, signature.function_type) else {
        return;
    };
    if !function.asynchronous {
        report(
            diagnostics,
            caller_name,
            instruction,
            "await frame does not belong to an async function",
        );
    }
    let expected = value_edge_types(types, function.result).len() as u64;
    if instruction.result_count != expected {
        report(
            diagnostics,
            caller_name,
            instruction,
            format!(
                "await defines {} results; expected {expected}",
                instruction.result_count
            ),
        );
    }
}

fn verify_implicit_edge(
    instruction: &BinaryInstruction,
    body: &BinaryFunctionBody,
    target: u64,
    expected: &[u64],
    caller: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(block) = usize::try_from(target)
        .ok()
        .and_then(|index| body.blocks.get(index))
    else {
        return;
    };
    let actual = block
        .parameters
        .iter()
        .map(|parameter| parameter.ty)
        .collect::<Vec<_>>();
    if actual != expected {
        report(
            diagnostics,
            caller,
            instruction,
            format!(
                "implicit coroutine edge to block {target} has parameter types {actual:?}; expected {expected:?}"
            ),
        );
    }
}

fn value_edge_types(types: &BinaryTypeTable, ty: u64) -> Vec<u64> {
    match type_at(types, ty) {
        Some(BinaryType::Unit | BinaryType::Never) | None => Vec::new(),
        Some(_) => vec![ty],
    }
}

fn direct_callee<'a>(
    instruction: &BinaryInstruction,
    signatures: &'a BinarySignatureTable,
) -> Option<(u64, &'a BinarySignature)> {
    let index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => Some(*index),
            _ => None,
        })?;
    let signature = signatures.records.get(usize::try_from(index).ok()?)?;
    Some((index, signature))
}

fn function_type(types: &BinaryTypeTable, index: u64) -> Option<&BinaryFunctionType> {
    match type_at(types, index)? {
        BinaryType::Function(function) => Some(function),
        _ => None,
    }
}

fn type_at(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}

fn value_ids(instruction: &BinaryInstruction) -> Vec<u64> {
    instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => vec![*value],
            BinaryOperand::Values(values) => values.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn value_type(
    instruction: &BinaryInstruction,
    operand_index: usize,
    value_types: &[Option<u64>],
) -> Option<u64> {
    value_type_id(*value_ids(instruction).get(operand_index)?, value_types)
}

fn value_type_id(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    value_types
        .get(usize::try_from(value).ok()?)
        .copied()
        .flatten()
}

fn index_values(instruction: &BinaryInstruction) -> Vec<u64> {
    instruction
        .operands
        .iter()
        .filter_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })
        .collect()
}

fn report(
    diagnostics: &mut Vec<Diagnostic>,
    function: &str,
    instruction: &BinaryInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(
        V2,
        format!(
            "function @{function} CODE offset {}: {}",
            instruction.offset,
            message.into()
        ),
    ));
}
