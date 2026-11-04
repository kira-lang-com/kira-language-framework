use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind};
use tir_format::{FunctionBody, TextInstruction};
use tir_layout::{DefinitionKind, LayoutEngine};
use tir_types::{Convention, Type};

use crate::gpu_profile::policy::TypePolicies;
use crate::model::ValueUse;
use crate::ssa::text_instruction_operands_start;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn handles(opcode: &str) -> bool {
    matches!(
        opcode,
        "struct.new"
            | "struct.extract"
            | "struct.insert"
            | "struct.destructure"
            | "tuple.new"
            | "tuple.extract"
            | "tuple.destructure"
            | "array.new"
            | "array.extract"
            | "array.insert"
            | "enum.new"
            | "enum.tag"
            | "enum.take"
            | "enum.project"
    )
}

pub(super) fn infer_result_types(
    source: &str,
    instruction: &TextInstruction,
    operands: &[ValueUse],
    layouts: &LayoutEngine,
    policies: &TypePolicies,
) -> Option<Vec<Type>> {
    let opcode = instruction.opcode.as_str();
    let first = operands.first().map(|operand| operand.ty.clone());
    Some(match opcode {
        "struct.new" | "array.new" | "enum.new" => {
            vec![constructor_type(source, instruction, opcode)?]
        }
        "tuple.new" => vec![Type::Tuple(
            operands.iter().map(|operand| operand.ty.clone()).collect(),
        )],
        "struct.extract" => vec![
            record_fields(first.as_ref()?, layouts)?
                .get(immediate_index(instruction)?)?
                .clone(),
        ],
        "struct.insert" => vec![first.clone()?],
        "struct.destructure" => record_fields(first.as_ref()?, layouts)?,
        "tuple.extract" => {
            let Type::Tuple(fields) = first.as_ref()? else {
                return None;
            };
            vec![fields.get(immediate_index(instruction)?)?.clone()]
        }
        "tuple.destructure" => {
            let Type::Tuple(fields) = first.as_ref()? else {
                return None;
            };
            fields.clone()
        }
        "array.extract" => {
            let Type::Array { element, .. } = first.as_ref()? else {
                return None;
            };
            vec![element.as_ref().clone()]
        }
        "array.insert" => vec![first.clone()?],
        "enum.tag" => vec![enum_tag_type(first.as_ref()?, layouts)?],
        "enum.take" | "enum.project" => {
            vec![enum_payload_type(
                first.as_ref()?,
                instruction,
                source,
                layouts,
                policies,
            )?]
        }
        _ => return None,
    })
}

