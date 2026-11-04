use std::collections::HashMap;

use tir_diag::Diagnostic;
use tir_format::{TextDebugAttachment, TextInstruction};
use tir_types::Type;

use crate::instruction_types::push_error;

pub(super) fn verify_attachment(
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(TextDebugAttachment::Variable { value, ty, .. }) =
        instruction.debug_attachment.as_ref()
    else {
        return;
    };
    let Some(value_type) = value_types.get(value) else {
        return;
    };
    let message = match instruction.opcode.as_str() {
        "debug.value" if value_type != ty => {
            Some("debug.value does not match the declared variable type")
        }
        "debug.addr" => match value_type {
            Type::Address { pointee, .. } if pointee.as_ref() != ty => {
                Some("debug.addr does not match the declared variable type")
            }
            Type::Address { .. } | Type::Pointer(_) => None,
            _ => Some("debug.addr requires a pointer or address operand"),
        },
        _ => None,
    };
    if let Some(message) = message {
        push_error(diagnostics, instruction, message);
    }
}
