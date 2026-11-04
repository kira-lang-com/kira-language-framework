use tir_format::text::{Token, TokenKind, lex};

pub(super) fn function_address_info(source: &str) -> (bool, Option<u32>) {
    let Some(tokens) = significant_tokens(source) else {
        return (false, None);
    };
    let header_end = tokens
        .iter()
        .position(|token| punctuation(token, '{'))
        .unwrap_or(tokens.len());
    let header = &tokens[..header_end];
    let address_taken = header
        .iter()
        .any(|token| atom(token) == Some("address_taken"));
    let tag = header.windows(2).find_map(|pair| {
        (atom(&pair[0]) == Some("tag"))
            .then(|| hex_u32(&pair[1]))
            .flatten()
    });
    (address_taken, tag)
}

fn significant_tokens(source: &str) -> Option<Vec<Token>> {
    Some(
        lex(source)
            .ok()?
            .into_iter()
            .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
            .collect(),
    )
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn hex_u32(token: &Token) -> Option<u32> {
    let value = atom(token)?
        .strip_prefix("0x")
        .or_else(|| atom(token)?.strip_prefix("0X"))?;
    u32::from_str_radix(value, 16).ok()
}
