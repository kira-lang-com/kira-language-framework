use klf_core::FileId;
use klf_diag::Diagnostic;
use klf_source::Span;

const PARSE_ERR: &str = "KLF-RIDL-0001";

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Tok {
    Ident(String),
    Str(String),
    Number(String),
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Lt,
    Gt,
    Colon,
    Comma,
    Eq,
    Arrow,
    Dot,
    At,
    Eof,
}

#[derive(Clone, Debug)]
pub(super) struct Spanned {
    pub(super) tok: Tok,
    pub(super) span: Span,
}

fn span_at(file: FileId, start: usize, end: usize) -> Span {
    let clamp = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    Span::new(file, clamp(start), clamp(end))
}

fn skip_trivia(file: FileId, bytes: &[u8], i: &mut usize) -> Result<bool, Diagnostic> {
    let start = *i;
    if bytes[*i].is_ascii_whitespace() {
        *i += 1;
        return Ok(true);
    }
    if bytes[*i] == b'/' && *i + 1 < bytes.len() && bytes[*i + 1] == b'/' {
        while *i < bytes.len() && bytes[*i] != b'\n' {
            *i += 1;
        }
        return Ok(true);
    }
    if bytes[*i] == b'/' && *i + 1 < bytes.len() && bytes[*i + 1] == b'*' {
        *i += 2;
        let mut depth = 1;
        while *i < bytes.len() && depth > 0 {
            if bytes[*i] == b'/' && *i + 1 < bytes.len() && bytes[*i + 1] == b'*' {
                depth += 1;
                *i += 2;
            } else if bytes[*i] == b'*' && *i + 1 < bytes.len() && bytes[*i + 1] == b'/' {
                depth -= 1;
                *i += 2;
            } else {
                *i += 1;
            }
        }
        if depth != 0 {
            return Err(Diagnostic::error(PARSE_ERR, "unterminated block comment")
                .with_label(span_at(file, start, *i), "comment starts here"));
        }
        return Ok(true);
    }
    Ok(false)
}

pub(super) fn lex(file: FileId, text: &str) -> Result<Vec<Spanned>, Diagnostic> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < bytes.len() {
        if skip_trivia(file, bytes, &mut i)? {
            continue;
        }
        let c = bytes[i];
        let start = i;
        let two = |a: u8, b: u8| c == a && i + 1 < bytes.len() && bytes[i + 1] == b;
        let simple = |t: Tok| Some(t);
        let tok = if two(b'-', b'>') {
            i += 2;
            simple(Tok::Arrow)
        } else {
            let t = match c {
                b'{' => simple(Tok::LBrace),
                b'}' => simple(Tok::RBrace),
                b'(' => simple(Tok::LParen),
                b')' => simple(Tok::RParen),
                b'[' => simple(Tok::LBracket),
                b']' => simple(Tok::RBracket),
                b'<' => simple(Tok::Lt),
                b'>' => simple(Tok::Gt),
                b':' => simple(Tok::Colon),
                b',' => simple(Tok::Comma),
                b'=' => simple(Tok::Eq),
                b'.' => simple(Tok::Dot),
                b'@' => simple(Tok::At),
                _ => None,
            };
            if t.is_some() {
                i += 1;
            }
            t
        };
        if let Some(t) = tok {
            out.push(Spanned {
                tok: t,
                span: span_at(file, start, i),
            });
            continue;
        }
        if c == b'"' {
            i += 1;
            let mut s = String::new();
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                }
                s.push(bytes[i] as char);
                i += 1;
            }
            if i >= bytes.len() {
                return Err(Diagnostic::error(PARSE_ERR, "unterminated string literal")
                    .with_label(span_at(file, start, i), "string starts here"));
            }
            i += 1;
            out.push(Spanned {
                tok: Tok::Str(s),
                span: span_at(file, start, i),
            });
            continue;
        }
        if c.is_ascii_digit() || (c == b'-' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
        {
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            out.push(Spanned {
                tok: Tok::Number(text[start..i].to_owned()),
                span: span_at(file, start, i),
            });
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.push(Spanned {
                tok: Tok::Ident(text[start..i].to_owned()),
                span: span_at(file, start, i),
            });
            continue;
        }
        return Err(
            Diagnostic::error(PARSE_ERR, format!("unexpected character `{}`", c as char))
                .with_label(span_at(file, start, i + 1), "here"),
        );
    }
    out.push(Spanned {
        tok: Tok::Eof,
        span: span_at(file, bytes.len(), bytes.len()),
    });
    Ok(out)
}

pub(super) fn describe(tok: &Tok) -> String {
    match tok {
        Tok::Ident(s) => s.clone(),
        Tok::Str(s) => format!("\"{s}\""),
        Tok::Number(n) => n.clone(),
        Tok::LBrace => "{".to_owned(),
        Tok::RBrace => "}".to_owned(),
        Tok::LParen => "(".to_owned(),
        Tok::RParen => ")".to_owned(),
        Tok::LBracket => "[".to_owned(),
        Tok::RBracket => "]".to_owned(),
        Tok::Lt => "<".to_owned(),
        Tok::Gt => ">".to_owned(),
        Tok::Colon => ":".to_owned(),
        Tok::Comma => ",".to_owned(),
        Tok::Eq => "=".to_owned(),
        Tok::Arrow => "->".to_owned(),
        Tok::Dot => ".".to_owned(),
        Tok::At => "@".to_owned(),
        Tok::Eof => "end of input".to_owned(),
    }
}
