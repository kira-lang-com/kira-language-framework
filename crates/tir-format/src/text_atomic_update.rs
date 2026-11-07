//! Restricted CPU atomic RMW and compare-exchange CODE (0x202–0x203).
//! RMW selectors 16–18 cannot fit the normative 4-bit selector field and are
//! left unsupported until the 22.8 selector-width conflict is resolved.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum AtomicUpdate {
    Rmw {
        result: String,
        operation: u64,
        pointer: String,
        value: String,
        flags: u64,
    },
    CmpXchg {
        result: String,
        pointer: String,
        expected: String,
        next: String,
        flags: u64,
    },
}

pub(crate) fn handles(op: &str) -> bool {
    matches!(op, "atomic.rmw" | "atomic.cmpxchg")
}

const RMW: [&str; 16] = [
    "xchg",
    "add",
    "sub",
    "and",
    "nand",
    "or",
    "xor",
    "max",
    "min",
    "umax",
    "umin",
    "fadd",
    "fsub",
    "fmax",
    "fmin",
    "uinc_wrap",
];

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<AtomicUpdate, TextParseError> {
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "atomic update must define one SSA result",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "atomic update source span is invalid",
            )
        })?
        .trim();
    let prefix = format!("%{result} = {} ", instruction.opcode);
    let mut rest = line
        .strip_prefix(&prefix)
        .ok_or_else(|| error(item, instruction.span, "atomic update syntax is invalid"))?;
    let mut selector = 0u64;
    if instruction.opcode == "atomic.rmw" {
        let (name, trailing) = rest.split_once(' ').ok_or_else(|| {
            error(
                item,
                instruction.span,
                "atomic.rmw requires an operation name",
            )
        })?;
        selector = RMW
            .iter()
            .position(|candidate| candidate == &name)
            .ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "RMW selector is unknown or cannot fit four bits",
                )
            })? as u64;
        rest = trailing.trim();
    }
    let mut order = 0;
    let mut failure = 0;
    let mut scope = 0;
    let mut weak = false;
    let mut saw_order = false;
    let mut saw_failure = false;
    let mut saw_scope = false;
    while let Some(stripped) = rest.strip_prefix('[') {
        let (option, trailing) = stripped.split_once(']').ok_or_else(|| {
            error(
                item,
                instruction.span,
                "unterminated atomic update modifier",
            )
        })?;
        if option == "weak" && instruction.opcode == "atomic.cmpxchg" && !weak {
            weak = true;
        } else if let Some(mode) = option
            .strip_prefix("order=")
            .filter(|_| instruction.opcode == "atomic.rmw" && !saw_order)
            .or_else(|| {
                option
                    .strip_prefix("success_order=")
                    .filter(|_| instruction.opcode == "atomic.cmpxchg" && !saw_order)
            })
        {
            order = parse_order(mode)
                .ok_or_else(|| error(item, instruction.span, "invalid RMW success order"))?;
            saw_order = true;
        } else if let Some(mode) = option
            .strip_prefix("failure_order=")
            .filter(|_| instruction.opcode == "atomic.cmpxchg" && !saw_failure)
        {
            failure = match mode {
                "relaxed" => 1,
                "acquire" => 2,
                "seq_cst" => 5,
                _ => {
                    return Err(error(
                        item,
                        instruction.span,
                        "compare-exchange failure order is invalid",
                    ));
                }
            };
            saw_failure = true;
        } else if let Some(mode) = option.strip_prefix("scope=").filter(|_| !saw_scope) {
            scope = match mode {
                "thread" => 1,
                "system" => 5,
                _ => {
                    return Err(error(
                        item,
                        instruction.span,
                        "CPU atomic scope must be thread/system",
                    ));
                }
            };
            saw_scope = true;
        } else {
            return Err(error(
                item,
                instruction.span,
                "invalid or duplicate atomic update modifier",
            ));
        }
        if !trailing.is_empty() && !trailing.starts_with(' ') {
            return Err(error(
                item,
                instruction.span,
                "atomic modifier spacing is invalid",
            ));
        }
        rest = trailing.trim();
    }
    if instruction.opcode == "atomic.cmpxchg" && !valid_failure_order(order, failure) {
        return Err(error(
            item,
            instruction.span,
            "failure order is stronger than success order",
        ));
    }
    let args = rest
        .split(',')
        .map(|name| {
            let name = name
                .trim()
                .strip_prefix('%')
                .filter(|name| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
                })
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "atomic update argument is not an SSA value",
                    )
                })?;
            Ok(name.to_owned())
        })
        .collect::<Result<Vec<_>, TextParseError>>()?;
    let flags = (selector << 48)
        | (order << 20)
        | (failure << 23)
        | (scope << 26)
        | (u64::from(weak) << 29);
    match (instruction.opcode.as_str(), args.as_slice()) {
        ("atomic.rmw", [pointer, value]) => Ok(AtomicUpdate::Rmw {
            result: result.clone(),
            operation: selector,
            pointer: pointer.clone(),
            value: value.clone(),
            flags,
        }),
        ("atomic.cmpxchg", [pointer, expected, next]) => Ok(AtomicUpdate::CmpXchg {
            result: result.clone(),
            pointer: pointer.clone(),
            expected: expected.clone(),
            next: next.clone(),
            flags,
        }),
        _ => Err(error(
            item,
            instruction.span,
            "wrong atomic update operand count",
        )),
    }
}

