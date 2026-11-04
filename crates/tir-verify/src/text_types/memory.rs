use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::text::TokenKind;
use tir_layout::LayoutEngine;
use tir_types::Type;

use crate::gpu_profile::policy::TypePolicies;
use crate::model::ValueUse;
use crate::ssa::text_instruction_operands_start;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify_instruction(
    source: &str,
    instruction: &TextInstruction,
    operands: &[ValueUse],
    results: Option<&[Type]>,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let error =
        |message: String| Diagnostic::error(V2, message).with_primary_span(instruction.span);
    let result = results.and_then(|results| results.first());
    match instruction.opcode.as_str() {
        "load" => {
            let selected = selector(instruction).unwrap_or_else(|| "trivial".to_owned());
            let pointee = operands
                .first()
                .and_then(|operand| address_pointee(&operand.ty));
            if operands.len() != 1 || result.is_none() || pointee.is_none() {
                diagnostics.push(error("load requires one address and one result".to_owned()));
            } else if result != pointee {
                diagnostics.push(error(
                    "load result type must match its address pointee".to_owned(),
                ));
            } else if let Some(ty) = pointee {
                if !policies.is_loadable(ty, layouts) {
                    diagnostics.push(error(format!("load type {ty} is address-only")));
                }
                match selected.as_str() {
                    "trivial" if !policies.is_trivial(ty) => diagnostics.push(error(
                        "load [trivial] requires a type with trivial copy and destroy policies"
                            .to_owned(),
                    )),
                    "copy" if !policies.is_copyable(ty) => diagnostics.push(error(
                        "load [copy] requires a type with a copy policy".to_owned(),
                    )),
                    "copy" | "take" | "trivial" => {}
                    _ => {}
                }
            }
            if instruction.results.len() != 1 {
                diagnostics.push(error("load must define one result".to_owned()));
            }
        }
        "load.borrow" => {
            let pointee = operands
                .first()
                .and_then(|operand| address_pointee(&operand.ty));
            if operands.len() != 1 || result.is_none() || pointee.is_none() {
                diagnostics.push(error(
                    "load.borrow requires one address and one result".to_owned(),
                ));
            } else if result != pointee {
                diagnostics.push(error(
                    "load.borrow result type must match its address pointee".to_owned(),
                ));
            } else if let Some(ty) = pointee
                && !policies.is_loadable(ty, layouts)
            {
                diagnostics.push(error(format!("load.borrow type {ty} is address-only")));
            }
            if instruction.results.len() != 1 {
                diagnostics.push(error("load.borrow must define one result".to_owned()));
            }
        }
        "store" => {
            let selected = selector(instruction).unwrap_or_else(|| "trivial".to_owned());
            let value_type = operands.first().map(|operand| &operand.ty);
            let pointee = operands
                .get(1)
                .and_then(|operand| address_pointee(&operand.ty));
            if operands.len() != 2 || value_type.is_none() || pointee.is_none() {
                diagnostics.push(error("store requires a value and an address".to_owned()));
            } else if value_type != pointee {
                diagnostics.push(error(
                    "store value type must match its address pointee".to_owned(),
                ));
            } else if let Some(ty) = pointee {
                if !policies.is_loadable(ty, layouts) {
                    diagnostics.push(error(format!("store type {ty} is address-only")));
                }
                if selected == "trivial" && !policies.is_trivial(ty) {
                    diagnostics.push(error(
                        "store [trivial] requires a type with trivial copy and destroy policies"
                            .to_owned(),
                    ));
                }
            }
            if !instruction.results.is_empty() {
                diagnostics.push(error("store must not define a result".to_owned()));
            }
        }
        "copy.addr" | "move.addr" => {
            let source = operands
                .first()
                .and_then(|operand| address_pointee(&operand.ty));
            let destination = operands
                .get(1)
                .and_then(|operand| address_pointee(&operand.ty));
            if operands.len() != 2 || source.is_none() || destination.is_none() {
                diagnostics.push(error(format!(
                    "{} requires source and destination addresses",
                    instruction.opcode
                )));
            } else if source != destination {
                diagnostics.push(error(format!(
                    "{} source and destination pointee types must match",
                    instruction.opcode
                )));
            } else if instruction.opcode == "copy.addr"
                && source.is_some_and(|ty| !policies.is_copyable(ty))
            {
                diagnostics.push(error(
                    "copy.addr requires a type with a copy policy".to_owned(),
                ));
            }
            if !instruction.results.is_empty() {
                diagnostics.push(error(format!(
                    "{} must not define a result",
                    instruction.opcode
                )));
            }
        }
        "destroy.addr" => {
            if operands.len() != 1 || address_pointee(&operands[0].ty).is_none() {
                diagnostics.push(error("destroy.addr requires one address".to_owned()));
            }
            if !instruction.results.is_empty() {
                diagnostics.push(error("destroy.addr must not define a result".to_owned()));
            }
        }
        "raw.load" => {
            let ty = leading_type(source, instruction);
            if operands.len() != 1
                || !matches!(
                    operands.first().map(|operand| &operand.ty),
                    Some(Type::Pointer(_))
                )
            {
                diagnostics.push(error("raw.load requires one pointer operand".to_owned()));
            }
            match ty.as_ref() {
                Some(ty) if !policies.is_trivial(ty) || layouts.layout(ty).is_err() => {
                    diagnostics.push(error(format!(
                        "raw.load type {ty} must be a trivial memory type"
                    )));
                }
                None => {
                    diagnostics.push(error("raw.load requires a valid explicit type".to_owned()))
                }
                _ => {}
            }
            if instruction.results.len() != 1 || result != ty.as_ref() {
                diagnostics.push(error(
                    "raw.load result must match its explicit type".to_owned(),
                ));
            }
        }
        "raw.store" => {
            if operands.len() != 2
                || !matches!(
                    operands.get(1).map(|operand| &operand.ty),
                    Some(Type::Pointer(_))
                )
            {
                diagnostics.push(error(
                    "raw.store requires a value and pointer operand".to_owned(),
                ));
            }
            if let Some(value) = operands.first()
                && (!policies.is_trivial(&value.ty) || layouts.layout(&value.ty).is_err())
            {
                diagnostics.push(error(format!(
                    "raw.store type {} must be a trivial memory type",
                    value.ty
                )));
            }
            if !instruction.results.is_empty() {
                diagnostics.push(error("raw.store must not define a result".to_owned()));
            }
        }
        "ptr.offset" => {
            if operands.len() != 2
                || !matches!(
                    operands.first().map(|operand| &operand.ty),
                    Some(Type::Pointer(_))
                )
                || operands
                    .get(1)
                    .is_none_or(|operand| operand.ty != Type::Integer(64))
                || result != operands.first().map(|operand| &operand.ty)
                || instruction.results.len() != 1
            {
                diagnostics.push(error(
                    "ptr.offset requires ptr, i64 and the same ptr result type".to_owned(),
                ));
            }
        }
        "ptr.diff" => {
            if operands.len() != 2
                || !matches!(
                    operands.first().map(|operand| &operand.ty),
                    Some(Type::Pointer(_))
                )
                || operands.first().map(|operand| &operand.ty)
                    != operands.get(1).map(|operand| &operand.ty)
                || result != Some(&Type::Integer(64))
                || instruction.results.len() != 1
            {
                diagnostics.push(error(
                    "ptr.diff requires equal pointer types and returns i64".to_owned(),
                ));
            }
        }
        "mem.copy" | "mem.move" => {
            if operands.len() != 3
                || operands
                    .first()
                    .is_none_or(|operand| !is_memory_pointer(&operand.ty))
                || operands
                    .get(1)
                    .is_none_or(|operand| !is_memory_pointer(&operand.ty))
                || operands
                    .get(2)
                    .is_none_or(|operand| operand.ty != Type::Integer(64))
            {
                diagnostics.push(error(format!(
                    "{} requires two memory pointers and an i64 byte count",
                    instruction.opcode
                )));
            }
            verify_no_result(instruction, &mut diagnostics, &error);
        }
        "mem.set" => {
            if operands.len() != 3
                || operands
                    .first()
                    .is_none_or(|operand| !is_memory_pointer(&operand.ty))
                || operands
                    .get(1)
                    .is_none_or(|operand| operand.ty != Type::Integer(8))
                || operands
                    .get(2)
                    .is_none_or(|operand| operand.ty != Type::Integer(64))
            {
                diagnostics.push(error(
                    "mem.set requires a memory pointer, i8 byte and i64 count".to_owned(),
                ));
            }
            verify_no_result(instruction, &mut diagnostics, &error);
        }
        "prefetch" => {
            if operands.len() != 1
                || operands
                    .first()
                    .is_none_or(|operand| !is_memory_pointer(&operand.ty))
            {
                diagnostics.push(error("prefetch requires one memory pointer".to_owned()));
            }
            verify_no_result(instruction, &mut diagnostics, &error);
        }
        _ => {}
    }
    diagnostics
}

