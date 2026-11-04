use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind};
use tir_types::{AddressSpace, Type};

use crate::type_classes::{is_float, is_integer, is_memory_type};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Order {
    Relaxed,
    Acquire,
    Release,
    AcqRel,
    SeqCst,
}

pub(super) fn verify(
    instruction: &TextInstruction,
    operands: &[Type],
    explicit_type: Option<Type>,
    gpu_function: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = verify_order_scope(instruction, gpu_function);
    let ty = match instruction.opcode.as_str() {
        "atomic.load" => explicit_type.as_ref(),
        "atomic.store" => operands.first(),
        "atomic.rmw" => operands.get(1),
        "atomic.cmpxchg" => operands.get(1),
        _ => None,
    };
    if let Some(ty) = ty
        && (!is_memory_type(ty)
            || !crate::conversion_types::static_size(ty)
                .is_some_and(|size| matches!(size, 1 | 2 | 4 | 8 | 16)))
    {
        push(
            &mut diagnostics,
            instruction,
            format!(
                "{} type {ty} is not a supported atomic width",
                instruction.opcode
            ),
        );
    }
    if instruction.opcode == "atomic.rmw" {
        let operation = instruction
            .tokens
            .get(crate::calls::opcode_end(instruction))
            .and_then(atom);
        let valid = match operation {
            Some("xchg") => true,
            Some(
                "add" | "sub" | "and" | "nand" | "or" | "xor" | "max" | "min" | "umax" | "umin"
                | "uinc_wrap" | "udec_wrap" | "usub_cond" | "usub_sat",
            ) => operands
                .get(1)
                .is_some_and(|ty| matches!(ty, Type::Integer(_)) && is_integer(ty)),
            Some("fadd" | "fsub" | "fmax" | "fmin") => operands
                .get(1)
                .is_some_and(|ty| matches!(ty, Type::Float(_)) && is_float(ty)),
            _ => false,
        };
        if !valid {
            push(
                &mut diagnostics,
                instruction,
                "atomic.rmw operation is unknown or incompatible with its value type",
            );
        }
    }
    if gpu_function {
        let pointer = match instruction.opcode.as_str() {
            "atomic.load" | "atomic.rmw" | "atomic.cmpxchg" => operands.first(),
            "atomic.store" => operands.get(1),
            _ => None,
        };
        if pointer.is_some_and(|ty| {
            !matches!(
                ty,
                Type::Pointer(AddressSpace::Device | AddressSpace::Threadgroup)
            )
        }) {
            push(
                &mut diagnostics,
                instruction,
                "GPU atomics require a device or threadgroup pointer",
            );
        }
    }
    diagnostics
}

fn verify_order_scope(instruction: &TextInstruction, gpu_function: bool) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let (modifiers, errors) = parse_modifiers(instruction);
    for (span, message) in errors {
        push_at(&mut diagnostics, span, message);
    }
    let opcode = instruction.opcode.as_str();
    let allowed: &[&str] = match opcode {
        "atomic.load" | "atomic.store" | "atomic.rmw" | "fence" => &["order", "scope"],
        "atomic.cmpxchg" => &["weak", "success_order", "failure_order", "scope"],
        _ => &[],
    };
    for (name, value, span) in &modifiers {
        if !allowed.contains(&name.as_str()) {
            push_at(
                &mut diagnostics,
                *span,
                format!("{name} is not a modifier of {opcode}"),
            );
        }
        if name == "weak" && value.is_some() {
            push_at(&mut diagnostics, *span, "weak does not take a value");
        } else if name != "weak" && value.is_none() {
            push_at(&mut diagnostics, *span, format!("{name} requires a value"));
        }
    }
    let value = |name: &str| {
        modifiers
            .iter()
            .find(|(key, _, _)| key == name)
            .and_then(|(_, value, _)| value.as_deref())
    };
    let success_key = if opcode == "atomic.cmpxchg" {
        "success_order"
    } else {
        "order"
    };
    let success = match value(success_key) {
        Some(name) => match parse_order(name) {
            Some(order) => order,
            None => {
                push_at(
                    &mut diagnostics,
                    modifier_span(&modifiers, success_key, instruction.span),
                    format!("unknown atomic memory order '{name}'"),
                );
                Order::SeqCst
            }
        },
        None => Order::SeqCst,
    };
    if opcode == "atomic.load" && matches!(success, Order::Release | Order::AcqRel) {
        push(
            &mut diagnostics,
            instruction,
            "atomic.load cannot use release or acq_rel order",
        );
    }
    if opcode == "atomic.store" && matches!(success, Order::Acquire | Order::AcqRel) {
        push(
            &mut diagnostics,
            instruction,
            "atomic.store cannot use acquire or acq_rel order",
        );
    }
    if opcode == "atomic.cmpxchg" {
        let failure = match value("failure_order") {
            Some(name) => match parse_failure_order(name) {
                Some(order) => order,
                None => {
                    push_at(
                        &mut diagnostics,
                        modifier_span(&modifiers, "failure_order", instruction.span),
                        format!("invalid compare-exchange failure order '{name}'"),
                    );
                    Order::Relaxed
                }
            },
            None => derived_failure_order(success),
        };
        if !failure_is_valid(success, failure) {
            push(
                &mut diagnostics,
                instruction,
                "compare-exchange failure order is stronger than or incompatible with success order",
            );
        }
    }
    if let Some(scope) = value("scope") {
        let valid = match scope {
            "thread" | "system" => true,
            "subgroup" | "threadgroup" | "device" => gpu_function,
            _ => false,
        };
        if !valid {
            push_at(
                &mut diagnostics,
                modifier_span(&modifiers, "scope", instruction.span),
                if gpu_function {
                    format!("unknown atomic scope '{scope}'")
                } else {
                    format!("atomic scope '{scope}' is not valid for CPU code")
                },
            );
        }
    }
    diagnostics
}

fn parse_modifiers(
    instruction: &TextInstruction,
) -> (Vec<(String, Option<String>, Span)>, Vec<(Span, String)>) {
    let tokens = &instruction.tokens;
    let mut cursor = crate::calls::opcode_end(instruction);
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

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn push(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    push_at(diagnostics, instruction.span, message);
}

fn push_at(diagnostics: &mut Vec<Diagnostic>, span: Span, message: impl Into<String>) {
    diagnostics.push(Diagnostic::error(VFY3, message.into()).with_primary_span(span));
}
