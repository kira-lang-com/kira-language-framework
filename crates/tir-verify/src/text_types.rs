use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind};
use tir_format::{FunctionBody, TextFunctionSignature, parse_type};
use tir_layout::LayoutEngine;
use tir_types::{Abi, AddressSpace, ClosureType, FunctionType, Type};

use crate::gpu_profile::policy::TypePolicies;
use crate::model::{Instruction, ValueDefinition, ValueUse};
use crate::ssa::{text_instruction_operands_start, text_value_uses};

mod address;
mod aggregate;
mod arity;
mod const_bytes;
mod heap;
mod inference;
mod loadable;
mod memory;
mod ownership;
use address::{
    infer_element_address, infer_enum_payload_address, infer_field_address, type_after_keyword,
};
pub(crate) use inference::explicit_type;
use inference::*;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify_function(
    source: &str,
    body: &FunctionBody,
    signature: Option<&TextFunctionSignature>,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
    call_conventions: &crate::signature_types::CallConventions,
    gpu_scope: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut value_types = HashMap::new();
    let mut null_values = HashSet::new();
    let mut borrowed_values = aggregate::borrowed_parameters(body);
    for block in &body.blocks {
        for parameter in &block.parameters {
            value_types
                .entry(parameter.name.clone())
                .or_insert_with(|| parameter.ty.clone());
        }
    }

    for block in &body.blocks {
        for instruction in &block.instructions {
            if instruction.opcode == "const.bytes" {
                diagnostics.extend(const_bytes::verify(source, instruction, layouts, policies));
            }
            if matches!(instruction.opcode.as_str(), "return" | "fail") {
                diagnostics.extend(verify_exit(instruction, signature, &value_types));
                continue;
            }
            if let Some(expected) = arity::expected_operand_count(instruction.opcode.as_str()) {
                let actual = instruction_operand_count(instruction);
                if actual != expected {
                    diagnostics.push(
                        Diagnostic::error(
                            V2,
                            format!(
                                "{} has {actual} SSA operands; expected {expected}",
                                instruction.opcode
                            ),
                        )
                        .with_primary_span(instruction.span),
                    );
                    continue;
                }
            }
            let symbol_result_type = if matches!(
                instruction.opcode.as_str(),
                "kernel.percpu.addr" | "foreign.addr" | "foreign.data" | "foreign.entry"
            ) {
                match call_conventions.symbol_address_result_type(instruction) {
                    Ok(ty) => Some(ty),
                    Err(message) => {
                        diagnostics.push(
                            Diagnostic::error(V2, message).with_primary_span(instruction.span),
                        );
                        None
                    }
                }
            } else if instruction.opcode == "func.ref" {
                match call_conventions.function_reference_result_type(source, instruction) {
                    Ok(ty) => Some(ty),
                    Err(message) => {
                        diagnostics.push(
                            Diagnostic::error(V2, message).with_primary_span(instruction.span),
                        );
                        None
                    }
                }
            } else {
                None
            };
            let Some(operand_types) = operand_types(instruction, &value_types) else {
                continue;
            };
            if instruction.opcode == "closure.new"
                && operand_types.get(1).is_some_and(|context| {
                    !matches!(&context.ty, Type::Unique(_)) && !null_values.contains(&context.name)
                })
            {
                diagnostics.push(
                    Diagnostic::error(
                        V2,
                        "closure.new context must be an owned unique value or null",
                    )
                    .with_primary_span(instruction.span),
                );
            }
            if instruction.opcode == "closure.callback"
                && !closure_callback_type(source, instruction).is_some_and(|(index, signature)| {
                    signature.abi == Abi::C
                        && signature.parameters.get(index).is_some_and(|parameter| {
                            parameter.ty == Type::Pointer(AddressSpace::Generic)
                        })
                })
            {
                diagnostics.push(
                    Diagnostic::error(
                        V2,
                        "closure.callback requires an abi(c) function type whose indexed parameter is ptr",
                    )
                    .with_primary_span(instruction.span),
                );
            }
            diagnostics.extend(address::verify_instruction(
                source,
                instruction,
                &operand_types,
                layouts,
                policies,
            ));
            let gpu_operand_types = operand_types
                .iter()
                .map(|operand| operand.ty.clone())
                .collect::<Vec<_>>();
            diagnostics.extend(crate::gpu_profile::instructions::verify(
                source,
                instruction,
                &gpu_operand_types,
            ));
            let inferred_result_types = symbol_result_type
                .or_else(|| direct_call_result_type(source, instruction, call_conventions))
                .map(|ty| vec![ty])
                .or_else(|| {
                    infer_result_types(source, instruction, &operand_types, layouts, policies)
                });
            diagnostics.extend(memory::verify_instruction(
                source,
                instruction,
                &operand_types,
                inferred_result_types.as_deref(),
                layouts,
                policies,
            ));
            diagnostics.extend(heap::verify_instruction(
                source,
                instruction,
                &operand_types,
                inferred_result_types.as_deref(),
                policies,
            ));
            diagnostics.extend(aggregate::verify_instruction(
                source,
                instruction,
                &operand_types,
                inferred_result_types.as_deref(),
                &borrowed_values,
                layouts,
                policies,
            ));
            if instruction.opcode.starts_with("vec.") {
                diagnostics.extend(crate::vector_text::verify(
                    source,
                    instruction,
                    &operand_types,
                    inferred_result_types.as_deref(),
                    instruction.results.len(),
                ));
            }
            let Some(result_types) = inferred_result_types else {
                continue;
            };
            let typed = Instruction {
                opcode: instruction.opcode.clone(),
                operands: operand_types,
                results: instruction
                    .results
                    .iter()
                    .enumerate()
                    .map(|(index, name)| ValueDefinition {
                        name: name.clone(),
                        ty: result_types
                            .get(index)
                            .or_else(|| result_types.last())
                            .cloned()
                            .unwrap_or(Type::Unit),
                        span: instruction.span,
                    })
                    .collect(),
                span: instruction.span,
            };
            diagnostics.extend(crate::instruction_types::verify_instruction(&typed));
            borrowed_values.extend(aggregate::borrowed_results(
                instruction,
                &result_types,
                &borrowed_values,
                policies,
            ));
            for (name, ty) in instruction.results.iter().zip(result_types) {
                if instruction.opcode == "const.null" && instruction.results.len() == 1 {
                    null_values.insert(name.clone());
                }
                value_types.insert(name.clone(), ty);
            }
        }
    }
    diagnostics.extend(crate::atomic_order::verify_function(
        body,
        gpu_scope,
        &value_types,
        source,
        layouts,
        policies,
    ));
    diagnostics.extend(ownership::verify(
        body,
        &borrowed_values,
        &value_types,
        policies,
        source,
        call_conventions,
    ));
    diagnostics.extend(loadable::verify(body, &value_types, layouts, policies));
    diagnostics
}