pub(super) fn verify_instruction(
    source: &str,
    instruction: &TextInstruction,
    operands: &[ValueUse],
    inferred_results: Option<&[Type]>,
    borrowed: &HashSet<String>,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    if !handles(instruction.opcode.as_str()) {
        return Vec::new();
    }
    let error =
        |message: String| Diagnostic::error(V2, message).with_primary_span(instruction.span);
    let Some(expected) = infer_result_types(source, instruction, operands, layouts, policies)
    else {
        return vec![error(format!(
            "{} has an invalid aggregate type, index, or variant",
            instruction.opcode
        ))];
    };
    let Some(inferred_results) = inferred_results else {
        return vec![error(format!(
            "{} result types could not be inferred",
            instruction.opcode
        ))];
    };
    if inferred_results != expected || expected.len() != instruction.results.len() {
        return vec![error(format!(
            "{} has {} results; aggregate shape requires {}",
            instruction.opcode,
            instruction.results.len(),
            expected.len()
        ))];
    }
    let uses = crate::ssa::text_value_uses(instruction);
    let source_borrowed = uses
        .first()
        .is_some_and(|(name, _)| borrowed.contains(name));
    let valid = match instruction.opcode.as_str() {
        "struct.new" => {
            let target = &expected[0];
            let Some(fields) = record_fields(target, layouts) else {
                return vec![error(
                    "struct.new requires a defined struct type".to_owned(),
                )];
            };
            operands.len() == fields.len()
                && fields
                    .iter()
                    .zip(operands)
                    .zip(&uses)
                    .all(|((field, value), (name, _))| {
                        field == &value.ty
                            && (policies.is_trivial(field) || !borrowed.contains(name))
                    })
                && policies.is_loadable(target, layouts)
        }
        "struct.extract" => {
            let field = &expected[0];
            operands.len() == 1 && (policies.is_trivial(field) || source_borrowed)
        }
        "struct.insert" => {
            let field = operands.first().and_then(|source| {
                record_fields(&source.ty, layouts)?
                    .get(immediate_index(instruction)?)
                    .cloned()
            });
            operands.len() == 2
                && field.is_some_and(|field| operands[1].ty == field)
                && operands
                    .first()
                    .is_some_and(|value| policies.is_trivial(&value.ty))
        }
        "struct.destructure" => operands.len() == 1 && !source_borrowed,
        "tuple.new" => {
            let Type::Tuple(fields) = &expected[0] else {
                return vec![error("tuple.new inferred a non-tuple result".to_owned())];
            };
            operands.len() == fields.len()
                && operands.iter().zip(&uses).all(|(value, (name, _))| {
                    policies.is_trivial(&value.ty) || !borrowed.contains(name)
                })
                && policies.is_loadable(&expected[0], layouts)
        }
        "tuple.extract" => {
            let field = &expected[0];
            operands.len() == 1 && (policies.is_trivial(field) || source_borrowed)
        }
        "tuple.destructure" => operands.len() == 1 && !source_borrowed,
        "array.new" => {
            let Type::Array { count, element } = &expected[0] else {
                return vec![error("array.new requires an array result type".to_owned())];
            };
            u64::try_from(operands.len()).ok() == Some(*count)
                && operands.iter().zip(&uses).all(|(value, (name, _))| {
                    value.ty == **element
                        && (policies.is_trivial(element) || !borrowed.contains(name))
                })
                && policies.is_loadable(&expected[0], layouts)
        }
        "array.extract" => {
            operands.len() == 2
                && matches!(
                    operands.get(1).map(|value| &value.ty),
                    Some(Type::Integer(_))
                )
                && policies.is_trivial(&expected[0])
        }
        "array.insert" => {
            let Some(source) = operands.first() else {
                return vec![error("array.insert requires an array source".to_owned())];
            };
            let Type::Array { element, .. } = &source.ty else {
                return vec![error("array.insert requires an array source".to_owned())];
            };
            operands.len() == 3
                && matches!(
                    operands.get(1).map(|value| &value.ty),
                    Some(Type::Integer(_))
                )
                && operands.get(2).is_some_and(|value| value.ty == **element)
                && policies.is_trivial(&source.ty)
        }
        "enum.new" => {
            let target = &expected[0];
            let Some(payload) = enum_payload_fields(target, instruction, source, layouts, policies)
            else {
                return vec![error("enum.new has an invalid variant payload".to_owned())];
            };
            operands.len() == payload.len()
                && payload
                    .iter()
                    .zip(operands)
                    .zip(&uses)
                    .all(|((field, value), (name, _))| {
                        field == &value.ty
                            && (policies.is_trivial(field) || !borrowed.contains(name))
                    })
                && policies.is_loadable(target, layouts)
        }
        "enum.tag" => operands.len() == 1,
        "enum.take" => operands.len() == 1 && !source_borrowed,
        "enum.project" => operands.len() == 1 && source_borrowed,
        _ => false,
    };
    if valid {
        Vec::new()
    } else {
        vec![error(format!(
            "{} has incompatible aggregate operands, result shape, or ownership",
            instruction.opcode
        ))]
    }
}

pub(super) fn borrowed_parameters(body: &FunctionBody) -> HashSet<String> {
    body.blocks
        .iter()
        .flat_map(|block| &block.parameters)
        .filter(|parameter| {
            matches!(
                parameter.convention,
                Some(Convention::Borrow | Convention::BorrowMut)
            )
        })
        .map(|parameter| parameter.name.clone())
        .collect()
}

pub(super) fn borrowed_results(
    instruction: &TextInstruction,
    result_types: &[Type],
    borrowed: &HashSet<String>,
    policies: &TypePolicies,
) -> Vec<String> {
    let source_is_borrowed = crate::ssa::text_value_uses(instruction)
        .first()
        .is_some_and(|(name, _)| borrowed.contains(name));
    let has_borrowed_operand = crate::ssa::text_value_uses(instruction)
        .iter()
        .any(|(name, _)| borrowed.contains(name));
    let borrowed_result = matches!(instruction.opcode.as_str(), "load.borrow" | "borrow.begin")
        || (source_is_borrowed
            && matches!(
                instruction.opcode.as_str(),
                "struct.extract" | "tuple.extract" | "enum.project"
            ))
        || (has_borrowed_operand && matches!(instruction.opcode.as_str(), "select" | "freeze"));
    if borrowed_result {
        instruction
            .results
            .iter()
            .zip(result_types)
            .filter(|(_, ty)| !policies.is_trivial(ty))
            .map(|(name, _)| name.clone())
            .collect()
    } else {
        Vec::new()
    }
}

fn constructor_type(source: &str, instruction: &TextInstruction, opcode: &str) -> Option<Type> {
    let tokens = &instruction.tokens;
    let start = text_instruction_operands_start(tokens);
    let delimiter = if opcode == "enum.new" {
        top_level_token(tokens, start, ',')?
    } else {
        top_level_token(tokens, start, '(')?
    };
    let first = tokens.get(start)?;
    tir_format::parse_type(
        source
            .get(first.span.start..tokens.get(delimiter)?.span.start)?
            .trim(),
    )
    .ok()
}

