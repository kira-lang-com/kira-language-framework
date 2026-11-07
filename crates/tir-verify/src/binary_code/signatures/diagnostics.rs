//! Shared call and return diagnostics.
use tir_diag::{Code, Diagnostic};
use tir_format::{BinaryInstruction, BinaryType, BinaryTypeTable};

use super::{
    V2,
    helpers::{result_edge_types, type_at, value_ids},
};

pub(super) fn verify_exit(
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

pub(super) fn report(
    diagnostics: &mut Vec<Diagnostic>,
    function: &str,
    instruction: &BinaryInstruction,
    message: impl Into<String>,
) {
    report_with_code(diagnostics, function, instruction, V2, message);
}

pub(super) fn report_with_code(
    diagnostics: &mut Vec<Diagnostic>,
    function: &str,
    instruction: &BinaryInstruction,
    code: Code,
    message: impl Into<String>,
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