pub(crate) fn inferred_value_types(
    source: &str,
    body: &FunctionBody,
    layouts: &LayoutEngine,
    policies: &TypePolicies,
) -> HashMap<String, Type> {
    inferred_value_types_impl(source, body, layouts, policies, None)
}

pub(crate) fn inferred_value_types_with_calls(
    source: &str,
    body: &FunctionBody,
    layouts: &LayoutEngine,
    policies: &TypePolicies,
    call_conventions: &crate::signature_types::CallConventions,
) -> HashMap<String, Type> {
    inferred_value_types_impl(source, body, layouts, policies, Some(call_conventions))
}

fn inferred_value_types_impl(
    source: &str,
    body: &FunctionBody,
    layouts: &LayoutEngine,
    policies: &TypePolicies,
    call_conventions: Option<&crate::signature_types::CallConventions>,
) -> HashMap<String, Type> {
    let mut value_types = HashMap::new();
    for parameter in body.blocks.iter().flat_map(|block| &block.parameters) {
        value_types
            .entry(parameter.name.clone())
            .or_insert_with(|| parameter.ty.clone());
    }

    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        let uses = text_value_uses(instruction);
        let result_types = match instruction.opcode.as_str() {
            "func.ref" => call_conventions
                .and_then(|call_conventions| {
                    call_conventions
                        .function_reference_result_type(source, instruction)
                        .ok()
                })
                .map(|ty| vec![ty]),
            "kernel.percpu.addr" | "foreign.addr" | "foreign.data" | "foreign.entry" => {
                call_conventions
                    .and_then(|call_conventions| {
                        call_conventions
                            .symbol_address_result_type(instruction)
                            .ok()
                    })
                    .map(|ty| vec![ty])
            }
            "stack.alloc" => stack_alloc_result_type(source, instruction).map(|ty| vec![ty]),
            "atomic.load" | "raw.load" | "mmio.load" => {
                explicit_type(source, instruction).map(|ty| vec![ty])
            }
            "atomic.rmw" => uses
                .last()
                .and_then(|(name, _)| value_types.get(name))
                .cloned()
                .map(|ty| vec![ty]),
            "atomic.cmpxchg" => uses
                .last()
                .and_then(|(name, _)| value_types.get(name))
                .cloned()
                .map(|ty| vec![ty, Type::Integer(1)]),
            "load" | "load.borrow" => uses
                .first()
                .and_then(|(name, _)| value_types.get(name))
                .and_then(|ty| match ty {
                    Type::Address { pointee, .. } => Some(pointee.as_ref().clone()),
                    _ => None,
                })
                .map(|ty| vec![ty]),
            "closure.callback" => closure_callback_type(source, instruction)
                .map(|(_, signature)| vec![Type::Function(signature)]),
            "call.closure" => uses
                .first()
                .and_then(|(name, _)| value_types.get(name))
                .and_then(closure_call_result_type)
                .map(|ty| vec![ty]),
            "call.indirect" => crate::signature_types::indirect_call_type(source, instruction)
                .and_then(|(signature, _)| synchronous_call_result_type(&signature))
                .map(|ty| vec![ty]),
            "call" | "call.foreign" | "call.variadic" | "call.witness" | "call.vtable" => {
                call_conventions
                    .and_then(|conventions| {
                        direct_call_result_type(source, instruction, conventions)
                    })
                    .map(|ty| vec![ty])
            }
            _ => {
                let operands = uses
                    .iter()
                    .map(|(name, span)| {
                        value_types.get(name).cloned().map(|ty| ValueUse {
                            name: name.clone(),
                            ty,
                            span: *span,
                        })
                    })
                    .collect::<Option<Vec<_>>>();
                operands.and_then(|operands| {
                    infer_result_types(source, instruction, &operands, layouts, policies)
                })
            }
        };
        if let Some(result_types) = result_types {
            for (index, name) in instruction.results.iter().enumerate() {
                if let Some(ty) = result_types.get(index).or_else(|| result_types.last()) {
                    value_types.insert(name.clone(), ty.clone());
                }
            }
        }
    }
    value_types
}

