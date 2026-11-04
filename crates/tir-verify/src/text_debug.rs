use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{FunctionBody, TextDebugAttachment};
use tir_types::Type;

const VFY1: Code = Code::new(Dialect::Tir, Area::Verifier, 1);

pub(super) fn verify(body: &FunctionBody, value_types: &HashMap<String, Type>) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        let Some(TextDebugAttachment::Variable { value, ty, .. }) =
            instruction.debug_attachment.as_ref()
        else {
            continue;
        };
        let Some(value_type) = value_types.get(value) else {
            continue;
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
            diagnostics.push(Diagnostic::error(VFY1, message).with_primary_span(instruction.span));
        }
    }
    diagnostics
}
