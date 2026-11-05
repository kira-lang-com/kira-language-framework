use crate::text::TokenKind;
use crate::{TextInstruction, TextItem, TextParseError};
use tir_diag::Span;

pub(crate) struct ParsedSelect {
    pub(crate) name: String,
    pub(crate) condition: String,
    pub(crate) when_true: String,
    pub(crate) when_false: String,
}

pub(crate) fn parse_select(
    instruction: &TextInstruction,
    item: TextItem,
) -> Result<ParsedSelect, TextParseError> {
    let tokens = &instruction.tokens;
    let valid = instruction.opcode == "select"
        && instruction.results.len() == 1
        && tokens.len() == 12
        && matches!(
            tokens.first().map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
        && matches!(
            tokens.get(2).map(|token| &token.kind),
            Some(TokenKind::Punctuation('='))
        )
        && matches!(tokens.get(3).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "select")
        && matches!(
            tokens.get(4).map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
        && matches!(
            tokens.get(6).map(|token| &token.kind),
            Some(TokenKind::Punctuation(','))
        )
        && matches!(
            tokens.get(7).map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
        && matches!(
            tokens.get(9).map(|token| &token.kind),
            Some(TokenKind::Punctuation(','))
        )
        && matches!(
            tokens.get(10).map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        );
    if !valid {
        return Err(item_error(
            item,
            instruction.span,
            "expected `%name = select %condition, %true, %false`",
        ));
    }
    let name = atom(tokens, 1, item, "invalid select result name")?;
    if name != instruction.results[0] {
        return Err(item_error(
            item,
            instruction.span,
            "invalid select result name",
        ));
    }
    Ok(ParsedSelect {
        name,
        condition: atom(tokens, 5, item, "invalid select condition")?,
        when_true: atom(tokens, 8, item, "invalid select value")?,
        when_false: atom(tokens, 11, item, "invalid select value")?,
    })
}

fn atom(
    tokens: &[crate::text::Token],
    index: usize,
    item: TextItem,
    message: &'static str,
) -> Result<String, TextParseError> {
    match tokens.get(index) {
        Some(token) => match &token.kind {
            TokenKind::Atom(value) => Ok(value.clone()),
            _ => Err(item_error(item, token.span, message)),
        },
        None => Err(item_error(item, Span::new(0, 0), message)),
    }
}

fn item_error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
