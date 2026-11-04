mod call_verification;
mod generic_types;
mod helpers;
mod witness_verification;

use call_verification::verify_call;
use generic_types::{
    call_result_edge_types, call_type_arguments, infer_type_arguments,
    specialized_result_edge_types, type_matches_with_arguments,
};

use helpers::{
    check_call_type_argument_count, contains_generic, explicit_type, frame_type, function_type,
    resolve_direct_callee, result_edge_types, type_at, value_ids, value_type,
};
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryFunctionType, BinaryGenericTable, BinaryInstruction, BinaryOperand,
    BinarySignature, BinarySignatureTable, BinaryType, BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const V10: Code = Code::new(Dialect::Tir, Area::Verifier, 10);
const V11: Code = Code::new(Dialect::Tir, Area::Verifier, 11);
const V19: Code = Code::new(Dialect::Tir, Area::Verifier, 19);

pub(super) fn verify_generic_witness_signatures(
    generics: &BinaryGenericTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
) -> Vec<Diagnostic> {
    witness_verification::verify(generics, types, signatures)
}

pub(super) fn verify_body(
    body: &BinaryFunctionBody,
    caller: &BinarySignature,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    value_types: &[Option<u64>],
    generics: Option<&tir_format::BinaryGenericTable>,
    strings: Option<&tir_format::StringTable>,
    imports: Option<&[tir_format::BinaryImport]>,
    globals: Option<&tir_format::BinaryGlobalTable>,
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
                0x1e5 => verify_exit(
                    instruction,
                    caller_function.result,
                    value_types,
                    types,
                    &caller.name,
                    "return",
                    &mut diagnostics,
                ),
                0x1e6 => {
                    if let Some(error_type) = caller_function.error {
                        verify_exit(
                            instruction,
                            error_type,
                            value_types,
                            types,
                            &caller.name,
                            "fail",
                            &mut diagnostics,
                        );
                    } else {
                        report(
                            &mut diagnostics,
                            &caller.name,
                            instruction,
                            "fail is used in a function without an error type",
                        );
                    }
                }
                0x1a0 | 0x1a8 | 0x1a7 | 0x1aa | 0x1e7 => {
                    let Some(callee) = resolve_direct_callee(
                        instruction,
                        signatures,
                        types,
                        imports.unwrap_or_default(),
                    ) else {
                        continue;
                    };
                    check_call_type_argument_count(
                        instruction,
                        &callee,
                        &caller.name,
                        &mut diagnostics,
                    );
                    let type_arguments =
                        call_type_arguments(instruction, callee.generic_parameter_count);
                    verify_direct_call(
                        instruction,
                        &callee.name,
                        callee.local_index,
                        type_arguments,
                        callee.generic_parameter_count,
                        callee.function_type,
                        value_types,
                        types,
                        caller_function,
                        caller.modifiers & (1 << 42) != 0,
                        &caller.name,
                        body,
                        &mut diagnostics,
                    );
                }
                0x1a1 | 0x1a9 => {
                    if let Some(function_index) = explicit_type(instruction)
                        && let Some(function) = function_type(types, function_index)
                    {
                        if instruction.opcode == 0x1a1
                            && let Some(actual_function_type) =
                                value_type(instruction, 0, value_types)
                            && actual_function_type != function_index
                        {
                            report_v19(
                                &mut diagnostics,
                                &caller.name,
                                instruction,
                                format!(
                                    "indirect-call value has function type {actual_function_type}; declared type is {function_index}"
                                ),
                            );
                        }
                        if instruction.opcode == 0x1a9
                            && let Some(callee) = resolve_direct_callee(
                                instruction,
                                signatures,
                                types,
                                imports.unwrap_or_default(),
                            )
                            && callee.generic_parameter_count == 0
                            && !contains_generic(types, callee.function_type_index, &mut Vec::new())
                            && callee.function_type_index != function_index
                        {
                            report(
                                &mut diagnostics,
                                &caller.name,
                                instruction,
                                format!(
                                    "variadic-call type {function_index} does not match callee type {}",
                                    callee.function_type_index
                                ),
                            );
                        }
                        let skip = usize::from(instruction.opcode == 0x1a1);
                        let arguments: Vec<u64> = if instruction.opcode == 0x1a9 {
                            let fixed = instruction
                                .operands
                                .iter()
                                .filter_map(|operand| match operand {
                                    BinaryOperand::Index(index) => Some(*index),
                                    _ => None,
                                })
                                .next()
                                .and_then(|count| usize::try_from(count).ok())
                                .unwrap_or(0);
                            value_ids(instruction).into_iter().take(fixed).collect()
                        } else {
                            value_ids(instruction).into_iter().skip(skip).collect()
                        };
                        verify_call(
                            instruction,
                            "indirect call",
                            function,
                            &arguments,
                            None,
                            value_types,
                            types,
                            &caller.name,
                            &mut diagnostics,
                        );
                    }
                }
                0x1a3 => {
                    let Some(closure_type) = value_type(instruction, 0, value_types)
                        .and_then(|index| type_at(types, index))
                    else {
                        continue;
                    };
                    let BinaryType::Closure {
                        function_type: closure_function_type,
                        ..
                    } = closure_type
                    else {
                        continue;
                    };
                    if let Some(function) = function_type(types, *closure_function_type) {
                        let arguments: Vec<u64> =
                            value_ids(instruction).into_iter().skip(1).collect();
                        verify_call(
                            instruction,
                            "closure call",
                            function,
                            &arguments,
                            None,
                            value_types,
                            types,
                            &caller.name,
                            &mut diagnostics,
                        );
                    }
                }
                0x1a5 | 0x1a6 => {
                    let Some(function_index) = super::owned_values::witness_or_vtable_function_type(
                        instruction,
                        signatures,
                        generics,
                        strings,
                        imports,
                        globals,
                    ) else {
                        continue;
                    };
                    let Some(function) = function_type(types, function_index) else {
                        continue;
                    };
                    verify_call(
                        instruction,
                        if instruction.opcode == 0x1a5 {
                            "witness call"
                        } else {
                            "vtable call"
                        },
                        function,
                        &value_ids(instruction),
                        None,
                        value_types,
                        types,
                        &caller.name,
                        &mut diagnostics,
                    );
                }
                0x404 => verify_vft_call(
                    instruction,
                    value_types,
                    types,
                    &caller.name,
                    &mut diagnostics,
                ),
                _ => {}
            }
        }
    }
    diagnostics
}