fn stack_alloc_result_type(
    source: &str,
    instruction: &tir_format::TextInstruction,
) -> Option<Type> {
    let start = text_instruction_operands_start(&instruction.tokens);
    let first = instruction.tokens.get(start)?;
    let mut allocated = None;
    for token in instruction.tokens.iter().skip(start + 1) {
        if matches!(token.kind, TokenKind::Newline | TokenKind::End) {
            break;
        }
        let text = source.get(first.span.start..token.span.end)?;
        if let Ok(ty) = parse_type(text.trim()) {
            allocated = Some(ty);
        }
    }
    let space = instruction
        .tokens
        .windows(3)
        .find_map(|window| {
            (punctuation(&window[0], '<') && punctuation(&window[2], '>'))
                .then(|| atom_at(window, 1).and_then(address_space))
                .flatten()
        })
        .unwrap_or_else(|| default_stack_alloc_space(source));
    Some(Type::Address {
        pointee: Box::new(allocated?),
        space: Some(space),
    })
}

fn default_stack_alloc_space(source: &str) -> AddressSpace {
    let Ok(tokens) = tir_format::text::lex(source) else {
        return AddressSpace::Generic;
    };
    let header_end = tokens
        .iter()
        .position(|token| punctuation(token, '{'))
        .unwrap_or(tokens.len());
    let header = &tokens[..header_end];
    if matches!(
        header.first().and_then(|token| match &token.kind {
            TokenKind::Atom(name) => Some(name.as_str()),
            _ => None,
        }),
        Some("entry")
    ) {
        return AddressSpace::Thread;
    }
    let mut in_target = false;
    let mut targets_gpu = false;
    let mut targets_cpu = false;
    for token in header {
        match &token.kind {
            TokenKind::Atom(name) if name == "target" => in_target = true,
            TokenKind::Punctuation(')') => in_target = false,
            TokenKind::Atom(name) if in_target && name == "gpu" => targets_gpu = true,
            TokenKind::Atom(name) if in_target && name == "cpu" => targets_cpu = true,
            _ => {}
        }
    }
    if targets_gpu && !targets_cpu {
        AddressSpace::Thread
    } else {
        AddressSpace::Generic
    }
}

