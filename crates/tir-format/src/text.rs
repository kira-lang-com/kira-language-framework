use std::fmt;

use tir_diag::Span;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Atom(String),
    /// The raw contents between quotes, with backslash escapes retained.
    String(String),
    Punctuation(char),
    Arrow,
    Newline,
    End,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteStringEscapeError {
    TrailingBackslash,
    UnknownEscape(char),
    InvalidHexEscape,
    InvalidUnicodeEscape,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KiraStringEscapeError {
    TrailingBackslash,
    UnknownEscape(char),
    CarriageReturnWithoutLineFeed,
}

impl fmt::Display for KiraStringEscapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrailingBackslash => formatter.write_str("trailing backslash in string"),
            Self::UnknownEscape(character) => {
                write!(formatter, "unknown Kira string escape `\\{character}`")
            }
            Self::CarriageReturnWithoutLineFeed => {
                formatter.write_str("string continuation must use LF or CRLF")
            }
        }
    }
}

impl fmt::Display for ByteStringEscapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrailingBackslash => formatter.write_str("trailing backslash in byte string"),
            Self::UnknownEscape(character) => {
                write!(formatter, "unknown byte-string escape `\\{character}`")
            }
            Self::InvalidHexEscape => {
                formatter.write_str("byte-string `\\x` escape requires two hexadecimal digits")
            }
            Self::InvalidUnicodeEscape => formatter.write_str(
                "byte-string `\\u` escape must contain a valid Unicode scalar in braces",
            ),
        }
    }
}

pub fn decode_kira_byte_string(raw: &str) -> Result<Vec<u8>, ByteStringEscapeError> {
    let mut output = Vec::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            push_utf8(character, &mut output);
            continue;
        }
        match chars
            .next()
            .ok_or(ByteStringEscapeError::TrailingBackslash)?
        {
            'n' => output.push(b'\n'),
            't' => output.push(b'\t'),
            'r' => output.push(b'\r'),
            '0' => output.push(0),
            '\\' => output.push(b'\\'),
            '"' => output.push(b'"'),
            'x' => {
                let high = chars
                    .next()
                    .and_then(|digit| digit.to_digit(16))
                    .ok_or(ByteStringEscapeError::InvalidHexEscape)?;
                let low = chars
                    .next()
                    .and_then(|digit| digit.to_digit(16))
                    .ok_or(ByteStringEscapeError::InvalidHexEscape)?;
                output.push(((high << 4) | low) as u8);
            }
            'u' => {
                if chars.next() != Some('{') {
                    return Err(ByteStringEscapeError::InvalidUnicodeEscape);
                }
                let mut scalar = 0_u32;
                let mut digits = 0_u32;
                loop {
                    match chars.next() {
                        Some('}') if digits > 0 => break,
                        Some(character) => {
                            let digit = character
                                .to_digit(16)
                                .ok_or(ByteStringEscapeError::InvalidUnicodeEscape)?;
                            scalar = scalar
                                .checked_mul(16)
                                .and_then(|value| value.checked_add(digit))
                                .ok_or(ByteStringEscapeError::InvalidUnicodeEscape)?;
                            digits += 1;
                        }
                        None => return Err(ByteStringEscapeError::InvalidUnicodeEscape),
                    }
                }
                let character =
                    char::from_u32(scalar).ok_or(ByteStringEscapeError::InvalidUnicodeEscape)?;
                push_utf8(character, &mut output);
            }
            other => return Err(ByteStringEscapeError::UnknownEscape(other)),
        }
    }
    Ok(output)
}

fn push_utf8(character: char, output: &mut Vec<u8>) {
    let mut buffer = [0; 4];
    output.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
}

pub fn decode_kira_string_literal_contents(raw: &str) -> Result<String, KiraStringEscapeError> {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        match chars
            .next()
            .ok_or(KiraStringEscapeError::TrailingBackslash)?
        {
            'n' => output.push('\n'),
            't' => output.push('\t'),
            'r' => output.push('\r'),
            'e' => output.push('\x1b'),
            '0' => output.push('\0'),
            '"' => output.push('"'),
            '\\' => output.push('\\'),
            '\n' => skip_string_continuation_indent(&mut chars),
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                skip_string_continuation_indent(&mut chars);
            }
            '\r' => return Err(KiraStringEscapeError::CarriageReturnWithoutLineFeed),
            other => return Err(KiraStringEscapeError::UnknownEscape(other)),
        }
    }
    Ok(output)
}

fn skip_string_continuation_indent(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while chars
        .peek()
        .is_some_and(|character| character.is_whitespace())
    {
        chars.next();
    }
}

