use super::*;

pub(super) fn verify_call(
    instruction: &BinaryInstruction,
    callee: &str,
    function: &BinaryFunctionType,
    arguments: &[u64],
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    caller: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if function.error.is_some() && !matches!(instruction.opcode, 0x1a7 | 0x1e7 | 0x404) {
        report_with_code(
            diagnostics,
            caller,
            instruction,
            V10,
            "a callee with fails must use try.call",
        );
    }
    if arguments.len() != function.parameters.len() {
        report(
            diagnostics,
            caller,
            instruction,
            format!(
                "call to {callee} has {} arguments; expected {}",
                arguments.len(),
                function.parameters.len()
            ),
        );
    }
    if !is_generic_call(function, types) {
        for (index, (argument, parameter)) in arguments.iter().zip(&function.parameters).enumerate()
        {
            if let Some(actual) = value_types
                .get(usize::try_from(*argument).unwrap_or(usize::MAX))
                .copied()
                .flatten()
                && actual != parameter.ty
            {
                report(
                    diagnostics,
                    caller,
                    instruction,
                    format!(
                        "call to {callee} argument {} has type {actual}; expected {}",
                        index + 1,
                        parameter.ty
                    ),
                );
            }
        }
    }
    if !matches!(instruction.opcode, 0x1a7 | 0x1aa | 0x1e7) {
        let expected = if function.asynchronous {
            1
        } else {
            result_edge_types(types, function.result).len() as u64
        };
        if instruction.result_count != expected {
            report(
                diagnostics,
                caller,
                instruction,
                format!(
                    "call to {callee} defines {} results; expected {expected}",
                    instruction.result_count
                ),
            );
        }
    }
}
