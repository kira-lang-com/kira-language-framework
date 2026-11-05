use tir_diag::Span;
use tir_format::text::{Token, TokenKind, lex};
use tir_target::Architecture;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum VariantArchitecture {
    Portable,
    Aarch64,
    X64,
    Rv64,
    Wasm,
}

impl VariantArchitecture {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "portable" => Some(Self::Portable),
            "aarch64" => Some(Self::Aarch64),
            "x64" => Some(Self::X64),
            "rv64" => Some(Self::Rv64),
            "wasm" => Some(Self::Wasm),
            _ => None,
        }
    }

    pub(super) const fn target(self) -> Option<Architecture> {
        match self {
            Self::Portable => None,
            Self::Aarch64 => Some(Architecture::Aarch64),
            Self::X64 => Some(Architecture::X64),
            Self::Rv64 => Some(Architecture::Rv64),
            Self::Wasm => Some(Architecture::Wasm),
        }
    }

    pub(super) const fn name(self) -> &'static str {
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
pub(super) struct VariantKey {
    pub(super) architecture: VariantArchitecture,
    pub(super) features: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct VariantMember {
    pub(super) key: VariantKey,
    pub(super) function: String,
    pub(super) span: Span,
}

pub(super) fn parse_set(source: &str) -> Result<Vec<VariantMember>, (Span, String)> {
    let tokens = lex(source).map_err(|error| (error.span, error.message.to_owned()))?;
    let tokens = tokens
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::End))
        .collect::<Vec<_>>();
    let mut cursor = 0;
    skip_newlines(&tokens, &mut cursor);
    expect_atom(&tokens, &mut cursor, "variants")?;
    let Some((_, end, _)) = read_symbol(&tokens, cursor) else {
        return Err((
            peek_span(&tokens, cursor),
            "expected variant-set symbol".to_owned(),
        ));
    };
    cursor = end;
    expect_punctuation(&tokens, &mut cursor, '{')?;

    let mut members = Vec::new();
    loop {
        skip_newlines(&tokens, &mut cursor);
        if is_punctuation_at(&tokens, cursor, '}') {
            cursor += 1;
            break;
        }
        if cursor >= tokens.len() {
            return Err((
                peek_span(&tokens, cursor),
                "unterminated variant set".to_owned(),
            ));
        }
        let start = tokens[cursor].span.start;
        let arch_name = take_atom(&tokens, &mut cursor, "expected variant architecture")?;
        let architecture = VariantArchitecture::parse(&arch_name).ok_or_else(|| {
            (
                tokens[cursor - 1].span,
                format!("unknown variant architecture {arch_name}"),
            )
        })?;
        let mut features = Vec::new();
        if is_punctuation_at(&tokens, cursor, '[') {
            cursor += 1;
            loop {
                features.push(take_atom(&tokens, &mut cursor, "expected variant feature")?);
                if is_punctuation_at(&tokens, cursor, ',') {
                    cursor += 1;
                    continue;
                }
                expect_punctuation(&tokens, &mut cursor, ']')?;
                break;
            }
        }
        features.sort_unstable();
        expect_punctuation(&tokens, &mut cursor, ':')?;
        let Some((function, next, symbol_span)) = read_symbol(&tokens, cursor) else {
            return Err((
                peek_span(&tokens, cursor),
                "expected variant member function".to_owned(),
            ));
        };
        cursor = next;
        members.push(VariantMember {
            key: VariantKey {
                architecture,
                features,
            },
            function,
            span: Span::new(start, symbol_span.end),
        });
        skip_newlines(&tokens, &mut cursor);
    }
    skip_newlines(&tokens, &mut cursor);
    if cursor != tokens.len() {
        return Err((
            tokens[cursor].span,
            "unexpected token after variant set".to_owned(),
        ));
    }
    if members.is_empty() {
        return Err((
            Span::new(0, source.len()),
            "variant set must have at least one member".to_owned(),
        ));
    }
    Ok(members)
}

pub(super) fn parse_function_variant(
    header: &[Token],
) -> Result<Option<VariantKey>, (Span, String)> {
    let Some(index) = header.iter().position(|token| is_atom(token, "variant")) else {
        return Ok(None);
    };
    if let Some(duplicate) = header
        .iter()
        .enumerate()
        .skip(index + 1)
        .find(|(_, token)| is_atom(token, "variant"))
    {
        return Err((
            duplicate.1.span,
            "function has more than one variant attribute".to_owned(),
        ));
    }
    let mut cursor = index + 1;
    expect_punctuation(header, &mut cursor, '(')?;
    let arch_name = take_atom(
        header,
        &mut cursor,
        "expected function variant architecture",
    )?;
    let architecture = VariantArchitecture::parse(&arch_name).ok_or_else(|| {
        (
            header[cursor - 1].span,
            format!("unknown function variant architecture {arch_name}"),
        )
    })?;
    if architecture == VariantArchitecture::Portable {
        return Err((
            header[cursor - 1].span,
            "function variant attribute must name an architecture-specific body".to_owned(),
        ));
    }
    let mut features = Vec::new();
    while is_punctuation_at(header, cursor, ',') {
        cursor += 1;
        features.push(take_atom(
            header,
            &mut cursor,
            "expected function variant feature",
        )?);
    }
    features.sort_unstable();
    expect_punctuation(header, &mut cursor, ')')?;
    Ok(Some(VariantKey {
        architecture,
        features,
    }))
}

pub(super) fn intrinsic_architecture(tokens: &[Token]) -> Option<VariantArchitecture> {
    let name = tokens.iter().find_map(|token| match &token.kind {
        TokenKind::String(value) => Some(value.as_str()),
        _ => None,
    })?;
    if name.starts_with("llvm.aarch64.") {
        Some(VariantArchitecture::Aarch64)
    } else if name.starts_with("llvm.x86.") {
        Some(VariantArchitecture::X64)
    } else if name.starts_with("llvm.riscv.") {
        Some(VariantArchitecture::Rv64)
    } else if name.starts_with("llvm.wasm.") {
        Some(VariantArchitecture::Wasm)
    } else {
        None
    }
}

pub(super) fn significant(tokens: Vec<Token>) -> Vec<Token> {
    tokens
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect()
}

pub(super) fn read_symbol(tokens: &[Token], index: usize) -> Option<(String, usize, Span)> {
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

pub(super) fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

pub(super) fn is_atom(token: &Token, expected: &str) -> bool {
    atom(token) == Some(expected)
}

pub(super) fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn expect_atom(tokens: &[Token], cursor: &mut usize, expected: &str) -> Result<(), (Span, String)> {
    if tokens
        .get(*cursor)
        .is_some_and(|token| is_atom(token, expected))
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
    tokens
        .get(index)
        .is_some_and(|token| is_punctuation(token, expected))
}

fn peek_span(tokens: &[Token], index: usize) -> Span {
    tokens.get(index).map_or_else(
        || tokens.last().map_or(Span::new(0, 0), |token| token.span),
        |token| token.span,
    )
}