#[must_use]
pub fn decode_kira_string_escapes(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('n') => output.push('\n'),
            Some('t') => output.push('\t'),
            Some('r') => output.push('\r'),
            Some('0') => output.push('\0'),
            Some('\\') => output.push('\\'),
            Some('"') => output.push('"'),
            Some('u') => decode_unicode_escape(&mut chars, &mut output),
            Some(other) => output.push(other),
            None => {}
        }
    }
    output
}

fn decode_unicode_escape(chars: &mut std::str::Chars<'_>, output: &mut String) {
    if chars.clone().next() != Some('{') {
        output.push('u');
        return;
    }
    chars.next();
    let mut hex = String::new();
    for character in chars.by_ref() {
        if character == '}' {
            break;
        }
        hex.push(character);
    }
    if let Some(character) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
        output.push(character);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextError {
    pub span: Span,
    pub message: &'static str,
}

impl fmt::Display for TextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for TextError {}

/// Splits TIR or TVISA text into span-aware tokens and retains line breaks.
pub fn lex(source: &str) -> Result<Vec<Token>, TextError> {
    let mut tokens = Vec::new();
    let mut cursor = 0usize;
    while cursor < source.len() {
        let character = character_at(source, cursor);
        match character {
            '\n' => {
                tokens.push(Token {
                    kind: TokenKind::Newline,
                    span: Span::new(cursor, cursor + 1),
                });
                cursor += 1;
            }
            '\r' => {
                let length = if source.as_bytes().get(cursor + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                tokens.push(Token {
                    kind: TokenKind::Newline,
                    span: Span::new(cursor, cursor + length),
                });
                cursor += length;
            }
            ';' => {
                while cursor < source.len() && !matches!(character_at(source, cursor), '\n' | '\r')
                {
                    cursor += character_at(source, cursor).len_utf8();
                }
            }
            '"' => {
                let start = cursor;
                cursor += 1;
                let content_start = cursor;
                let mut escaped = false;
                let mut closed = false;
                while cursor < source.len() {
                    let current = character_at(source, cursor);
                    if escaped {
                        escaped = false;
                        if current == '\r' && source.as_bytes().get(cursor + 1) == Some(&b'\n') {
                            cursor += 2;
                        } else {
                            cursor += current.len_utf8();
                        }
                    } else if current == '\\' {
                        escaped = true;
                        cursor += 1;
                    } else if matches!(current, '\n' | '\r') {
                        return Err(TextError {
                            span: Span::new(start, cursor),
                            message: "newline in string literal",
                        });
                    } else if current == '"' {
                        let raw = source[content_start..cursor].to_owned();
                        cursor += 1;
                        tokens.push(Token {
                            kind: TokenKind::String(raw),
                            span: Span::new(start, cursor),
                        });
                        closed = true;
                        break;
                    } else {
                        cursor += current.len_utf8();
                    }
                }
                if !closed {
                    return Err(TextError {
                        span: Span::new(start, source.len()),
                        message: "unterminated string literal",
                    });
                }
            }
            character if character.is_whitespace() => {
                cursor += character.len_utf8();
            }
            '-' if source[cursor..].starts_with("->") => {
                tokens.push(Token {
                    kind: TokenKind::Arrow,
                    span: Span::new(cursor, cursor + 2),
                });
                cursor += 2;
            }
            character if punctuation(character) => {
                tokens.push(Token {
                    kind: TokenKind::Punctuation(character),
                    span: Span::new(cursor, cursor + character.len_utf8()),
                });
                cursor += character.len_utf8();
            }
            _ => {
                let start = cursor;
                while cursor < source.len() {
                    let current = character_at(source, cursor);
                    if current.is_whitespace()
                        || punctuation(current)
                        || current == ';'
                        || current == '"'
                    {
                        break;
                    }
                    cursor += current.len_utf8();
                }
                if cursor == start {
                    cursor += character_at(source, cursor).len_utf8();
                }
                tokens.push(Token {
                    kind: TokenKind::Atom(source[start..cursor].to_owned()),
                    span: Span::new(start, cursor),
                });
            }
        }
    }
    tokens.push(Token {
        kind: TokenKind::End,
        span: Span::new(source.len(), source.len()),
    });
    Ok(tokens)
}

fn character_at(source: &str, offset: usize) -> char {
    source[offset..].chars().next().unwrap_or('\0')
}

const fn punctuation(character: char) -> bool {
    matches!(
        character,
        '@' | '%'
            | '$'
            | ':'
            | ','
            | '.'
            | '='
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '<'
            | '>'
            | '+'
            | '-'
            | '*'
            | '/'
            | '!'
            | '&'
            | '|'
    )
}
