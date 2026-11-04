use tir_diag::Span;
use tir_format::text::{TokenKind, decode_kira_string_literal_contents, lex};

use crate::binary::FormatError;
use crate::text::{TextGpuEntryField, TextGpuEntryValue, TextGpuEntryValueKind};

pub(super) fn invalid(span: Span, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(span.start), message)
}

pub(super) fn fields(
    fields: &[TextGpuEntryField],
    expected: &[&str],
    label: &str,
) -> Result<(), FormatError> {
    for field in fields {
        if !expected.contains(&field.name.as_str()) {
            return Err(invalid(
                field.span,
                format!("unknown field '{}' in {label}", field.name),
            ));
        }
    }
    for (index, field) in fields.iter().enumerate() {
        if fields[..index]
            .iter()
            .any(|previous| previous.name == field.name)
        {
            return Err(invalid(
                field.span,
                format!("duplicate field '{}' in {label}", field.name),
            ));
        }
    }
    for name in expected {
        if !fields.iter().any(|field| field.name == *name) {
            let span = fields.first().map_or(Span::new(0, 0), |field| field.span);
            return Err(invalid(span, format!("{label} is missing field '{name}'")));
        }
    }
    Ok(())
}

pub(super) fn field<'a>(
    fields: &'a [TextGpuEntryField],
    name: &str,
) -> Result<&'a TextGpuEntryValue, FormatError> {
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
        .ok_or_else(|| {
            invalid(
                fields.first().map_or(Span::new(0, 0), |field| field.span),
                format!("missing field '{name}'"),
            )
        })
}

pub(super) fn scalar<'a>(
    value: &'a TextGpuEntryValue,
    label: &str,
) -> Result<&'a str, FormatError> {
    match &value.kind {
        TextGpuEntryValueKind::Scalar(value) => Ok(value),
        _ => Err(invalid(value.span, format!("{label} must be a scalar"))),
    }
}

pub(super) fn array<'a>(
    value: &'a TextGpuEntryValue,
    label: &str,
) -> Result<&'a [TextGpuEntryValue], FormatError> {
    match &value.kind {
        TextGpuEntryValueKind::Array(values) => Ok(values),
        _ => Err(invalid(value.span, format!("{label} must be an array"))),
    }
}

pub(super) fn object<'a>(
    value: &'a TextGpuEntryValue,
    label: &str,
) -> Result<&'a [TextGpuEntryField], FormatError> {
    match &value.kind {
        TextGpuEntryValueKind::Object(fields) => Ok(fields),
        _ => Err(invalid(value.span, format!("{label} must be an object"))),
    }
}

pub(super) fn number(value: &TextGpuEntryValue, label: &str) -> Result<u64, FormatError> {
    let text = scalar(value, label)?;
    tir_format::numeric_literals::parse_unsigned_integer(text)
        .and_then(|number| u64::try_from(number).ok())
        .ok_or_else(|| invalid(value.span, format!("{label} must be an unsigned integer")))
}

pub(super) fn hexadecimal(value: &TextGpuEntryValue, label: &str) -> Result<u64, FormatError> {
    let text = scalar(value, label)?;
    if !text.starts_with("0x") && !text.starts_with("0X") {
        return Err(invalid(
            value.span,
            format!("{label} must be a hexadecimal integer"),
        ));
    }
    number(value, label)
}

pub(super) fn string(value: &TextGpuEntryValue, label: &str) -> Result<String, FormatError> {
    let literal = scalar(value, label)?;
    let Some(contents) = literal
        .strip_prefix('"')
        .and_then(|literal| literal.strip_suffix('"'))
    else {
        return Err(invalid(
            value.span,
            format!("{label} must be a string literal"),
        ));
    };
    decode_kira_string_literal_contents(contents)
        .map_err(|error| invalid(value.span, format!("invalid {label}: {error}")))
}

pub(super) fn type_value(
    value: &TextGpuEntryValue,
    label: &str,
) -> Result<tir_types::Type, FormatError> {
    let text = scalar(value, label)?;
    tir_format::parse_tvisa_type(text)
        .map_err(|error| invalid(value.span, format!("invalid {label}: {error}")))
}

pub(super) fn canonical_literal(
    source: &str,
    value: &TextGpuEntryValue,
    label: &str,
) -> Result<String, FormatError> {
    let raw = source
        .get(value.span.start..value.span.end)
        .ok_or_else(|| invalid(value.span, format!("invalid source span for {label}")))?;
    let tokens = lex(raw).map_err(|error| invalid(value.span, error.message))?;
    let mut canonical = String::new();
    for token in tokens {
        if matches!(token.kind, TokenKind::End | TokenKind::Newline) {
            continue;
        }
        let text = raw
            .get(token.span.start..token.span.end)
            .ok_or_else(|| invalid(value.span, format!("invalid token span in {label}")))?;
        canonical.push_str(text);
    }
    Ok(canonical)
}

pub(super) fn is_none(value: &TextGpuEntryValue) -> bool {
    matches!(&value.kind, TextGpuEntryValueKind::Scalar(text) if text == "none")
}
