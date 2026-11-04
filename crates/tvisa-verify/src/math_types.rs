use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_types::Type;

use crate::type_classes::is_float;

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify(instruction: &TextInstruction, operands: &[Type]) -> Vec<Diagnostic> {
    let expected = match instruction.opcode.as_str() {
        "math.atan2" | "math.pow" | "math.hypot" => 2,
        "math.sin" | "math.cos" | "math.tan" | "math.asin" | "math.acos" | "math.atan"
        | "math.sinh" | "math.cosh" | "math.tanh" | "math.asinh" | "math.acosh" | "math.atanh"
        | "math.exp" | "math.exp2" | "math.exp10" | "math.expm1" | "math.log" | "math.log2"
        | "math.log10" | "math.log1p" | "math.cbrt" | "math.rsqrt" => 1,
        _ => {
            return vec![error(
                instruction,
                format!("unknown mathematical instruction {}", instruction.opcode),
            )];
        }
    };
    let mut diagnostics = Vec::new();
    if operands.len() != expected {
        diagnostics.push(error(
            instruction,
            format!(
                "{} has {} operands; expected {expected}",
                instruction.opcode,
                operands.len()
            ),
        ));
    }
    if let Some(first) = operands.first() {
        if !is_float(first) {
            diagnostics.push(error(
                instruction,
                format!("{} requires floating-point values", instruction.opcode),
            ));
        }
        if operands.iter().skip(1).any(|operand| operand != first) {
            diagnostics.push(error(
                instruction,
                format!("{} operands must have the same type", instruction.opcode),
            ));
        }
    }
    diagnostics
}

fn error(instruction: &TextInstruction, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(VFY3, message.into()).with_primary_span(instruction.span)
}