fn parse_order(name: &str) -> Option<u64> {
    Some(match name {
        "relaxed" => 1,
        "acquire" => 2,
        "release" => 3,
        "acq_rel" => 4,
        "seq_cst" => 5,
        _ => return None,
    })
}

fn valid_failure_order(order: u64, failure: u64) -> bool {
    let success = if order == 0 { 5 } else { order };
    let failure = if failure == 0 {
        match success {
            1 | 3 => 1,
            2 | 4 => 2,
            5 => 5,
            _ => return false,
        }
    } else {
        failure
    };
    match success {
        1 | 3 => failure == 1,
        2 | 4 => matches!(failure, 1 | 2),
        5 => matches!(failure, 1 | 2 | 5),
        _ => false,
    }
}

fn supported_scalar(ty: &Type) -> bool {
    match ty {
        Type::Integer(bits) => matches!(bits, 8 | 16 | 32 | 64 | 128),
        Type::Float(float) => matches!(float.bits(), 16 | 32 | 64),
        _ => false,
    }
}

pub(crate) fn validate(
    op: &AtomicUpdate,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    let get = |name: &str| {
        values
            .get(name)
            .ok_or_else(|| error(item, span, "atomic update uses an undefined SSA value"))
    };
    match op {
        AtomicUpdate::Rmw {
            result,
            operation,
            pointer,
            value,
            ..
        } => {
            let ty = get(value)?;
            let applicable = if *operation == 0 {
                supported_scalar(ty)
            } else if matches!(operation, 11..=14) {
                matches!(ty, Type::Float(_)) && supported_scalar(ty)
            } else {
                matches!(ty, Type::Integer(_)) && supported_scalar(ty)
            };
            if !applicable || !matches!(get(pointer)?, Type::Pointer(_)) {
                return Err(error(
                    item,
                    span,
                    "RMW operation requires a compatible scalar and pointer",
                ));
            }
            Ok((result.clone(), ty.clone()))
        }
        AtomicUpdate::CmpXchg {
            result,
            pointer,
            expected,
            next,
            ..
        } => {
            let ty = get(expected)?;
            if !supported_scalar(ty)
                || ty != get(next)?
                || !matches!(get(pointer)?, Type::Pointer(_))
            {
                return Err(error(
                    item,
                    span,
                    "compare-exchange requires pointer and matching scalar arguments",
                ));
            }
            Ok((
                result.clone(),
                Type::Tuple(vec![ty.clone(), Type::Integer(1)]),
            ))
        }
    }
}

