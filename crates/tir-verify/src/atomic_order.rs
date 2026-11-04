use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{FunctionBody, TextInstruction, TextItemKind};
use tir_layout::LayoutEngine;
use tir_types::{AddressSpace, Type};

use crate::gpu_profile::policy::TypePolicies;
use crate::ssa::{text_instruction_operands_start, text_value_uses};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Order {
    Relaxed,
    Acquire,
    Release,
    AcqRel,
    SeqCst,
}

pub(super) fn gpu_scope_allowed(source: &str, kind: TextItemKind, gpu_target: bool) -> bool {
    if kind == TextItemKind::Entry {
        return true;
    }
    if !gpu_target {
        return false;
    }
    let Ok(tokens) = lex(source) else {
        return true;
    };
    let header_end = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('{')))
        .unwrap_or(tokens.len());
    let header = &tokens[..header_end];
    for (index, token) in header.iter().enumerate() {
        if !matches!(&token.kind, TokenKind::Atom(name) if name == "target") {
            continue;
        }
        let mut cursor = index + 1;
        if !matches!(
            header.get(cursor).map(|token| &token.kind),
            Some(TokenKind::Punctuation('('))
        ) {
            continue;
        }
        cursor += 1;
        let mut has_gpu = false;
        let mut has_cpu = false;
        while cursor < header.len() && !matches!(header[cursor].kind, TokenKind::Punctuation(')')) {
            match &header[cursor].kind {
                TokenKind::Atom(name) if name == "gpu" => has_gpu = true,
                TokenKind::Atom(name) if name == "cpu" => has_cpu = true,
                _ => {}
            }
            cursor += 1;
        }
        return has_gpu && !has_cpu;
    }
    gpu_target
}

pub(super) fn verify_function(
    body: &FunctionBody,
    gpu_scope: bool,
    value_types: &HashMap<String, Type>,
    source: &str,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if !instruction.opcode.starts_with("atomic.") && instruction.opcode != "fence" {
            continue;
        }
        verify_instruction(
            source,
            instruction,
            gpu_scope,
            value_types,
            layouts,
            policies,
            &mut diagnostics,
        );
    }
    diagnostics
}

fn verify_instruction(
    source: &str,
    instruction: &TextInstruction,
    gpu_scope: bool,
    value_types: &HashMap<String, Type>,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let opcode = instruction.opcode.as_str();
    let (modifiers, parse_errors) = parse_modifiers(instruction);
    for (span, message) in parse_errors {
        error_at(diagnostics, span, message);
    }
    let allowed: &[&str] = match opcode {
        "atomic.load" | "atomic.store" | "atomic.rmw" | "fence" => &["order", "scope"],
        "atomic.cmpxchg" => &["weak", "success_order", "failure_order", "scope"],
        _ => &[],
    };
    for (name, value, span) in &modifiers {
        if !allowed.contains(&name.as_str()) {
            error_at(
                diagnostics,
                *span,
                format!("{name} is not a modifier of {opcode}"),
            );
        }
        if name == "weak" && value.is_some() {
            error_at(diagnostics, *span, "weak does not take a value");
        } else if name != "weak" && value.is_none() {
            error_at(diagnostics, *span, format!("{name} requires a value"));
        }
    }

    let value = |name: &str| {
        modifiers
            .iter()
            .find(|(key, _, _)| key == name)
            .and_then(|(_, value, _)| value.as_deref())
    };
    let success = match value(if opcode == "atomic.cmpxchg" {
        "success_order"
    } else {
        "order"
    }) {
        Some(name) => match parse_order(name) {
            Some(order) => order,
            None => {
                error_at(
                    diagnostics,
                    modifier_span(
                        &modifiers,
                        if opcode == "atomic.cmpxchg" {
                            "success_order"
                        } else {
                            "order"
                        },
                        instruction.span,
                    ),
                    format!("unknown atomic memory order '{name}'"),
                );
                Order::SeqCst
            }
        },
        None => Order::SeqCst,
    };
    if opcode == "atomic.load" && matches!(success, Order::Release | Order::AcqRel) {
        error(
            diagnostics,
            instruction,
            "atomic.load cannot use release or acq_rel order",
        );
    }
    if opcode == "atomic.store" && matches!(success, Order::Acquire | Order::AcqRel) {
        error(
            diagnostics,
            instruction,
            "atomic.store cannot use acquire or acq_rel order",
        );
    }
    if opcode == "atomic.cmpxchg" {
        let failure = match value("failure_order") {
            Some(name) => match parse_failure_order(name) {
                Some(order) => order,
                None => {
                    error_at(
                        diagnostics,
                        modifier_span(&modifiers, "failure_order", instruction.span),
                        format!("invalid compare-exchange failure order '{name}'"),
                    );
                    Order::Relaxed
                }
            },
            None => derived_failure_order(success),
        };
        if !failure_is_valid(success, failure) {
            error(
                diagnostics,
                instruction,
                "compare-exchange failure order is stronger than or incompatible with success order",
            );
        }
    }
    if let Some(scope) = value("scope") {
        let valid = match scope {
            "thread" => true,
            "subgroup" | "threadgroup" | "device" => gpu_scope,
            "system" => true,
            _ => false,
        };
        if !valid {
            error_at(
                diagnostics,
                modifier_span(&modifiers, "scope", instruction.span),
                if gpu_scope {
                    format!("unknown atomic scope '{scope}'")
                } else {
                    format!("atomic scope '{scope}' is not valid for CPU code")
                },
            );
        }
    }
    verify_gpu_pointer(instruction, gpu_scope, value_types, diagnostics);
    verify_atomic_types(
        source,
        instruction,
        value_types,
        layouts,
        policies,
        diagnostics,
    );
}

