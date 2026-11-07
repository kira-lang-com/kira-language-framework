use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{Abi, Convention, Type};

use crate::types::{
    BinaryInstruction, BinaryOperand, BinarySignatureTable, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct IndirectCall {
    pub result: Option<String>,
    pub function_value: String,
    pub function_type: Type,
    pub args: Vec<String>,
    pub modifiers: u64,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<IndirectCall, TextParseError> {
    if instruction.opcode != "call.indirect" {
        return Err(error(item, instruction.span, "expected call.indirect"));
    }
    let result = match instruction.results.as_slice() {
        [name] => Some(name.clone()),
        [] => None,
        _ => {
            return Err(error(
                item,
                instruction.span,
                "call.indirect may define at most one result",
            ));
        }
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "indirect call source span is invalid",
            )
        })?
        .trim();
    let prefix = match &result {
        Some(name) => format!("%{name} = call.indirect "),
        None => "call.indirect ".to_owned(),
    };
    let raw = line.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "call.indirect requires an SSA function value",
        )
    })?;
    let (modifiers, raw) = crate::text_call_attributes::parse(raw, item, instruction.span)?;
    let raw = raw.strip_prefix('%').ok_or_else(|| {
        error(
            item,
            instruction.span,
            "call.indirect requires an SSA function value",
        )
    })?;
    let (function_value, signature_and_args) = raw.split_once(" : ").ok_or_else(|| {
        error(
            item,
            instruction.span,
            "call.indirect requires an explicit function type",
        )
    })?;
    if !valid_name(function_value) {
        return Err(error(
            item,
            instruction.span,
            "indirect callee SSA name is invalid",
        ));
    }
    let (type_name, argument_text) = signature_and_args.rsplit_once(" (").ok_or_else(|| {
        error(
            item,
            instruction.span,
            "call.indirect requires parenthesized arguments",
        )
    })?;
    let args = argument_text.strip_suffix(')').ok_or_else(|| {
        error(
            item,
            instruction.span,
            "call.indirect argument list is not closed",
        )
    })?;
    let function_type = crate::parse_type(type_name.trim()).map_err(|issue| {
        error(
            item,
            instruction.span,
            format!("invalid indirect-call function type: {issue}"),
        )
    })?;
    let args = if args.trim().is_empty() {
        Vec::new()
    } else {
        args.split(',')
            .map(|value| {
                let name = value
                    .trim()
                    .strip_prefix('%')
                    .filter(|name| valid_name(name))
                    .ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "indirect-call arguments must be SSA values",
                        )
                    })?;
                Ok(name.to_owned())
            })
            .collect::<Result<Vec<_>, TextParseError>>()?
    };
    Ok(IndirectCall {
        result,
        function_value: function_value.to_owned(),
        function_type,
        args,
        modifiers,
    })
}

pub(crate) fn validate(
    call: &IndirectCall,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    let Type::Function(signature) = &call.function_type else {
        return Err(error(
            item,
            span,
            "call.indirect type must be a function type",
        ));
    };
    if signature.foreign || signature.failure.is_some() || signature.asynchronous {
        return Err(error(
            item,
            span,
            "foreign, failing or asynchronous indirect calls are not yet supported",
        ));
    }
    if values.get(&call.function_value) != Some(&call.function_type) {
        return Err(error(
            item,
            span,
            "indirect callee SSA value does not have the declared function type",
        ));
    }
    if call.args.len() != signature.parameters.len() {
        return Err(error(
            item,
            span,
            "indirect call arguments disagree with declared function type",
        ));
    }
    for (actual, expected) in call.args.iter().zip(&signature.parameters) {
        if values.get(actual) != Some(&expected.ty) {
            return Err(error(
                item,
                span,
                "indirect call argument has an incompatible type",
            ));
        }
    }
    match (call.result.as_ref(), signature.result.as_ref()) {
        (None, Type::Unit) => Ok(None),
        (Some(name), result) if !matches!(result, Type::Unit | Type::Never) => {
            Ok(Some((name.clone(), result.clone())))
        }
        _ => Err(error(
            item,
            span,
            "indirect call SSA result count is invalid",
        )),
    }
}

pub(crate) fn encode(
    call: &IndirectCall,
    values: &[&str],
    types: &BinaryTypeTable,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let ty = interned_type_index(types, &call.function_type)
        .ok_or_else(|| error(item, span, "indirect function type is not interned"))?;
    let value_index = |name: &str| {
        values
            .iter()
            .position(|value| *value == name)
            .map(|pos| (values.len() - pos) as u64)
            .ok_or_else(|| error(item, span, "indirect call SSA operand is undefined"))
    };
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(0x1a1)?;
    write(call.modifiers)?;
    write(u64::from(call.result.is_some()))?;
    write(value_index(&call.function_value)?)?;
    write(ty as u64)?;
    write(call.args.len() as u64)?;
    for arg in &call.args {
        write(value_index(arg)?)?;
    }
    Ok(())
}

