use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_types::{FunctionType, Type};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(instruction: &TextInstruction, operands: &[Type]) -> Vec<Diagnostic> {
    if instruction.opcode != "gpu.call.vft" {
        return Vec::new();
    }
    let mut diagnostics = Vec::new();
    let Some(Type::VisibleFunctionTable(function)) = operands.first() else {
        if operands.first().is_some() {
            error(
                &mut diagnostics,
                instruction,
                "gpu.call.vft first operand must be a visible function table",
            );
        }
        return diagnostics;
    };
    if function.failure.is_some() {
        error(
            &mut diagnostics,
            instruction,
            "gpu.call.vft function type cannot fail",
        );
    }
    if operands
        .get(1)
        .is_some_and(|ty| !matches!(ty, Type::Integer(_)))
    {
        error(
            &mut diagnostics,
            instruction,
            "gpu.call.vft table index must be an integer",
        );
    }
    if operands.len() != function.parameters.len() + 2 {
        error(
            &mut diagnostics,
            instruction,
            format!(
                "gpu.call.vft has {} arguments; function type requires {}",
                operands.len().saturating_sub(2),
                function.parameters.len()
            ),
        );
    }
    for (index, (actual, expected)) in operands
        .iter()
        .skip(2)
        .zip(&function.parameters)
        .enumerate()
    {
        if actual != &expected.ty {
            error(
                &mut diagnostics,
                instruction,
                format!(
                    "gpu.call.vft argument {} has type {actual}; expected {}",
                    index + 1,
                    expected.ty
                ),
            );
        }
    }
    let result_count = result_types(function).len();
    if instruction.results.len() != result_count {
        error(
            &mut diagnostics,
            instruction,
            format!(
                "gpu.call.vft defines {} results; function type yields {result_count}",
                instruction.results.len()
            ),
        );
    }
    diagnostics
}

pub(super) fn infer_result_types(operands: &[Type]) -> Vec<Type> {
    operands
        .first()
        .and_then(|ty| match ty {
            Type::VisibleFunctionTable(function) => Some(result_types(function)),
            _ => None,
        })
        .unwrap_or_default()
}

fn result_types(function: &FunctionType) -> Vec<Type> {
    match function.result.as_ref() {
        Type::Unit => Vec::new(),
        Type::Tuple(elements) => elements.clone(),
        result => vec![result.clone()],
    }
}

fn error(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(V2, message.into()).with_primary_span(instruction.span));
}
