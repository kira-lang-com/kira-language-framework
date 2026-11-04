use std::collections::HashMap;

use tir_diag::{Diagnostic, Span};
use tir_format::text::{KiraStringEscapeError, decode_kira_string_literal_contents, lex};

use super::VFY22;
use super::{Field, Value, ValueKind};

pub(super) fn diagnostic(base: usize, span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(VFY22, message).with_primary_span(Span::new(
        base.saturating_add(span.start),
        base.saturating_add(span.end),
    ))
}

pub(super) fn scalar(value: &Value) -> Option<&str> {
    match &value.kind {
        ValueKind::Scalar(text) => Some(text),
        _ => None,
    }
}

pub(super) fn array(value: &Value) -> Option<&[Value]> {
    match &value.kind {
        ValueKind::Array(values) => Some(values),
        _ => None,
    }
}

pub(super) fn object(value: &Value) -> Option<&[Field]> {
    match &value.kind {
        ValueKind::Object(fields) => Some(fields),
        _ => None,
    }
}

pub(super) fn object_map(value: &Value) -> Option<HashMap<&str, &Value>> {
    Some(
        object(value)?
            .iter()
            .map(|field| (field.name.as_str(), &field.value))
            .collect(),
    )
}

pub(super) fn number(value: &Value) -> Option<u64> {
    u64::try_from(tir_format::numeric_literals::parse_unsigned_integer(
        scalar(value)?,
    )?)
    .ok()
}

pub(super) fn verify_number(
    value: &Value,
    name: &str,
    base: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if number(value).is_none() {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("{name} must be a nonnegative integer"),
        ));
    }
}

pub(super) fn verify_hex(
    value: &Value,
    name: &str,
    base: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let parsed = scalar(value).and_then(|value| {
        value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
            .and_then(|_| tir_format::numeric_literals::parse_unsigned_integer(value))
            .and_then(|value| u64::try_from(value).ok())
    });
    if parsed.is_none() {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("{name} must be a 64-bit hexadecimal value"),
        ));
    }
}

pub(super) fn verify_object(
    value: &Value,
    name: &str,
    expected: &[&str],
    base: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(fields) = object(value) else {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("{name} must be an object"),
        ));
        return;
    };
    let mut seen = HashMap::new();
    for field in fields {
        if !expected.contains(&field.name.as_str()) {
            diagnostics.push(diagnostic(
                base,
                field.span,
                format!("unknown member '{}' in {name}", field.name),
            ));
        } else if seen.insert(field.name.as_str(), ()).is_some() {
            diagnostics.push(diagnostic(
                base,
                field.span,
                format!("duplicate member '{}' in {name}", field.name),
            ));
        }
    }
    for member in expected {
        if !seen.contains_key(member) {
            diagnostics.push(diagnostic(
                base,
                value.span,
                format!("{name} is missing member '{member}'"),
            ));
        }
    }
}

pub(super) fn string_value(value: &Value) -> Result<Option<String>, KiraStringEscapeError> {
    let Some(raw) = scalar(value)
        .and_then(|raw| raw.strip_prefix('"'))
        .and_then(|raw| raw.strip_suffix('"'))
    else {
        return Ok(None);
    };
    decode_kira_string_literal_contents(raw).map(Some)
}

pub(super) fn normalize_text(source: &str) -> Option<String> {
    let mut normalized = String::new();
    for token in lex(source).ok()? {
        if matches!(
            token.kind,
            tir_format::text::TokenKind::End | tir_format::text::TokenKind::Newline
        ) {
            continue;
        }
        normalized.push_str(source.get(token.span.start..token.span.end)?);
    }
    Some(normalized)
}

pub(super) fn valid_semantic(value: &str) -> bool {
    let Some((name, argument)) = value
        .split_once('(')
        .and_then(|(name, tail)| tail.strip_suffix(')').map(|arg| (name, arg)))
    else {
        return matches!(
            value,
            "user"
                | "position"
                | "stencil"
                | "sample_mask"
                | "point_size"
                | "layer"
                | "viewport"
                | "primitive_id"
                | "view_index"
                | "cull"
                | "shading_rate"
        );
    };
    match name {
        "color" | "color_index1" | "clip_distance" | "cull_distance" => {
            tir_format::numeric_literals::parse_unsigned_integer(argument)
                .and_then(|value| u64::try_from(value).ok())
                .is_some()
        }
        "depth" => matches!(argument, "any" | "greater" | "less"),
        _ => false,
    }
}
