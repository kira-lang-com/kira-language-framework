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

#[derive(Clone, Debug)]
pub(super) struct FunctionHeader {
    pub(super) symbol: String,
    pub(super) signature: Vec<TokenKind>,
    pub(super) abi: String,
    pub(super) engine: Option<String>,
    pub(super) target_cpu: bool,
    pub(super) target_gpu: bool,
    pub(super) variant: Option<VariantKey>,
    pub(super) symbol_span: Span,
}

pub(super) fn parse_variant_set(source: &str) -> Result<Vec<VariantMember>, (Span, String)> {
    let tokens = lex(source).map_err(|error| (error.span, error.message.to_owned()))?;
    let tokens = tokens
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::End))
        .collect::<Vec<_>>();
    let mut cursor = 0;
    skip_newlines(&tokens, &mut cursor);
    expect_atom(&tokens, &mut cursor, "variants")?;
    let Some((_, next, _)) = read_symbol(&tokens, cursor) else {
        return Err((
            peek_span(&tokens, cursor),
            "expected variant-set symbol".to_owned(),
        ));
    };
    cursor = next;
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
        let architecture_name = take_atom(&tokens, &mut cursor, "expected variant architecture")?;
        let architecture = VariantArchitecture::parse(&architecture_name).ok_or_else(|| {
            (
                tokens[cursor - 1].span,
                format!("unknown variant architecture {architecture_name}"),
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
                "expected variant function".to_owned(),
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

pub(super) fn parse_function_header(source: &str) -> Result<FunctionHeader, (Span, String)> {
    let tokens = lex(source).map_err(|error| (error.span, error.message.to_owned()))?;
    let tokens = significant(tokens);
    let header_end = tokens
        .iter()
        .position(|token| is_punctuation(token, '{'))
        .unwrap_or(tokens.len());
    let header = &tokens[..header_end];
    let symbol_index = header
        .iter()
        .position(|token| is_punctuation(token, '@'))
        .ok_or_else(|| {
            (
                Span::new(0, 0),
                "function declaration has no symbol".to_owned(),
            )
        })?;
    let (symbol, symbol_end, symbol_span) = read_symbol(header, symbol_index).ok_or_else(|| {
        (
            header[symbol_index].span,
            "malformed function symbol".to_owned(),
        )
    })?;
    let parameter_open = (symbol_end..header.len())
        .find(|index| is_punctuation(&header[*index], '('))
        .ok_or_else(|| {
            (
                symbol_span,
                "function declaration has no parameter list".to_owned(),
            )
        })?;
    let parameter_close =
        matching_delimiter(header, parameter_open, '(', ')').ok_or_else(|| {
            (
                header[parameter_open].span,
                "unterminated parameter list".to_owned(),
            )
        })?;
    let arrow = (parameter_close + 1..header.len())
        .find(|index| matches!(header[*index].kind, TokenKind::Arrow))
        .ok_or_else(|| {
            (
                header[parameter_close].span,
                "function declaration has no result type".to_owned(),
            )
        })?;
    let attributes_start = signature_attribute_start(header, arrow + 1);
    let mut signature = header[symbol_end..attributes_start]
        .iter()
        .map(|token| token.kind.clone())
        .collect::<Vec<_>>();
    let attributes = &header[attributes_start..];
    normalize_parameter_names(
        header,
        parameter_open,
        parameter_close,
        symbol_end,
        &mut signature,
    );

    let abi = parenthesized_value(attributes, "abi").unwrap_or_else(|| "ridl".to_owned());
    let engine = parenthesized_value(attributes, "engine");
    let target_targets = parenthesized_values(attributes, "target");
    let target_cpu = target_targets
        .as_ref()
        .map_or(true, |targets| targets.iter().any(|target| target == "cpu"));
    let target_gpu =
        target_targets.is_some_and(|targets| targets.iter().any(|target| target == "gpu"));
    let variant = parse_function_variant(attributes)?;
    if attributes
        .iter()
        .filter(|token| is_atom(token, "async"))
        .count()
        != 0
    {
        signature.push(TokenKind::Atom("async".to_owned()));
    }
    Ok(FunctionHeader {
        symbol,
        signature,
        abi,
        engine,
        target_cpu,
        target_gpu,
        variant,
        symbol_span,
    })
}

fn parse_function_variant(header: &[Token]) -> Result<Option<VariantKey>, (Span, String)> {
    let mut matches = header
        .iter()
        .enumerate()
        .filter(|(_, token)| is_atom(token, "variant"));
    let Some((index, _)) = matches.next() else {
        return Ok(None);
    };
    if let Some((_, token)) = matches.next() {
        return Err((
            token.span,
            "function has more than one variant attribute".to_owned(),
        ));
    }
    let mut cursor = index + 1;
    expect_punctuation(header, &mut cursor, '(')?;
    let architecture_name = take_atom(header, &mut cursor, "expected variant architecture")?;
    let architecture = VariantArchitecture::parse(&architecture_name).ok_or_else(|| {
        (
            header[cursor - 1].span,
            format!("unknown variant architecture {architecture_name}"),
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
        features.push(take_atom(header, &mut cursor, "expected variant feature")?);
    }
    expect_punctuation(header, &mut cursor, ')')?;
    features.sort_unstable();
    Ok(Some(VariantKey {
        architecture,
        features,
    }))
}

fn signature_attribute_start(tokens: &[Token], start: usize) -> usize {
    let mut parentheses = 0i32;
    let mut brackets = 0i32;
    let mut angles = 0i32;
    let mut braces = 0i32;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if parentheses == 0 && brackets == 0 && angles == 0 && braces == 0 {
            if is_attribute_start(tokens, index) {
                return index;
            }
        }
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '(' => parentheses += 1,
                ')' => parentheses -= 1,
                '[' => brackets += 1,
                ']' => brackets -= 1,
                '<' => angles += 1,
                '>' => angles -= 1,
                '{' => braces += 1,
                '}' => braces -= 1,
                _ => {}
            }
        }
    }
    tokens.len()
}

fn is_attribute_start(tokens: &[Token], index: usize) -> bool {
    let Some(name) = tokens.get(index).and_then(|token| atom(token)) else {
        return false;
    };
    let is_name = matches!(
        name,
        "engine"
            | "semantics"
            | "abi"
            | "target"
            | "async"
            | "coro"
            | "variant"
            | "structured"
            | "fp"
            | "inline"
            | "noinline"
            | "cold"
            | "noreturn"
            | "returns_twice"
            | "nounwind"
            | "inlinable"
            | "strategy"
            | "instance"
    ) || (name == "kernel"
        && tokens
            .get(index + 1)
            .is_some_and(|token| is_punctuation(token, '.'))
        && tokens
            .get(index + 2)
            .is_some_and(|token| is_atom(token, "fp")));
    is_name && !index_is_name_component(tokens, index)
}

fn index_is_name_component(tokens: &[Token], index: usize) -> bool {
    index > 0
        && (is_punctuation(&tokens[index - 1], '%')
            || is_punctuation(&tokens[index - 1], '$')
            || is_punctuation(&tokens[index - 1], '@')
            || is_punctuation(&tokens[index - 1], '.'))
}

fn normalize_parameter_names(
    header: &[Token],
    open: usize,
    close: usize,
    signature_start: usize,
    signature: &mut [TokenKind],
) {
    for index in open + 1..close.saturating_sub(1) {
        if is_punctuation(&header[index], '%')
            && is_punctuation(&header[index + 2], ':')
            && atom(&header[index + 1]).is_some()
        {
            let signature_index = index - signature_start + 1;
            if let Some(TokenKind::Atom(name)) = signature.get_mut(signature_index) {
                *name = "__tir_parameter".to_owned();
            }
        }
    }
}

fn parenthesized_value(tokens: &[Token], attribute: &str) -> Option<String> {
    parenthesized_values(tokens, attribute).and_then(|values| values.into_iter().next())
}

fn parenthesized_values(tokens: &[Token], attribute: &str) -> Option<Vec<String>> {
    let index = tokens.iter().position(|token| is_atom(token, attribute))?;
    let mut cursor = index + 1;
    if !is_punctuation_at(tokens, cursor, '(') {
        return None;
    }
    cursor += 1;
    let mut values = Vec::new();
    loop {
        let token = tokens.get(cursor)?;
        if is_punctuation(token, ')') {
            return Some(values);
        }
        if let Some(value) = atom(token) {
            values.push(value.to_owned());
        }
        cursor += 1;
    }
}

fn matching_delimiter(tokens: &[Token], open: usize, left: char, right: char) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if is_punctuation(token, left) {
            depth += 1;
        } else if is_punctuation(token, right) {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
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

fn significant(tokens: Vec<Token>) -> Vec<Token> {
    tokens
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect()
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

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn is_atom(token: &Token, expected: &str) -> bool {
    atom(token) == Some(expected)
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
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
