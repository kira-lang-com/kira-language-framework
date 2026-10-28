//! Logical lines and preprocessing tokens. Comments and backslash-newline
//! splices are removed here, so the directive and expansion code sees one
//! logical line at a time with the physical line it began on.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Ident,
    Number,
    Literal,
    Punct,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Token {
    pub(crate) kind: Kind,
    pub(crate) text: String,
    pub(crate) line: usize,
    /// Whether whitespace came before this token in its source, so rendering
    /// keeps the spelling the header was written with.
    pub(crate) spaced: bool,
    /// Macros this token may no longer expand to (the hide set of Prosser's
    /// algorithm), so a self-referential macro stops after one expansion.
    pub(crate) hide: Vec<String>,
}

impl Token {
    pub(crate) fn new(kind: Kind, text: impl Into<String>, line: usize) -> Self {
        Self {
            kind,
            text: text.into(),
            line,
            spaced: false,
            hide: Vec::new(),
        }
    }

    pub(crate) fn is(&self, punct: &str) -> bool {
        self.kind == Kind::Punct && self.text == punct
    }
}

/// A preprocessing failure at a physical line of the file being read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Fault {
    pub(crate) line: usize,
    pub(crate) message: String,
}

impl Fault {
    pub(crate) fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

pub(crate) struct Logical {
    pub(crate) line: usize,
    pub(crate) text: String,
}

const MULTI_PUNCT: [&str; 13] = [
    "...", "##", "->", "<<", ">>", "<=", ">=", "==", "!=", "&&", "||", "++", "--",
];

const STRING_PREFIXES: [&str; 4] = ["L", "u", "U", "u8"];

/// The logical lines of `text`, comments replaced by a space and each line
/// spliced with the one after a trailing backslash.
pub(crate) fn logical_lines(text: &str) -> Result<Vec<Logical>, Fault> {
    let chars: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut start = 1;
    let mut line = 1;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '\\' && next == Some('\n') {
            i += 2;
            line += 1;
            continue;
        }
        if c == '\\' && next == Some('\r') && chars.get(i + 2) == Some(&'\n') {
            i += 3;
            line += 1;
            continue;
        }
        match (c, next) {
            ('\n', _) => {
                lines.push(Logical {
                    line: start,
                    text: std::mem::take(&mut current),
                });
                line += 1;
                start = line;
                i += 1;
            }
            ('/', Some('*')) => {
                let opened = line;
                i += 2;
                loop {
                    match (chars.get(i), chars.get(i + 1)) {
                        (None, _) => {
                            return Err(Fault::new(opened, "unterminated `/*` comment"));
                        }
                        (Some('*'), Some('/')) => {
                            i += 2;
                            break;
                        }
                        (Some('\n'), _) => {
                            line += 1;
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                current.push(' ');
            }
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ('"' | '\'', _) => {
                i = copy_literal(&chars, i, line, &mut current)?;
            }
            _ => {
                current.push(c);
                i += 1;
            }
        }
    }
    lines.push(Logical {
        line: start,
        text: current,
    });
    Ok(lines)
}

/// Copies a string or character literal starting at `start` and returns the
/// index after its closing quote.
fn copy_literal(
    chars: &[char],
    start: usize,
    line: usize,
    out: &mut String,
) -> Result<usize, Fault> {
    let quote = chars[start];
    let mut i = start;
    out.push(quote);
    i += 1;
    loop {
        match chars.get(i) {
            None | Some('\n') => {
                return Err(Fault::new(line, "unterminated string or character literal"));
            }
            Some('\\') => {
                out.push('\\');
                if let Some(&escaped) = chars.get(i + 1) {
                    out.push(escaped);
                }
                i += 2;
            }
            Some(&ch) => {
                out.push(ch);
                i += 1;
                if ch == quote {
                    return Ok(i);
                }
            }
        }
    }
}

/// The preprocessing tokens of one logical line.
pub(crate) fn tokenize(text: &str, line: usize) -> Result<Vec<Token>, Fault> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut spaced = true;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            spaced = true;
            i += 1;
            continue;
        }
        let start = i;
        let (end, kind) = if c.is_ascii_digit()
            || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            (number_end(&chars, i), Kind::Number)
        } else if c.is_ascii_alphabetic() || c == '_' {
            let word_end = ident_end(&chars, i);
            let word: String = chars[i..word_end].iter().collect();
            let prefixed = matches!(chars.get(word_end), Some('"' | '\''))
                && STRING_PREFIXES.contains(&word.as_str());
            if prefixed {
                (literal_end(&chars, word_end, line)?, Kind::Literal)
            } else {
                (word_end, Kind::Ident)
            }
        } else if c == '"' || c == '\'' {
            (literal_end(&chars, i, line)?, Kind::Literal)
        } else {
            (punct_end(&chars, i), Kind::Punct)
        };
        let mut token = Token::new(kind, chars[start..end].iter().collect::<String>(), line);
        token.spaced = spaced;
        spaced = false;
        tokens.push(token);
        i = end;
    }
    Ok(tokens)
}

fn ident_end(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
        i += 1;
    }
    i
}

fn number_end(chars: &[char], start: usize) -> usize {
    let mut i = start + 1;
    while i < chars.len() {
        let c = chars[i];
        let exponent_sign = matches!(c, '+' | '-') && matches!(chars[i - 1], 'e' | 'E' | 'p' | 'P');
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' || exponent_sign {
            i += 1;
        } else {
            break;
        }
    }
    i
}

fn literal_end(chars: &[char], quote_at: usize, line: usize) -> Result<usize, Fault> {
    copy_literal(chars, quote_at, line, &mut String::new())
}

fn punct_end(chars: &[char], start: usize) -> usize {
    for multi in MULTI_PUNCT {
        let len = multi.chars().count();
        if multi
            .chars()
            .enumerate()
            .all(|(k, ch)| chars.get(start + k) == Some(&ch))
        {
            return start + len;
        }
    }
    start + 1
}

/// The value of an integer constant (C17 6.4.4.1) without its suffix, read
/// as a signed 64-bit integer, or `None` when the text is not one.
pub(crate) fn integer_literal(text: &str) -> Option<i64> {
    let digits = text.trim_end_matches(['u', 'U', 'l', 'L']);
    let value = if let Some(hex) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()?
    } else if let Some(binary) = digits.strip_prefix("0b").or_else(|| digits.strip_prefix("0B")) {
        u64::from_str_radix(binary, 2).ok()?
    } else if digits.len() > 1 && digits.starts_with('0') {
        u64::from_str_radix(&digits[1..], 8).ok()?
    } else {
        digits.parse::<u64>().ok()?
    };
    Some(value.cast_signed())
}