fn verify_direct_call(
    instruction: &BinaryInstruction,
    callee_name: &str,
    callee_index: Option<u64>,
    type_arguments: Option<&[u64]>,
    generic_parameter_count: usize,
    function: &BinaryFunctionType,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    caller: &BinaryFunctionType,
    caller_is_nounwind: bool,
    caller_name: &str,
    body: &BinaryFunctionBody,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let opcode = instruction.opcode;
    let is_try_call = opcode == 0x1a7;
    let is_invoke = opcode == 0x1aa;
    let is_tail_call = opcode == 0x1e7;
    let inferred_type_arguments = if type_arguments.is_none()
        && generic_parameter_count > 0
        && (is_try_call || is_invoke)
    {
        let inferred = infer_call_type_arguments(
            instruction,
            function,
            generic_parameter_count,
            value_types,
            types,
        );
        if inferred.is_none() {
            report_with_code(
                diagnostics,
                caller_name,
                instruction,
                format!(
                    "call to @{callee_name} cannot infer all generic type arguments from its arguments"
                ),
                V11,
            );
        }
        inferred
    } else {
        None
    };
    let type_arguments = type_arguments.or(inferred_type_arguments.as_deref());
    if opcode == 0x1a0 || opcode == 0x1a8 || is_try_call || is_invoke || is_tail_call {
        let arguments = value_ids(instruction);
        verify_call(
            instruction,
            callee_name,
            function,
            &arguments,
            type_arguments,
            value_types,
            types,
            caller_name,
            diagnostics,
        );
    }

    if is_try_call {
        if function.error.is_none() {
            report_v10(
                diagnostics,
                caller_name,
                instruction,
                "try.call requires a callee function type with fails",
            );
        }
        verify_try_call_edges(
            instruction,
            body,
            function,
            callee_index.unwrap_or(u64::MAX),
            types,
            type_arguments,
            caller_name,
            diagnostics,
        );
    } else if is_invoke {
        if caller_is_nounwind {
            report_v10(
                diagnostics,
                caller_name,
                instruction,
                "invoke is not permitted in a nounwind function",
            );
        }
        verify_invoke_edges(
            instruction,
            body,
            function,
            callee_index.unwrap_or(u64::MAX),
            types,
            type_arguments,
            caller_name,
            diagnostics,
        );
    }

    let callee_result_matches = if function.asynchronous {
        callee_index
            .and_then(|callee_index| frame_type(types, callee_index))
            .is_some_and(|result| result == caller.result)
    } else {
        type_matches_with_arguments(types, function.result, caller.result, type_arguments)
    };
    let callee_error_matches = match (function.error, caller.error) {
        (None, None) => true,
        (Some(callee_error), Some(caller_error)) => {
            type_matches_with_arguments(types, callee_error, caller_error, type_arguments)
        }
        _ => false,
    };
    if is_tail_call
        && (!callee_result_matches || !callee_error_matches || function.c_abi != caller.c_abi)
    {
        report(
            diagnostics,
            caller_name,
            instruction,
            "tail.call callee ABI and result types must match the caller",
        );
    }
}

