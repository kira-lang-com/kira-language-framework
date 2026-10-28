//! Tokens of textual KSIR.

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Ident(String),
    Number(String),
    Str(String),
    Percent,
    At,
    Dollar,
    Bang,
    Arrow,
    Punct(char),
}

pub struct Lexed {
    pub tokens: Vec<(Token, u32)>,
}

/// Splits `text` into tokens with their line numbers.
///
/// # Errors
/// Returns the line of an unterminated string or a stray character.
pub fn lex(text: &str) -> Result<Lexed, String> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    let mut line = 1;
    while at < bytes.len() {
        let byte = bytes[at];
        match byte {
            b'\n' => {
                line += 1;
                at += 1;
            }
            b' ' | b'\t' | b'\r' => at += 1,
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
            }
            b'"' => {
                let mut value = String::new();
                at += 1;
                loop {
                    match bytes.get(at) {
                        None | Some(b'\n') => {
                            return Err(format!("line {line}: unterminated string"));
                        }
                        Some(b'"') => {
                            at += 1;
                            break;
                        }
                        Some(b'\\') => {
                            let escaped = *bytes
                                .get(at + 1)
                                .ok_or(format!("line {line}: bad escape"))?;
                            value.push(match escaped {
                                b'n' => '\n',
                                b't' => '\t',
                                other => char::from(other),
                            });
                            at += 2;
                        }
                        Some(_) => {
                            let rest = &text[at..];
                            let character = rest.chars().next().unwrap_or(' ');
                            value.push(character);
                            at += character.len_utf8();
                        }
                    }
                }
                tokens.push((Token::Str(value), line));
            }
            b'-' if bytes.get(at + 1) == Some(&b'>') => {
                tokens.push((Token::Arrow, line));
                at += 2;
            }
            b'0'..=b'9' | b'-' => {
                let start = at;
                at += 1;
                while at < bytes.len() {
                    let next = bytes[at];
                    let exponent_sign = matches!(next, b'+' | b'-')
                        && matches!(bytes[at - 1], b'e' | b'E')
                        && !text[start..at].contains(['x', 'X']);
                    if next.is_ascii_alphanumeric() || next == b'.' || next == b'_' || exponent_sign
                    {
                        at += 1;
                    } else {
                        break;
                    }
                }
                tokens.push((Token::Number(text[start..at].to_owned()), line));
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => {
                // A word may join letters with `-`, as `storage-image-read-write`.
                let start = at;
                while at < bytes.len()
                    && (bytes[at].is_ascii_alphanumeric()
                        || bytes[at] == b'_'
                        || bytes[at] == b'.'
                        || (bytes[at] == b'-'
                            && bytes.get(at + 1).is_some_and(u8::is_ascii_alphabetic)))
                {
                    at += 1;
                }
                tokens.push((Token::Ident(text[start..at].to_owned()), line));
            }
            b'%' => {
                tokens.push((Token::Percent, line));
                at += 1;
            }
            b'@' => {
                tokens.push((Token::At, line));
                at += 1;
            }
            b'$' => {
                tokens.push((Token::Dollar, line));
                at += 1;
            }
            b'!' => {
                tokens.push((Token::Bang, line));
                at += 1;
            }
            b'{' | b'}' | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b',' | b':' | b'=' => {
                tokens.push((Token::Punct(char::from(byte)), line));
                at += 1;
            }
            other => return Err(format!("line {line}: unexpected `{}`", char::from(other))),
        }
    }
    Ok(Lexed { tokens })
}