fn interned_type_index(types: &BinaryTypeTable, ty: &Type) -> Option<usize> {
    match ty {
        Type::Unit => types
            .entries
            .iter()
            .position(|entry| matches!(entry, BinaryType::Unit)),
        Type::Never => types
            .entries
            .iter()
            .position(|entry| matches!(entry, BinaryType::Never)),
        Type::Function(signature) => types.entries.iter().position(|entry| {
            let BinaryType::Function(candidate) = entry else {
                return false;
            };
            candidate.asynchronous == signature.asynchronous
                && candidate.c_abi == (signature.abi == Abi::C)
                && candidate.foreign == signature.foreign
                && candidate.parameters.len() == signature.parameters.len()
                && interned_type_index(types, &signature.result)
                    .is_some_and(|index| candidate.result == index as u64)
                && match (candidate.error, signature.failure.as_deref()) {
                    (None, None) => true,
                    (Some(actual), Some(expected)) => interned_type_index(types, expected)
                        .is_some_and(|index| actual == index as u64),
                    _ => false,
                }
                && candidate.parameters.iter().zip(&signature.parameters).all(
                    |(actual, expected)| {
                        let convention = match expected.convention {
                            Some(Convention::Owned) => 0,
                            Some(Convention::Borrow) => 1,
                            Some(Convention::BorrowMut) => 2,
                            None => 3,
                        };
                        actual.convention == convention
                            && actual.context == expected.context
                            && interned_type_index(types, &expected.ty)
                                .is_some_and(|index| actual.ty == index as u64)
                    },
                )
        }),
        _ => crate::text_conversion_assembly::target_index(types, ty),
    }
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.opcode != 0x1a1 {
        return Err(malformed("unsupported indirect-call opcode"));
    }
    let attributes = crate::text_call_attributes::canonical(instruction.modifiers)?;
    let [
        BinaryOperand::Value(value),
        BinaryOperand::Type(ty),
        args @ ..,
    ] = instruction.operands.as_slice()
    else {
        return Err(malformed(
            "indirect call requires a function SSA value and type index",
        ));
    };
    let args = match args {
        [] => Vec::new(),
        [BinaryOperand::Value(value)] => vec![*value],
        [BinaryOperand::Values(args)] if args.len() >= 2 => args.clone(),
        _ => return Err(malformed("indirect-call argument encoding is invalid")),
    };
    let Some(BinaryType::Function(function)) = types.entries.get(*ty as usize) else {
        return Err(malformed("indirect call type index is not a function"));
    };
    if function.foreign || function.error.is_some() || function.asynchronous {
        return Err(malformed(
            "indirect call has unsupported asynchronous, error or foreign type",
        ));
    }
    let (function_value, actual_type) = usize::try_from(*value)
        .ok()
        .and_then(|index| values.get(index))
        .ok_or_else(|| malformed("indirect callee SSA value is undefined"))?;
    if actual_type != ty || args.len() != function.parameters.len() {
        return Err(malformed(
            "indirect callee SSA type or argument count is invalid",
        ));
    }
    let mut arguments = Vec::new();
    for (value, expected) in args.iter().zip(&function.parameters) {
        let (name, ty) = usize::try_from(*value)
            .ok()
            .and_then(|index| values.get(index))
            .ok_or_else(|| malformed("indirect-call argument is undefined"))?;
        if ty != &expected.ty {
            return Err(malformed("indirect-call argument type is invalid"));
        }
        arguments.push(format!("%{name}"));
    }
    let result = match types.entries.get(function.result as usize) {
        Some(BinaryType::Unit) if instruction.result_count == 0 => None,
        Some(BinaryType::Unit | BinaryType::Never) | None => {
            return Err(malformed("indirect-call result shape is unsupported"));
        }
        Some(_) if instruction.result_count == 1 => Some(function.result),
        _ => {
            return Err(malformed(
                "indirect call result count disagrees with its function type",
            ));
        }
    };
    let type_name = crate::text_type_disassembly::format_type_reference(types, *ty, signatures)?;
    let arguments = arguments.join(", ");
    match result {
        Some(result_ty) => {
            let name = format!("v{}", values.len());
            writeln!(
                text,
                "    %{name} = call.indirect{attributes} %{function_value} : {type_name} ({arguments})"
            )
            .map_err(formatting_error)?;
            values.push((name, result_ty));
        }
        None => writeln!(
            text,
            "    call.indirect{attributes} %{function_value} : {type_name} ({arguments})"
        )
        .map_err(formatting_error)?,
    }
    Ok(())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
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