fn infer_call_type_arguments(
    instruction: &BinaryInstruction,
    function: &BinaryFunctionType,
    generic_parameter_count: usize,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> Option<Vec<u64>> {
    let mut inferred = vec![None; generic_parameter_count];
    let arguments = value_ids(instruction);
    for (parameter, argument) in function.parameters.iter().zip(arguments) {
        let actual = usize::try_from(argument)
            .ok()
            .and_then(|index| value_types.get(index))
            .copied()
            .flatten()?;
        let mut candidate = inferred.clone();
        if infer_type_arguments(types, parameter.ty, actual, &mut candidate) {
            inferred = candidate;
        }
    }
    inferred.into_iter().collect()
}

fn verify_try_call_edges(
    instruction: &BinaryInstruction,
    body: &BinaryFunctionBody,
    function: &BinaryFunctionType,
    callee_index: u64,
    types: &BinaryTypeTable,
    type_arguments: Option<&[u64]>,
    caller_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let targets = instruction
        .operands
        .iter()
        .filter_map(|operand| match operand {
            BinaryOperand::Index(target) => Some(*target),
            _ => None,
        })
        .collect::<Vec<_>>();
    let (result, result_type_known) =
        call_result_edge_types(types, function, callee_index, type_arguments);
    if let Some(target) = targets.first()
        && let Some(block) = usize::try_from(*target)
            .ok()
            .and_then(|index| body.blocks.get(index))
    {
        verify_implicit_edge(
            instruction,
            caller_name,
            *target,
            &block
                .parameters
                .iter()
                .map(|parameter| parameter.ty)
                .collect::<Vec<_>>(),
            &result,
            types,
            result_type_known,
            type_arguments,
            diagnostics,
        );
    }
    if let (Some(target), Some(error_type)) = (targets.get(1), function.error)
        && let Some(block) = usize::try_from(*target)
            .ok()
            .and_then(|index| body.blocks.get(index))
    {
        let error_values = specialized_result_edge_types(types, error_type, type_arguments);
        verify_implicit_edge(
            instruction,
            caller_name,
            *target,
            &block
                .parameters
                .iter()
                .map(|parameter| parameter.ty)
                .collect::<Vec<_>>(),
            &error_values,
            types,
            true,
            type_arguments,
            diagnostics,
        );
    }
}

fn verify_invoke_edges(
    instruction: &BinaryInstruction,
    body: &BinaryFunctionBody,
    function: &BinaryFunctionType,
    callee_index: u64,
    types: &BinaryTypeTable,
    type_arguments: Option<&[u64]>,
    caller_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let targets = instruction
        .operands
        .iter()
        .filter_map(|operand| match operand {
            BinaryOperand::Index(target) => Some(*target),
            _ => None,
        })
        .collect::<Vec<_>>();
    let (expected, result_type_known) =
        call_result_edge_types(types, function, callee_index, type_arguments);
    if let Some(target) = targets.first()
        && let Some(block) = usize::try_from(*target)
            .ok()
            .and_then(|index| body.blocks.get(index))
    {
        verify_implicit_edge(
            instruction,
            caller_name,
            *target,
            &block
                .parameters
                .iter()
                .map(|parameter| parameter.ty)
                .collect::<Vec<_>>(),
            &expected,
            types,
            result_type_known,
            type_arguments,
            diagnostics,
        );
    }
    if let Some(target) = targets.get(1)
        && let Some(block) = usize::try_from(*target)
            .ok()
            .and_then(|index| body.blocks.get(index))
    {
        if !block.parameters.is_empty() {
            report(
                diagnostics,
                caller_name,
                instruction,
                format!("invoke unwind target block {target} has parameters; expected none"),
            );
        }
        if block
            .instructions
            .as_ref()
            .and_then(|instructions| instructions.first())
            .is_none_or(|first| first.opcode != 0x1d0)
        {
            report(
                diagnostics,
                caller_name,
                instruction,
                format!("invoke unwind target block {target} does not begin with landingpad"),
            );
        }
    }
}

fn verify_implicit_edge(
    instruction: &BinaryInstruction,
    caller_name: &str,
    target: u64,
    actual: &[u64],
    expected: &[u64],
    types: &BinaryTypeTable,
    compare_types: bool,
    type_arguments: Option<&[u64]>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let types_match = !compare_types
        || actual.iter().zip(expected).all(|(actual, expected)| {
            type_matches_with_arguments(types, *expected, *actual, type_arguments)
        });
    if actual.len() != expected.len() || !types_match {
        report(
            diagnostics,
            caller_name,
            instruction,
            format!(
                "implicit edge to block {target} has parameter types {actual:?}; expected {expected:?}"
            ),
        );
    }
}

fn verify_vft_call(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    caller: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(function_index) = explicit_type(instruction) else {
        return;
    };
    let Some(function) = function_type(types, function_index) else {
        return;
    };
    if let Some(table_index) = value_type(instruction, 0, value_types) {
        match type_at(types, table_index) {
            Some(BinaryType::VisibleFunctionTable {
                function_type: table_function_type,
            }) if *table_function_type == function_index => {}
            Some(BinaryType::VisibleFunctionTable {
                function_type: table_function_type,
            }) => report(
                diagnostics,
                caller,
                instruction,
                format!(
                    "gpu.call.vft signature type {function_index} does not match table type {table_function_type}"
                ),
            ),
            Some(_) => report(
                diagnostics,
                caller,
                instruction,
                "gpu.call.vft first operand is not a visible function table",
            ),
            None => {}
        }
    }
    if let Some(index_type) = value_type(instruction, 1, value_types)
        && !matches!(type_at(types, index_type), Some(BinaryType::Integer(_)))
    {
        report(
            diagnostics,
            caller,
            instruction,
            "gpu.call.vft table index must have an integer type",
        );
    }
    if function.error.is_some() {
        report(
            diagnostics,
            caller,
            instruction,
            "gpu.call.vft function type cannot fail",
        );
    }
    let arguments = value_ids(instruction)
        .into_iter()
        .skip(2)
        .collect::<Vec<_>>();
    verify_call(
        instruction,
        "visible function table entry",
        function,
        &arguments,
        None,
        value_types,
        types,
        caller,
        diagnostics,
    );
}

fn verify_exit(
    instruction: &BinaryInstruction,
    expected: u64,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    caller: &str,
    name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if name == "return" && matches!(type_at(types, expected), Some(BinaryType::Never)) {
        report(
            diagnostics,
            caller,
            instruction,
            "a function returning never cannot use return",
        );
        return;
    }
    let expected_values = result_edge_types(types, expected);
    let values = value_ids(instruction);
    if values.len() != expected_values.len() {
        report(
            diagnostics,
            caller,
            instruction,
            format!(
                "{name} passes {} values; expected {}",
                values.len(),
                expected_values.len()
            ),
        );
        return;
    }
    for (index, (value, expected)) in values.iter().zip(expected_values).enumerate() {
        if let Some(actual) = value_types
            .get(usize::try_from(*value).unwrap_or(usize::MAX))
            .copied()
            .flatten()
            && actual != expected
        {
            report(
                diagnostics,
                caller,
                instruction,
                format!(
                    "{name} value {} has type {actual}; expected {expected}",
                    index + 1
                ),
            );
        }
    }
}

fn report(
    diagnostics: &mut Vec<Diagnostic>,
    function: &str,
    instruction: &BinaryInstruction,
    message: impl Into<String>,
) {
    report_with_code(diagnostics, function, instruction, message, V2);
}

fn report_v10(
    diagnostics: &mut Vec<Diagnostic>,
    function: &str,
    instruction: &BinaryInstruction,
    message: impl Into<String>,
) {
    report_with_code(diagnostics, function, instruction, message, V10);
}

fn report_v19(
    diagnostics: &mut Vec<Diagnostic>,
    function: &str,
    instruction: &BinaryInstruction,
    message: impl Into<String>,
) {
    report_with_code(diagnostics, function, instruction, message, V19);
}

fn report_with_code(
    diagnostics: &mut Vec<Diagnostic>,
    function: &str,
    instruction: &BinaryInstruction,
    message: impl Into<String>,
    code: Code,
) {
    diagnostics.push(Diagnostic::error(
        code,
        format!(
            "function @{function} CODE offset {}: {}",
            instruction.offset,
            message.into()
        ),
    ));
}