fn verify_atomic_types(
    source: &str,
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let opcode = instruction.opcode.as_str();
    if matches!(opcode, "atomic.wait" | "atomic.notify") {
        verify_wait_notify(instruction, value_types, diagnostics);
        return;
    }
    if !matches!(
        opcode,
        "atomic.load" | "atomic.store" | "atomic.rmw" | "atomic.cmpxchg"
    ) {
        return;
    }
    let uses = text_value_uses(instruction);
    let pointer_index = if opcode == "atomic.store" { 1 } else { 0 };
    let pointer_type = uses
        .get(pointer_index)
        .and_then(|(name, _)| value_types.get(name));
    let value_type = match opcode {
        "atomic.load" => crate::text_types::explicit_type(source, instruction),
        "atomic.store" => uses
            .first()
            .and_then(|(name, _)| value_types.get(name))
            .cloned(),
        "atomic.rmw" | "atomic.cmpxchg" => uses
            .get(1)
            .and_then(|(name, _)| value_types.get(name))
            .cloned(),
        _ => None,
    };
    let Some(value_type) = value_type else {
        return;
    };
    if !is_atomic_memory_type(&value_type) {
        error(
            diagnostics,
            instruction,
            format!("{opcode} value type {value_type} is not an atomic memory type"),
        );
    }
    if !policies.is_trivial(&value_type) {
        error(
            diagnostics,
            instruction,
            format!("{opcode} value type {value_type} is not trivially copyable"),
        );
    }
    match layouts.layout(&value_type) {
        Ok(layout) if matches!(layout.size, 1 | 2 | 4 | 8 | 16) => {}
        Ok(layout) => error(
            diagnostics,
            instruction,
            format!(
                "{opcode} value type {value_type} has unsupported atomic width {} bytes",
                layout.size
            ),
        ),
        Err(layout_error) => error(
            diagnostics,
            instruction,
            format!("{opcode} value type {value_type} has no atomic layout: {layout_error}"),
        ),
    }
    if let Some(pointer_type) = pointer_type {
        match pointer_type {
            Type::Pointer(_) => {}
            Type::Address { pointee, .. } if pointee.as_ref() == &value_type => {}
            Type::Address { pointee, .. } => error(
                diagnostics,
                instruction,
                format!("{opcode} pointer addresses {pointee}, not {value_type}"),
            ),
            _ => error(
                diagnostics,
                instruction,
                format!("{opcode} operand {} is not a pointer", pointer_index + 1),
            ),
        }
    }
    if opcode == "atomic.cmpxchg"
        && uses
            .get(2)
            .and_then(|(name, _)| value_types.get(name))
            .is_some_and(|new_type| new_type != &value_type)
    {
        error(
            diagnostics,
            instruction,
            "atomic.cmpxchg expected and new values must have the same type",
        );
    }
    if opcode == "atomic.rmw" {
        let operation = instruction
            .tokens
            .get(text_instruction_operands_start(&instruction.tokens))
            .and_then(|token| match &token.kind {
                TokenKind::Atom(name) => Some(name.as_str()),
                _ => None,
            });
        let integer_op = matches!(
            operation,
            Some(
                "add"
                    | "sub"
                    | "and"
                    | "nand"
                    | "or"
                    | "xor"
                    | "max"
                    | "min"
                    | "umax"
                    | "umin"
                    | "uinc_wrap"
                    | "udec_wrap"
                    | "usub_cond"
                    | "usub_sat"
            )
        );
        let float_op = matches!(operation, Some("fadd" | "fsub" | "fmax" | "fmin"));
        if integer_op && !has_integer_elements(&value_type)
            || float_op && !has_float_elements(&value_type)
        {
            error(
                diagnostics,
                instruction,
                "atomic.rmw operation is incompatible with its value type",
            );
        }
    }
}

