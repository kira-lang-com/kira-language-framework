mod call_verification;
mod calls;
mod diagnostics;
mod generic_types;
mod helpers;

use diagnostics::{report, report_with_code, verify_exit};
use helpers::{
    contains_generic, direct_callee, explicit_type, frame_type, function_type, is_generic_call,
    result_edge_types, type_at, value_ids, value_type,
};
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryFunctionType, BinaryInstruction, BinaryOperand, BinarySignature,
    BinarySignatureTable, BinaryType, BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const V10: Code = Code::new(Dialect::Tir, Area::Verifier, 10);
const V11: Code = Code::new(Dialect::Tir, Area::Verifier, 11);
const V19: Code = Code::new(Dialect::Tir, Area::Verifier, 19);

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
                    let Some((callee_index, callee)) = direct_callee(instruction, signatures)
                    else {
                        continue;
                    };
                    let type_arguments =
                        instruction
                            .operands
                            .iter()
                            .find_map(|operand| match operand {
                                BinaryOperand::Types(arguments) => Some(arguments.as_slice()),
                                _ => None,
                            });
                    let generic_parameter_count = callee.generic_parameters.len();
                    let infers_generic_arguments =
                        matches!(instruction.opcode, 0x1a7 | 0x1aa) && type_arguments.is_none();
                    if matches!(instruction.opcode, 0x1a7 | 0x1aa) && type_arguments.is_some() {
                        report_with_code(
                            &mut diagnostics,
                            &caller.name,
                            instruction,
                            V2,
                            "try.call and invoke infer generic type arguments and do not encode them",
                        );
                        continue;
                    }
                    let type_argument_count = type_arguments.map_or(0, <[u64]>::len);
                    if !infers_generic_arguments && type_argument_count != generic_parameter_count {
                        report_with_code(
                            &mut diagnostics,
                            &caller.name,
                            instruction,
                            V2,
                            format!(
                                "call to @{} has {} type arguments; expected {}",
                                callee.name, type_argument_count, generic_parameter_count
                            ),
                        );
                        continue;
                    }
                    let specialized_type = if type_arguments.is_none_or(<[u64]>::is_empty) {
                        Some(callee.function_type)
                    } else {
                        super::types::specialize_nominal_type(
                            types,
                            callee.function_type,
                            type_arguments.unwrap_or_default(),
                        )
                    };
                    let Some(specialized_type) = specialized_type else {
                        report_with_code(
                            &mut diagnostics,
                            &caller.name,
                            instruction,
                            V2,
                            format!(
                                "call to @{} does not resolve to a function type in TYPES after generic substitution",
                                callee.name
                            ),
                        );
                        continue;
                    };
                    let Some(function) = function_type(types, specialized_type) else {
                        continue;
                    };
                    verify_direct_call(
                        instruction,
                        &callee.name,
                        callee_index,
                        type_arguments,
                        generic_parameter_count,
                        function,
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
                            report_with_code(
                                &mut diagnostics,
                                &caller.name,
                                instruction,
                                V19,
                                format!(
                                    "indirect-call value has function type {actual_function_type}; declared type is {function_index}"
                                ),
                            );
                        }
                        if instruction.opcode == 0x1a9
                            && let Some((_, callee)) = direct_callee(instruction, signatures)
                            && callee.generic_parameters.is_empty()
                            && !contains_generic(types, callee.function_type, &mut Vec::new())
                            && callee.function_type != function_index
                        {
                            report(
                                &mut diagnostics,
                                &caller.name,
                                instruction,
                                format!(
                                    "variadic-call type {function_index} does not match callee type {}",
                                    callee.function_type
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
                        calls::verify_call(
                            instruction,
                            "indirect call",
                            function,
                            &arguments,
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
                        calls::verify_call(
                            instruction,
                            "closure call",
                            function,
                            &arguments,
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
                    calls::verify_call(
                        instruction,
                        if instruction.opcode == 0x1a5 {
                            "witness call"
                        } else {
                            "vtable call"
                        },
                        function,
                        &value_ids(instruction),
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
    callee_index: u64,
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
                V11,
                format!(
                    "call to @{callee_name} cannot infer all generic type arguments from its arguments"
                ),
            );
        }
        inferred
    } else {
        None
    };
    let type_arguments = type_arguments.or(inferred_type_arguments.as_deref());
    if opcode == 0x1a0 || opcode == 0x1a8 || is_try_call || is_invoke || is_tail_call {
        let arguments = value_ids(instruction);
        call_verification::verify_call(
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
            report_with_code(
                diagnostics,
                caller_name,
                instruction,
                V10,
                "try.call requires a callee function type with fails",
            );
        }
        verify_try_call_edges(
            instruction,
            body,
            function,
            callee_index,
            type_arguments,
            types,
            caller_name,
            diagnostics,
        );
    } else if is_invoke {
        if caller_is_nounwind {
            report_with_code(
                diagnostics,
                caller_name,
                instruction,
                V10,
                "invoke is not permitted in a nounwind function",
            );
        }
        verify_invoke_edges(
            instruction,
            body,
            function,
            callee_index,
            type_arguments,
            types,
            caller_name,
            diagnostics,
        );
    }

    let callee_result_matches = if function.asynchronous {
        frame_type(types, callee_index).is_some_and(|result| result == caller.result)
    } else {
        generic_types::type_matches_with_arguments(
            types,
            function.result,
            caller.result,
            type_arguments,
        )
    };
    let callee_error_matches = match (function.error, caller.error) {
        (None, None) => true,
        (Some(callee_error), Some(caller_error)) => generic_types::type_matches_with_arguments(
            types,
            callee_error,
            caller_error,
            type_arguments,
        ),
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
    let arguments = value_ids(instruction);
    let mut inferred = vec![None; generic_parameter_count];
    for (argument, parameter) in arguments.iter().zip(&function.parameters) {
        let actual = value_types
            .get(usize::try_from(*argument).ok()?)
            .copied()
            .flatten()?;
        if !generic_types::infer_type_arguments(types, parameter.ty, actual, &mut inferred) {
            return None;
        }
    }
    inferred.into_iter().collect()
}

fn verify_try_call_edges(
    instruction: &BinaryInstruction,
    body: &BinaryFunctionBody,
    function: &BinaryFunctionType,
    callee_index: u64,
    type_arguments: Option<&[u64]>,
    types: &BinaryTypeTable,
    caller_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let compare_types = type_arguments.is_some() || !is_generic_call(function, types);
    let targets = instruction
        .operands
        .iter()
        .filter_map(|operand| match operand {
            BinaryOperand::Index(target) => Some(*target),
            _ => None,
        })
        .collect::<Vec<_>>();
    let (result, result_type_known) =
        generic_types::call_result_edge_types(types, function, callee_index, type_arguments);
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
            compare_types && result_type_known,
            diagnostics,
        );
    }
    if let (Some(target), Some(error_type)) = (targets.get(1), function.error)
        && let Some(block) = usize::try_from(*target)
            .ok()
            .and_then(|index| body.blocks.get(index))
    {
        let error_values =
            generic_types::specialized_result_edge_types(types, error_type, type_arguments);
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
            compare_types,
            diagnostics,
        );
    }
}

fn verify_invoke_edges(
    instruction: &BinaryInstruction,
    body: &BinaryFunctionBody,
    function: &BinaryFunctionType,
    callee_index: u64,
    type_arguments: Option<&[u64]>,
    types: &BinaryTypeTable,
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
    let compare_types = type_arguments.is_some() || !is_generic_call(function, types);
    let (expected, result_type_known) =
        generic_types::call_result_edge_types(types, function, callee_index, type_arguments);
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
            compare_types && result_type_known,
            diagnostics,
        );
    }
    if let Some(target) = targets.get(1)
        && let Some(block) = usize::try_from(*target)
            .ok()
            .and_then(|index| body.blocks.get(index))
        && block
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

fn verify_implicit_edge(
    instruction: &BinaryInstruction,
    caller_name: &str,
    target: u64,
    actual: &[u64],
    expected: &[u64],
    compare_types: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if actual.len() != expected.len() || compare_types && actual != expected {
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
    calls::verify_call(
        instruction,
        "visible function table entry",
        function,
        &arguments,
        value_types,
        types,
        caller,
        diagnostics,
    );
}