pub(crate) fn encode(
    op: &AtomicUpdate,
    values: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let name_index = |name: &str| {
        values
            .iter()
            .position(|value| *value == name)
            .map(|position| (values.len() - position) as u64)
            .ok_or_else(|| error(item, span, "atomic update SSA operand is undefined"))
    };
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match op {
        AtomicUpdate::Rmw {
            pointer,
            value,
            flags,
            ..
        } => {
            write(0x202)?;
            write(*flags)?;
            write(1)?;
            write(name_index(pointer)?)?;
            write(name_index(value)?)?;
        }
        AtomicUpdate::CmpXchg {
            pointer,
            expected,
            next,
            flags,
            ..
        } => {
            write(0x203)?;
            write(*flags)?;
            write(1)?;
            for name in [pointer, expected, next] {
                write(name_index(name)?)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.result_count != 1 {
        return Err(malformed("atomic update must define one result"));
    }
    let (selector, op) = if instruction.opcode == 0x202 {
        let selector = (instruction.modifiers >> 48) & 15;
        (
            selector,
            RMW.get(selector as usize)
                .copied()
                .ok_or_else(|| malformed("invalid RMW selector"))?,
        )
    } else {
        (0, "")
    };
    let allowed = if instruction.opcode == 0x202 {
        (0xf << 48) | (0x7 << 20) | (0x7 << 26)
    } else {
        (0x7 << 20) | (0x7 << 23) | (0x7 << 26) | (1 << 29)
    };
    if instruction.modifiers & !allowed != 0 {
        return Err(malformed("unsupported atomic update modifier bits"));
    }
    let order = (instruction.modifiers >> 20) & 7;
    let failure = (instruction.modifiers >> 23) & 7;
    let scope = (instruction.modifiers >> 26) & 7;
    if order > 5 || !matches!(scope, 0 | 1 | 5) {
        return Err(malformed("invalid CPU atomic update order or scope"));
    }
    if instruction.opcode == 0x203
        && (!matches!(failure, 0 | 1 | 2 | 5) || !valid_failure_order(order, failure))
    {
        return Err(malformed("compare-exchange has illegal failure order"));
    }
    let args = match instruction.operands.as_slice() {
        [BinaryOperand::Values(args)]
            if args.len() == if instruction.opcode == 0x202 { 2 } else { 3 } =>
        {
            args
        }
        _ => return Err(malformed("atomic update operand count is invalid")),
    };
    let inputs = args
        .iter()
        .map(|index| {
            usize::try_from(*index)
                .ok()
                .and_then(|index| values.get(index))
                .map(|(name, ty)| (name.clone(), *ty))
                .ok_or_else(|| malformed("atomic update SSA index out of range"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !matches!(
        types.entries.get(inputs[0].1 as usize),
        Some(BinaryType::Pointer(_))
    ) {
        return Err(malformed("atomic update requires a pointer"));
    }
    let ty = inputs[1].1;
    let kind = types
        .entries
        .get(ty as usize)
        .ok_or_else(|| malformed("atomic update type index out of range"))?;
    let supported = matches!(kind, BinaryType::Integer(8 | 16 | 32 | 64 | 128))
        || matches!(kind, BinaryType::Float(float) if matches!(float.bits(), 16 | 32 | 64));
    if !supported {
        return Err(malformed("atomic update has unsupported scalar type"));
    }
    if instruction.opcode == 0x202
        && (selector != 0 && matches!(selector, 11..=14) != matches!(kind, BinaryType::Float(_)))
    {
        return Err(malformed("RMW selector is incompatible with value type"));
    }
    if instruction.opcode == 0x203 && inputs[2].1 != ty {
        return Err(malformed("compare-exchange expected and new type disagree"));
    }
    let mut modifiers = String::new();
    if instruction.opcode == 0x203 && instruction.modifiers & (1 << 29) != 0 {
        modifiers.push_str(" [weak]");
    }
    let order_name = match order {
        0 => None,
        1 => Some("relaxed"),
        2 => Some("acquire"),
        3 => Some("release"),
        4 => Some("acq_rel"),
        5 => Some("seq_cst"),
        _ => unreachable!(),
    };
    if let Some(order) = order_name {
        let key = if instruction.opcode == 0x202 {
            "order"
        } else {
            "success_order"
        };
        write!(modifiers, " [{key}={order}]").map_err(formatting_error)?;
    }
    if instruction.opcode == 0x203 && failure != 0 {
        let mode = match failure {
            1 => "relaxed",
            2 => "acquire",
            5 => "seq_cst",
            _ => unreachable!(),
        };
        write!(modifiers, " [failure_order={mode}]").map_err(formatting_error)?;
    }
    if scope != 0 {
        let mode = if scope == 1 { "thread" } else { "system" };
        write!(modifiers, " [scope={mode}]").map_err(formatting_error)?;
    }
    let result = format!("v{}", values.len());
    let arg_names = inputs
        .iter()
        .map(|(name, _)| format!("%{name}"))
        .collect::<Vec<_>>()
        .join(", ");
    let (mnemonic, result_ty) = if instruction.opcode == 0x202 {
        (format!("atomic.rmw {op}"), ty)
    } else {
        let i1 = types
            .entries
            .iter()
            .position(|entry| matches!(entry, BinaryType::Integer(1)))
            .ok_or_else(|| malformed("cmpxchg i1 type is not interned"))? as u64;
        let tuple = types
            .entries
            .iter()
            .position(|entry| {
                matches!(entry, BinaryType::Tuple(elements)
            if elements.as_slice() == [ty, i1])
            })
            .ok_or_else(|| malformed("cmpxchg tuple result type is not interned"))?
            as u64;
        ("atomic.cmpxchg".to_owned(), tuple)
    };
    writeln!(text, "    %{result} = {mnemonic}{modifiers} {arg_names}")
        .map_err(formatting_error)?;
    values.push((result, result_ty));
    Ok(())
}

fn formatting_error(issue: std::fmt::Error) -> FormatError {
    malformed(issue.to_string())
}
fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
fn error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
