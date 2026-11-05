use tir_diag::Span;
use tir_format::numeric_literals::parse_unsigned_integer;

use crate::text::error;
use crate::text_entries::{TextEntryField, TextEntryValue, TextEntryValueKind};

pub(super) fn member<'a>(
    fields: &'a [TextEntryField],
    name: &str,
) -> Result<&'a TextEntryValue, crate::text::TextParseError> {
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
        .ok_or_else(|| error(Span::new(0, 0), "missing GPU entry field"))
}

pub(super) fn object<'a>(
    value: &'a TextEntryValue,
    name: &str,
) -> Result<&'a [TextEntryField], crate::text::TextParseError> {
    if let TextEntryValueKind::Object(fields) = &value.kind {
        Ok(fields)
    } else {
        Err(error(value.span, &format!("{name} must be an object")))
    }
}

pub(super) fn array<'a>(
    value: &'a TextEntryValue,
    name: &str,
) -> Result<&'a [TextEntryValue], crate::text::TextParseError> {
    if let TextEntryValueKind::Array(values) = &value.kind {
        Ok(values)
    } else {
        Err(error(value.span, &format!("{name} must be an array")))
    }
}

pub(super) fn atom(value: &TextEntryValue) -> Result<&str, crate::text::TextParseError> {
    if let TextEntryValueKind::Atom(atom) = &value.kind {
        Ok(atom)
    } else {
        Err(error(value.span, "expected scalar literal"))
    }
}

pub(super) fn string(value: &TextEntryValue) -> Result<&str, crate::text::TextParseError> {
    if let TextEntryValueKind::String(string) = &value.kind {
        Ok(string)
    } else {
        Err(error(value.span, "expected string literal"))
    }
}

pub(super) fn unsigned(
    value: &TextEntryValue,
    name: &str,
    bits: u32,
) -> Result<u64, crate::text::TextParseError> {
    let number = parse_unsigned_integer(atom(value)?)
        .ok_or_else(|| error(value.span, &format!("invalid {name}")))?;
    if number >= (1_u128 << bits.min(64)) {
        return Err(error(value.span, &format!("{name} is out of range")));
    }
    u64::try_from(number).map_err(|_| error(value.span, &format!("{name} is out of range")))
}

pub(super) fn positive(
    value: &TextEntryValue,
    name: &str,
    bits: u32,
) -> Result<u64, crate::text::TextParseError> {
    let number = unsigned(value, name, bits)?;
    if number == 0 {
        return Err(error(value.span, &format!("{name} must be nonzero")));
    }
    Ok(number)
}

pub(super) fn require_hex(
    value: &TextEntryValue,
    name: &str,
    bits: u32,
) -> Result<(), crate::text::TextParseError> {
    let literal = atom(value)?;
    if !(literal.starts_with("0x") || literal.starts_with("0X")) {
        return Err(error(value.span, &format!("{name} must be hexadecimal")));
    }
    unsigned(value, name, bits).map(|_| ())
}

pub(super) fn require_atom_in(
    value: &TextEntryValue,
    allowed: &[&str],
    name: &str,
) -> Result<(), crate::text::TextParseError> {
    let actual = atom(value)?;
    if allowed.contains(&actual) {
        Ok(())
    } else {
        Err(error(value.span, &format!("unknown {name}")))
    }
}

pub(super) fn require_type(value: &TextEntryValue) -> Result<(), crate::text::TextParseError> {
    if matches!(value.kind, TextEntryValueKind::Type(_)) {
        Ok(())
    } else {
        Err(error(value.span, "expected TVISA type"))
    }
}

pub(super) fn is_atom(value: &TextEntryValue, expected: &str) -> bool {
    matches!(&value.kind, TextEntryValueKind::Atom(actual) if actual == expected)
}
