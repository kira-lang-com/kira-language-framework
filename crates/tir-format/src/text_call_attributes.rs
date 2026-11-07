//! CODE call-site attributes in the normative modifier bits 41–45.
//! `[builtin]` is intentionally rejected: it is spelled in TIR §12.11 but
//! has no assigned bit in §22.8 and cannot yet be losslessly represented.
use crate::{FormatError, TextItem, TextParseError};
use tir_diag::Span;

const ATTRIBUTES: [(&str, u64); 5] = [
    ("cold", 41),
    ("nounwind", 42),
    ("noreturn", 43),
    ("inline", 44),
    ("noinline", 45),
];
const MASK: u64 = 0x1f << 41;

pub(crate) fn parse<'a>(
    mut text: &'a str,
    item: TextItem,
    span: Span,
) -> Result<(u64, &'a str), TextParseError> {
    let mut flags = 0;
    while let Some(remainder) = text.strip_prefix('[') {
        let (name, after) = remainder
            .split_once(']')
            .ok_or_else(|| error(item, span, "unterminated call attribute"))?;
        let (_, bit) = ATTRIBUTES
            .iter()
            .find(|(attribute, _)| *attribute == name)
            .ok_or_else(|| error(item, span, "unrepresentable or unknown call attribute"))?;
        if flags & (1 << bit) != 0 {
            return Err(error(item, span, "duplicate call attribute"));
        }
        flags |= 1 << bit;
        if !after.starts_with(' ') {
            return Err(error(
                item,
                span,
                "call attributes must be separated by spaces",
            ));
        }
        text = after.trim_start();
    }
    if flags & (1 << 44) != 0 && flags & (1 << 45) != 0 {
        return Err(error(item, span, "inline and noinline cannot be combined"));
    }
    Ok((flags, text))
}

pub(crate) fn canonical(flags: u64) -> Result<String, FormatError> {
    if flags & !MASK != 0 || flags & (1 << 44) != 0 && flags & (1 << 45) != 0 {
        return Err(FormatError::malformed(None, "invalid call attribute bits"));
    }
    let mut text = String::new();
    for (name, bit) in ATTRIBUTES {
        if flags & (1 << bit) != 0 {
            text.push(' ');
            text.push('[');
            text.push_str(name);
            text.push(']');
        }
    }
    Ok(text)
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

#[cfg(test)]
mod tests {
    use super::canonical;
    #[test]
    fn canonical_call_attribute_order_matches_bit_positions() {
        assert_eq!(
            canonical((1 << 45) | (1 << 42)).unwrap(),
            " [nounwind] [noinline]"
        );
        assert!(canonical((1 << 44) | (1 << 45)).is_err());
        assert!(canonical(1 << 46).is_err());
    }
}