fn address_space(name: &str) -> Option<AddressSpace> {
    Some(match name {
        "generic" => AddressSpace::Generic,
        "device" => AddressSpace::Device,
        "constant" => AddressSpace::Constant,
        "threadgroup" => AddressSpace::Threadgroup,
        "thread" => AddressSpace::Thread,
        "object_payload" => AddressSpace::ObjectPayload,
        "ray_payload" => AddressSpace::RayPayload,
        "hit_attribute" => AddressSpace::HitAttribute,
        "tile" => AddressSpace::Tile,
        "node_record" => AddressSpace::NodeRecord,
        _ => return None,
    })
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

fn verify_exit(
    instruction: &tir_format::TextInstruction,
    signature: Option<&TextFunctionSignature>,
    value_types: &HashMap<String, Type>,
) -> Vec<Diagnostic> {
    let Some(signature) = signature else {
        return Vec::new();
    };
    let uses = text_value_uses(instruction);
    let expected = if instruction.opcode == "return" {
        Some(&signature.result)
    } else {
        signature.failure.as_ref()
    };
    let mut diagnostics = Vec::new();
    let Some(expected) = expected else {
        diagnostics.push(
            Diagnostic::error(
                V2,
                if instruction.opcode == "fail" {
                    "fail appears in a function without a failure type"
                } else {
                    "function result type is missing"
                },
            )
            .with_primary_span(instruction.span),
        );
        return diagnostics;
    };
    if instruction.opcode == "return" && matches!(expected, Type::Never) {
        diagnostics.push(
            Diagnostic::error(V2, "return appears in a never-result function")
                .with_primary_span(instruction.span),
        );
        return diagnostics;
    }
    let carries_value = !matches!(expected, Type::Unit);
    let expected_count = if carries_value { 1 } else { 0 };
    if uses.len() != expected_count {
        diagnostics.push(
            Diagnostic::error(
                V2,
                format!(
                    "{} carries {} values; function signature requires {expected_count}",
                    instruction.opcode,
                    uses.len()
                ),
            )
            .with_primary_span(instruction.span),
        );
        return diagnostics;
    }
    if let Some((name, span)) = uses.first()
        && let Some(actual) = value_types.get(name)
        && actual != expected
    {
        diagnostics.push(
            Diagnostic::error(
                V2,
                format!(
                    "{} value %{name} has type {actual}; function signature requires {expected}",
                    instruction.opcode
                ),
            )
            .with_primary_span(*span),
        );
    }
    diagnostics
}