fn record_fields(ty: &Type, layouts: &LayoutEngine) -> Option<Vec<Type>> {
    let Type::Nominal { path, arguments } = ty else {
        return None;
    };
    let definition = layouts.definition(path)?;
    if definition.kind != DefinitionKind::Struct || definition.parameters.len() != arguments.len() {
        return None;
    }
    let bindings = definition
        .parameters
        .iter()
        .cloned()
        .zip(arguments.iter().cloned())
        .collect::<HashMap<_, _>>();
    Some(
        definition
            .fields
            .iter()
            .map(|field| field.ty.substitute(&bindings))
            .collect(),
    )
}

fn enum_payload_fields(
    ty: &Type,
    instruction: &TextInstruction,
    source: &str,
    layouts: &LayoutEngine,
    policies: &TypePolicies,
) -> Option<Vec<Type>> {
    let Type::Nominal { path, arguments } = ty else {
        return None;
    };
    let definition = layouts.definition(path)?;
    if definition.kind != DefinitionKind::Enum || definition.parameters.len() != arguments.len() {
        return None;
    }
    let variant = variant_index(ty, instruction, source, policies)?;
    let variant = definition.variants.get(variant)?;
    let bindings = definition
        .parameters
        .iter()
        .cloned()
        .zip(arguments.iter().cloned())
        .collect::<HashMap<_, _>>();
    Some(
        variant
            .fields
            .iter()
            .map(|field| field.ty.substitute(&bindings))
            .collect(),
    )
}

fn enum_payload_type(
    ty: &Type,
    instruction: &TextInstruction,
    source: &str,
    layouts: &LayoutEngine,
    policies: &TypePolicies,
) -> Option<Type> {
    Some(
        match enum_payload_fields(ty, instruction, source, layouts, policies)?.as_slice() {
            [] => Type::Unit,
            [only] => only.clone(),
            many => Type::Tuple(many.to_vec()),
        },
    )
}

fn enum_tag_type(ty: &Type, layouts: &LayoutEngine) -> Option<Type> {
    let Type::Nominal { path, arguments } = ty else {
        return None;
    };
    let definition = layouts.definition(path)?;
    if definition.kind != DefinitionKind::Enum || definition.parameters.len() != arguments.len() {
        return None;
    }
    if let Some(tag_type) = &definition.tag_type {
        return Some(tag_type.clone());
    }
    if matches!(definition.rule, tir_layout::LayoutRule::Explicit) {
        return None;
    }
    let width = match definition.variants.len() {
        0..=256 => 8,
        257..=65_536 => 16,
        _ => 32,
    };
    Some(Type::Integer(width))
}

fn variant_index(
    ty: &Type,
    instruction: &TextInstruction,
    source: &str,
    policies: &TypePolicies,
) -> Option<usize> {
    let tokens = &instruction.tokens;
    let comma = top_level_token(tokens, text_instruction_operands_start(tokens), ',')?;
    let first_index = (comma + 1..tokens.len())
        .find(|index| !matches!(tokens[*index].kind, TokenKind::Newline | TokenKind::End))?;
    if let Some(index) = atom_at(tokens, first_index)
        .and_then(tir_format::numeric_literals::parse_unsigned_integer)
        .and_then(|index| usize::try_from(index).ok())
    {
        return Some(index);
    }
    let end_index = (first_index..tokens.len())
        .find(|index| matches!(tokens[*index].kind, TokenKind::Punctuation('(' | ',')))
        .unwrap_or(tokens.len());
    let last = tokens.get(end_index.checked_sub(1)?)?;
    let name = source
        .get(tokens.get(first_index)?.span.start..last.span.end)?
        .trim();
    policies.variant_index(ty, name.rsplit('.').next().unwrap_or(name))
}

fn immediate_index(instruction: &TextInstruction) -> Option<usize> {
    let comma = top_level_token(
        &instruction.tokens,
        text_instruction_operands_start(&instruction.tokens),
        ',',
    )?;
    atom_at(&instruction.tokens, comma + 1)
        .and_then(tir_format::numeric_literals::parse_unsigned_integer)
        .and_then(|index| usize::try_from(index).ok())
}

fn top_level_token(tokens: &[Token], start: usize, expected: char) -> Option<usize> {
    let mut nesting = Vec::new();
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if matches!(token.kind, TokenKind::Newline | TokenKind::End) {
            return None;
        }
        if punctuation(token, expected) && nesting.is_empty() {
            return Some(index);
        }
        match &token.kind {
            TokenKind::Punctuation(character @ ('<' | '(' | '[' | '{')) => nesting.push(*character),
            TokenKind::Punctuation(character @ ('>' | ')' | ']' | '}')) => {
                let expected = match *character {
                    '>' => '<',
                    ')' => '(',
                    ']' => '[',
                    '}' => '{',
                    _ => continue,
                };
                if nesting.last() == Some(&expected) {
                    nesting.pop();
                }
            }
            _ => {}
        }
    }
    None
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
