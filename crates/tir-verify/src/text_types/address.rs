use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind};
use tir_layout::{DefinitionKind, LayoutEngine};
use tir_types::Type;

use crate::gpu_profile::policy::TypePolicies;
use crate::model::ValueUse;
use crate::ssa::text_instruction_operands_start;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn type_after_keyword(
    source: &str,
    instruction: &TextInstruction,
    keyword: &str,
) -> Option<Type> {
    let start_index = text_instruction_operands_start(&instruction.tokens);
    let type_index = instruction.tokens[start_index..]
        .iter()
        .position(|token| matches!(&token.kind, TokenKind::Atom(atom) if atom == keyword))?
        + start_index;
    let start = instruction.tokens.get(type_index + 1)?.span.start;
    let mut nesting = 0usize;
    let mut end = instruction.span.end;
    for token in &instruction.tokens[type_index + 1..] {
        match token.kind {
            TokenKind::Punctuation(',' | '!') if nesting == 0 => {
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

pub(super) fn verify_instruction(
    source: &str,
    instruction: &TextInstruction,
    operands: &[ValueUse],
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let error = |message: &str| Diagnostic::error(V2, message).with_primary_span(instruction.span);
    let Some(address) = operands.first().map(|operand| &operand.ty) else {
        return Vec::new();
    };
    let mut diagnostics = Vec::new();
    match instruction.opcode.as_str() {
        "field.addr" => {
            let Some(index) = immediate_index(instruction) else {
                return vec![error(
                    "field.addr requires a non-negative integer field index",
                )];
            };
            let Type::Address { pointee, .. } = address else {
                return vec![error(
                    "field.addr requires an address to a tuple or nominal record",
                )];
            };
            let field_count = match pointee.as_ref() {
                Type::Tuple(fields) => fields.len(),
                Type::Nominal { path, arguments } => {
                    let Some(definition) = layouts.definition(path) else {
                        return vec![error("field.addr refers to an unknown nominal type")];
                    };
                    if definition.kind == DefinitionKind::Enum {
                        return vec![error("field.addr cannot select a field from an enum")];
                    }
                    if definition.parameters.len() != arguments.len() {
                        return vec![error("field.addr nominal type has the wrong generic arity")];
                    }
                    definition.fields.len()
                }
                _ => {
                    return vec![error(
                        "field.addr requires an address to a tuple or nominal record",
                    )];
                }
            };
            if index >= field_count {
                diagnostics.push(error("field.addr field index is out of range"));
            }
        }
        "elem.addr" => {
            if !matches!(address, Type::Address { pointee, .. } if matches!(pointee.as_ref(), Type::Array { .. }))
            {
                diagnostics.push(error("elem.addr requires an address to a fixed array"));
            }
            if operands
                .get(1)
                .is_some_and(|operand| !matches!(&operand.ty, Type::Integer(_)))
            {
                diagnostics.push(error("elem.addr index must have an integer type"));
            }
        }
        "enum.payload.addr" | "enum.inject" => {
            let Type::Address { pointee, .. } = address else {
                return vec![error(
                    "enum address operation requires an address to a nominal enum",
                )];
            };
            let Type::Nominal { path, arguments } = pointee.as_ref() else {
                return vec![error(
                    "enum address operation requires an address to a nominal enum",
                )];
            };
            let Some(definition) = layouts.definition(path) else {
                return vec![error("enum.payload.addr refers to an unknown nominal type")];
            };
            if definition.kind != DefinitionKind::Enum {
                return vec![error("enum.payload.addr requires an enum type")];
            }
            if definition.parameters.len() != arguments.len() {
                return vec![error(
                    "enum.payload.addr enum type has the wrong generic arity",
                )];
            }
            let variant_ty = Type::Nominal {
                path: path.clone(),
                arguments: arguments.clone(),
            };
            if enum_variant_index(instruction, &variant_ty, policies)
                .is_none_or(|index| index >= definition.variants.len())
            {
                diagnostics.push(error("enum address variant name/index is invalid"));
            }
        }
        "addr.cast" => {
            let Type::Address { pointee: from, .. } = address else {
                return vec![error("addr.cast requires an address operand")];
            };
            let Some(Type::Address { pointee: to, .. }) =
                type_after_keyword(source, instruction, "to")
            else {
                return vec![error("addr.cast target must be an address type")];
            };
            match (layouts.layout(from), layouts.layout(&to)) {
                (Ok(from_layout), Ok(to_layout)) if from_layout == to_layout => {}
                (Ok(_), Ok(_)) => {
                    diagnostics.push(error("addr.cast types are not layout-compatible"))
                }
                (Err(layout_error), _) | (_, Err(layout_error)) => diagnostics.push(error(
                    &format!("addr.cast type has no computable layout: {layout_error}"),
                )),
            }
        }
        "addr.toptr" => {
            if !matches!(address, Type::Address { .. }) {
                diagnostics.push(error("addr.toptr requires an address operand"));
            }
        }
        "ptr.toaddr" => {
            if !matches!(address, Type::Pointer(_)) {
                diagnostics.push(error("ptr.toaddr requires a pointer operand"));
            }
            if !matches!(
                type_after_keyword(source, instruction, "as"),
                Some(Type::Address { .. })
            ) {
                diagnostics.push(error("ptr.toaddr target must be an address type"));
            }
        }
        _ => {}
    }
    diagnostics
}

pub(super) fn infer_field_address(
    instruction: &TextInstruction,
    address: &Type,
    layouts: &LayoutEngine,
) -> Option<Type> {
    let Type::Address { pointee, space } = address else {
        return None;
    };
    let field_index = immediate_index(instruction)?;
    let field_type = match pointee.as_ref() {
        Type::Tuple(fields) => fields.get(field_index)?.clone(),
        Type::Nominal { path, arguments } => {
            let definition = layouts.definition(path)?;
            if definition.kind == DefinitionKind::Enum
                || definition.parameters.len() != arguments.len()
            {
                return None;
            }
            let field = definition.fields.get(field_index)?;
            let bindings = definition
                .parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            field.ty.substitute(&bindings)
        }
        _ => return None,
    };
    Some(Type::Address {
        pointee: Box::new(field_type),
        space: *space,
    })
}

pub(super) fn infer_element_address(address: &Type) -> Option<Type> {
    let Type::Address { pointee, space } = address else {
        return None;
    };
    let Type::Array { element, .. } = pointee.as_ref() else {
        return None;
    };
    Some(Type::Address {
        pointee: element.clone(),
        space: *space,
    })
}

pub(super) fn infer_enum_payload_address(
    instruction: &TextInstruction,
    address: &Type,
    layouts: &LayoutEngine,
    policies: &TypePolicies,
) -> Option<Type> {
    let Type::Address { pointee, space } = address else {
        return None;
    };
    let Type::Nominal { path, arguments } = pointee.as_ref() else {
        return None;
    };
    let definition = layouts.definition(path)?;
    if definition.kind != DefinitionKind::Enum || definition.parameters.len() != arguments.len() {
        return None;
    }
    let variant = definition
        .variants
        .get(enum_variant_index(instruction, pointee, policies)?)?;
    let bindings = definition
        .parameters
        .iter()
        .cloned()
        .zip(arguments.iter().cloned())
        .collect::<HashMap<_, _>>();
    let fields = variant
        .fields
        .iter()
        .map(|field| field.ty.substitute(&bindings))
        .collect::<Vec<_>>();
    let payload = match fields.as_slice() {
        [] => Type::Unit,
        [only] => only.clone(),
        _ => Type::Tuple(fields),
    };
    Some(Type::Address {
        pointee: Box::new(payload),
        space: *space,
    })
}

fn immediate_index(instruction: &TextInstruction) -> Option<usize> {
    let start = text_instruction_operands_start(&instruction.tokens);
    let comma = instruction.tokens[start..]
        .iter()
        .position(|token| punctuation(token, ','))?
        + start;
    let value = tir_format::numeric_literals::parse_unsigned_integer(atom_at(
        &instruction.tokens,
        comma + 1,
    )?)?;
    usize::try_from(value).ok()
}

fn enum_variant_index(
    instruction: &TextInstruction,
    ty: &Type,
    policies: &TypePolicies,
) -> Option<usize> {
    if let Some(index) = immediate_index(instruction) {
        return Some(index);
    }
    let start = text_instruction_operands_start(&instruction.tokens);
    let comma = instruction.tokens[start..]
        .iter()
        .position(|token| punctuation(token, ','))?
        + start;
    let name = atom_at(&instruction.tokens, comma + 1)?;
    policies.variant_index(ty, name)
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

fn atom_at(tokens: &[Token], index: usize) -> Option<&str> {
    match &tokens.get(index)?.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}