fn verify_no_result(
    instruction: &TextInstruction,
    diagnostics: &mut Vec<Diagnostic>,
    error: &impl Fn(String) -> Diagnostic,
) {
    if !instruction.results.is_empty() {
        diagnostics.push(error(format!(
            "{} must not define a result",
            instruction.opcode
        )));
    }
}

fn selector(instruction: &TextInstruction) -> Option<String> {
    crate::text_modifiers::leading_bracket_modifiers(instruction)?
        .first()
        .and_then(|group| group.first())
        .cloned()
}

fn leading_type(source: &str, instruction: &TextInstruction) -> Option<Type> {
    let start_index = text_instruction_operands_start(&instruction.tokens);
    let start = instruction.tokens.get(start_index)?.span.start;
    let mut nesting = 0usize;
    let mut end = instruction.span.end;
    for token in &instruction.tokens[start_index..] {
        match token.kind {
            TokenKind::Punctuation(',') if nesting == 0 => {
                end = token.span.start;
                break;
            }
            TokenKind::Punctuation('<' | '(' | '[' | '{') => nesting += 1,
            TokenKind::Punctuation('>' | ')' | ']' | '}') => nesting = nesting.saturating_sub(1),
            _ => {}
        }
    }
    tir_format::parse_type(source.get(start..end)?.trim()).ok()
}

fn address_pointee(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Address { pointee, .. } => Some(pointee),
        _ => None,
    }
}

fn is_memory_pointer(ty: &Type) -> bool {
    matches!(ty, Type::Pointer(_) | Type::Address { .. })
}