fn is_atomic_memory_type(ty: &Type) -> bool {
    !matches!(
        ty,
        Type::Integer(1)
            | Type::Code
            | Type::Unit
            | Type::Never
            | Type::ScalableVector(_)
            | Type::Reference(_)
            | Type::Unique(_)
            | Type::Weak(_)
            | Type::Address { .. }
            | Type::Function(_)
            | Type::Closure(_)
            | Type::Coroutine { .. }
            | Type::Frame(_)
            | Type::RayQuery
            | Type::HitObject
            | Type::CooperativeMatrix { .. }
            | Type::Entry { .. }
    )
}

fn has_integer_elements(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_))
}

fn has_float_elements(ty: &Type) -> bool {
    matches!(ty, Type::Float(_))
}

fn verify_wait_notify(
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let uses = text_value_uses(instruction);
    let pointer = uses.first().and_then(|(name, _)| value_types.get(name));
    let pointee = match pointer {
        Some(Type::Pointer(_)) => None,
        Some(Type::Address { pointee, .. }) => Some(pointee.as_ref()),
        Some(_) => {
            error(
                diagnostics,
                instruction,
                format!("{} operand 1 is not a pointer", instruction.opcode),
            );
            None
        }
        None => None,
    };
    let valid_word = |ty: &Type| matches!(ty, Type::Integer(32 | 64));
    if let Some(pointee) = pointee
        && !valid_word(pointee)
    {
        error(
            diagnostics,
            instruction,
            format!("{} pointer must address i32 or i64", instruction.opcode),
        );
    }
    if instruction.opcode == "atomic.wait" {
        if let Some(expected) = uses.get(1).and_then(|(name, _)| value_types.get(name)) {
            if !valid_word(expected) || pointee.is_some_and(|pointee| pointee != expected) {
                error(
                    diagnostics,
                    instruction,
                    "atomic.wait expected value must match its i32 or i64 pointer",
                );
            }
        }
        if uses
            .get(2)
            .and_then(|(name, _)| value_types.get(name))
            .is_some_and(|timeout| timeout != &Type::Integer(64))
        {
            error(
                diagnostics,
                instruction,
                "atomic.wait timeout must have type i64",
            );
        }
    } else if uses
        .get(1)
        .and_then(|(name, _)| value_types.get(name))
        .is_some_and(|count| count != &Type::Integer(32))
    {
        error(
            diagnostics,
            instruction,
            "atomic.notify count must have type i32",
        );
    }
}

