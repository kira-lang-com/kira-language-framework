use tir_diag::Span;
use tir_format::text::{Token, TokenKind, lex};
use tir_target::Architecture;

use super::{TextItem, TextItemKind, TextParseError, error};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TextVariantArchitecture {
    Portable,
    Aarch64,
    X64,
    Rv64,
    Wasm,
}

impl TextVariantArchitecture {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "portable" => Some(Self::Portable),
            "aarch64" => Some(Self::Aarch64),
            "x64" => Some(Self::X64),
            "rv64" => Some(Self::Rv64),
            "wasm" => Some(Self::Wasm),
            _ => None,
        }
    }

    #[must_use]
    pub const fn target(self) -> Option<Architecture> {
        match self {
            Self::Portable => None,
            Self::Aarch64 => Some(Architecture::Aarch64),
            Self::X64 => Some(Architecture::X64),
            Self::Rv64 => Some(Architecture::Rv64),
            Self::Wasm => Some(Architecture::Wasm),
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::Aarch64 => "aarch64",
            Self::X64 => "x64",
            Self::Rv64 => "rv64",
            Self::Wasm => "wasm",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TextVariantKey {
    pub architecture: TextVariantArchitecture,
    pub features: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextVariantMember {
    pub key: TextVariantKey,
    pub function: String,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextVariantSet {
    pub name: String,
    pub dispatch: bool,
    pub members: Vec<TextVariantMember>,
    pub span: Span,
}

pub(super) fn parse(
    source: &str,
    items: &[TextItem],
) -> Result<Vec<TextVariantSet>, TextParseError> {
    let mut sets = Vec::new();
    for item in items
        .iter()
        .filter(|item| matches!(item.kind, TextItemKind::Variants | TextItemKind::Dispatch))
    {
        let text = source
            .get(item.span.start..item.span.end)
            .unwrap_or_default();
        let tokens = lex(text).map_err(|cause| TextParseError {
            span: shift(cause.span, item.span.start),
            message: cause.message.to_owned(),
        })?;
        let tokens: Vec<_> = tokens
            .into_iter()
            .filter(|token| !matches!(token.kind, TokenKind::End))
            .collect();
        let mut cursor = 0;
        skip_newlines(&tokens, &mut cursor);
        let expected = if item.kind == TextItemKind::Dispatch {
            "dispatch"
        } else {
            "variants"
        };
        expect_atom(&tokens, &mut cursor, expected)
            .map_err(|(span, message)| error(shift(span, item.span.start), &message))?;
        let (name, next, _) = read_symbol(&tokens, cursor)
            .ok_or_else(|| error(item.span, "expected variant-set symbol"))?;
        cursor = next;
        expect_punctuation(&tokens, &mut cursor, '{')
            .map_err(|(span, message)| error(shift(span, item.span.start), &message))?;

        let mut members = Vec::new();
        loop {
            skip_newlines(&tokens, &mut cursor);
            if is_punctuation_at(&tokens, cursor, '}') {
                cursor += 1;
                break;
            }
            if cursor >= tokens.len() {
                return Err(error(item.span, "unterminated variant set"));
            }
            let start = tokens[cursor].span.start;
            let architecture_name =
                take_atom(&tokens, &mut cursor, "expected variant architecture")
                    .map_err(|(span, message)| error(shift(span, item.span.start), &message))?;
            let architecture =
                TextVariantArchitecture::parse(&architecture_name).ok_or_else(|| {
                    error(
                        shift(tokens[cursor - 1].span, item.span.start),
                        &format!("unknown variant architecture {architecture_name}"),
                    )
                })?;
            let mut features = Vec::new();
            if is_punctuation_at(&tokens, cursor, '[') {
                cursor += 1;
                loop {
                    features.push(
                        take_atom(&tokens, &mut cursor, "expected variant feature").map_err(
                            |(span, message)| error(shift(span, item.span.start), &message),
                        )?,
                    );
                    if is_punctuation_at(&tokens, cursor, ',') {
                        cursor += 1;
                        continue;
                    }
                    expect_punctuation(&tokens, &mut cursor, ']')
                        .map_err(|(span, message)| error(shift(span, item.span.start), &message))?;
                    break;
                }
            }
            features.sort_unstable();
            expect_punctuation(&tokens, &mut cursor, ':')
                .map_err(|(span, message)| error(shift(span, item.span.start), &message))?;
            let (function, next, symbol_span) = read_symbol(&tokens, cursor)
                .ok_or_else(|| error(item.span, "expected variant member function"))?;
            cursor = next;
            members.push(TextVariantMember {
                key: TextVariantKey {
                    architecture,
                    features,
                },
                function,
                span: shift(Span::new(start, symbol_span.end), item.span.start),
            });
            skip_newlines(&tokens, &mut cursor);
        }
        skip_newlines(&tokens, &mut cursor);
        if cursor != tokens.len() {
            return Err(error(
                shift(tokens[cursor].span, item.span.start),
                "unexpected token after variant set",
            ));
        }
        if members.is_empty() {
            return Err(error(
                item.span,
                "variant set must have at least one member",
            ));
        }
        sets.push(TextVariantSet {
            name,
            dispatch: item.kind == TextItemKind::Dispatch,
            members,
            span: item.span,
        });
    }
    Ok(sets)
}

fn read_symbol(tokens: &[Token], index: usize) -> Option<(String, usize, Span)> {
    if !is_punctuation_at(tokens, index, '@') {
        return None;
    }
    let name_token = tokens.get(index + 1)?;
    let mut name = atom(name_token)?.to_owned();
    let start = tokens[index].span.start;
    let mut end = name_token.span.end;
    let mut cursor = index + 2;
    while is_punctuation_at(tokens, cursor, '.') {
        let component = atom(tokens.get(cursor + 1)?)?;
        name.push('.');
        name.push_str(component);
        end = tokens[cursor + 1].span.end;
        cursor += 2;
    }
    Some((name, cursor, Span::new(start, end)))
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn expect_atom(tokens: &[Token], cursor: &mut usize, expected: &str) -> Result<(), (Span, String)> {
    if tokens
        .get(*cursor)
        .is_some_and(|token| atom(token) == Some(expected))
    {
        *cursor += 1;
        Ok(())
    } else {
        Err((peek_span(tokens, *cursor), format!("expected {expected}")))
    }
}

fn take_atom(
    tokens: &[Token],
    cursor: &mut usize,
    message: &str,
) -> Result<String, (Span, String)> {
    let Some(token) = tokens.get(*cursor) else {
        return Err((peek_span(tokens, *cursor), message.to_owned()));
    };
    let Some(value) = atom(token) else {
        return Err((token.span, message.to_owned()));
    };
    *cursor += 1;
    Ok(value.to_owned())
}

fn expect_punctuation(
    tokens: &[Token],
    cursor: &mut usize,
    expected: char,
) -> Result<(), (Span, String)> {
    if is_punctuation_at(tokens, *cursor, expected) {
        *cursor += 1;
        Ok(())
    } else {
        Err((peek_span(tokens, *cursor), format!("expected '{expected}'")))
    }
}

fn skip_newlines(tokens: &[Token], cursor: &mut usize) {
    while tokens
        .get(*cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Newline))
    {
        *cursor += 1;
    }
}

fn is_punctuation_at(tokens: &[Token], index: usize, expected: char) -> bool {
    tokens.get(index).is_some_and(
        |token| matches!(token.kind, TokenKind::Punctuation(value) if value == expected),
    )
}

fn peek_span(tokens: &[Token], index: usize) -> Span {
    tokens.get(index).map_or_else(
        || tokens.last().map_or(Span::new(0, 0), |token| token.span),
        |token| token.span,
    )
}

fn shift(span: Span, offset: usize) -> Span {
    Span::new(
        span.start.saturating_add(offset),
        span.end.saturating_add(offset),
    )
}