fn verify_gpu_pointer(
    instruction: &TextInstruction,
    gpu_scope: bool,
    value_types: &HashMap<String, Type>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !gpu_scope {
        return;
    }
    let index = match instruction.opcode.as_str() {
        "atomic.load" => Some(0),
        "atomic.store" => Some(1),
        "atomic.rmw" | "atomic.cmpxchg" => Some(0),
        _ => None,
    };
    let uses = text_value_uses(instruction);
    let Some((name, _)) = index.and_then(|index| uses.get(index)) else {
        return;
    };
    let Some(ty) = value_types.get(name) else {
        return;
    };
    let space = match ty {
        Type::Pointer(space) => Some(*space),
        Type::Address { space, .. } => *space,
        _ => None,
    };
    if !matches!(
        space,
        Some(AddressSpace::Device | AddressSpace::Threadgroup)
    ) {
        error(
            diagnostics,
            instruction,
            "GPU atomics require a device or threadgroup pointer",
        );
    }
}

fn parse_modifiers(
    instruction: &TextInstruction,
) -> (Vec<(String, Option<String>, Span)>, Vec<(Span, String)>) {
    let tokens = &instruction.tokens;
    let mut cursor = text_instruction_operands_start(tokens);
    if instruction.opcode == "atomic.rmw" {
        cursor = cursor.saturating_add(1);
    }
    let mut modifiers = Vec::new();
    let mut errors = Vec::new();
    while matches!(
        tokens.get(cursor).map(|token| &token.kind),
        Some(TokenKind::Punctuation('['))
    ) {
        let open = tokens[cursor].span;
        let start = cursor + 1;
        let Some(relative_end) = tokens[start..]
            .iter()
            .position(|token| matches!(token.kind, TokenKind::Punctuation(']')))
        else {
            errors.push((open, "unterminated atomic modifier".to_owned()));
            break;
        };
        let end = start + relative_end;
        let contents = &tokens[start..end];
        let parsed = match contents {
            [
                Token {
                    kind: TokenKind::Atom(name),
                    span,
                },
            ] => Some((name.clone(), None, *span)),
            [
                Token {
                    kind: TokenKind::Atom(name),
                    ..
                },
                Token {
                    kind: TokenKind::Punctuation('='),
                    ..
                },
                Token {
                    kind: TokenKind::Atom(value),
                    span,
                },
            ] => Some((name.clone(), Some(value.clone()), *span)),
            _ => None,
        };
        if let Some(modifier) = parsed {
            if modifiers.iter().any(|(name, _, _)| name == &modifier.0) {
                errors.push((
                    modifier.2,
                    format!("atomic modifier '{}' is repeated", modifier.0),
                ));
            } else {
                modifiers.push(modifier);
            }
        } else {
            errors.push((open, "malformed atomic modifier".to_owned()));
        }
        cursor = end + 1;
    }
    (modifiers, errors)
}

fn parse_order(name: &str) -> Option<Order> {
    match name {
        "relaxed" => Some(Order::Relaxed),
        "acquire" => Some(Order::Acquire),
        "release" => Some(Order::Release),
        "acq_rel" => Some(Order::AcqRel),
        "seq_cst" => Some(Order::SeqCst),
        _ => None,
    }
}

fn parse_failure_order(name: &str) -> Option<Order> {
    match name {
        "relaxed" => Some(Order::Relaxed),
        "acquire" => Some(Order::Acquire),
        "seq_cst" => Some(Order::SeqCst),
        _ => None,
    }
}

fn derived_failure_order(success: Order) -> Order {
    match success {
        Order::Relaxed | Order::Release => Order::Relaxed,
        Order::Acquire | Order::AcqRel => Order::Acquire,
        Order::SeqCst => Order::SeqCst,
    }
}

fn failure_is_valid(success: Order, failure: Order) -> bool {
    match failure {
        Order::Relaxed => true,
        Order::Acquire => matches!(success, Order::Acquire | Order::AcqRel | Order::SeqCst),
        Order::SeqCst => success == Order::SeqCst,
        Order::Release | Order::AcqRel => false,
    }
}

fn modifier_span(modifiers: &[(String, Option<String>, Span)], key: &str, fallback: Span) -> Span {
    modifiers
        .iter()
        .find(|(name, _, _)| name == key)
        .map_or(fallback, |(_, _, span)| *span)
}

fn error(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    error_at(diagnostics, instruction.span, message);
}

fn error_at(diagnostics: &mut Vec<Diagnostic>, span: Span, message: impl Into<String>) {
    diagnostics.push(Diagnostic::error(V2, message.into()).with_primary_span(span));
}
