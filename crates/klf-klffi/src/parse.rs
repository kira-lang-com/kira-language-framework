//! Lexer and recursive-descent parser for grammar A.3 (`.klffi`). Errors report
//! in the `KLF-FFI-00xx` band with a source line.

use crate::model::{
    Annotation, AnnotationKind, FfiConstant, FfiField, FfiFunction, FfiItem, FfiModule, FfiStruct,
};

const PARSE_ERR: &str = "KLF-FFI-0001";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FfiError {
    pub code: String,
    pub message: String,
    pub line: usize,
}

impl std::fmt::Display for FfiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "<klffi>:{}: error[{}]: {}",
            self.line, self.code, self.message
        )
    }
}

impl std::error::Error for FfiError {}

fn err(line: usize, message: impl Into<String>) -> FfiError {
    FfiError {
        code: PARSE_ERR.to_owned(),
        message: message.into(),
        line,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Tok {
    Ann(String),
    Ident(String),
    Int(String),
    Str(String),
    Arrow,
    Punct(char),
}

#[derive(Clone, Debug)]
struct Spanned {
    tok: Tok,
    line: usize,
}

fn skip_block_comment(bytes: &[u8], i: &mut usize, line: &mut usize) -> Result<(), FfiError> {
    *i += 2;
    let mut depth = 1;
    while *i < bytes.len() && depth > 0 {
        if bytes[*i] == b'\n' {
            *line += 1;
        }
        if *i + 1 < bytes.len() && bytes[*i] == b'*' && bytes[*i + 1] == b'/' {
            depth -= 1;
            *i += 2;
        } else if *i + 1 < bytes.len() && bytes[*i] == b'/' && bytes[*i + 1] == b'*' {
            depth += 1;
            *i += 2;
        } else {
            *i += 1;
        }
    }
    if depth > 0 {
        return Err(err(*line, "unterminated block comment"));
    }
    Ok(())
}

fn lex(text: &str) -> Result<Vec<Spanned>, FfiError> {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut line = 1;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b' ' | b'\t' | b'\r' => i += 1,
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                skip_block_comment(bytes, &mut i, &mut line)?;
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'>' => {
                out.push(Spanned {
                    tok: Tok::Arrow,
                    line,
                });
                i += 2;
            }
            b'@' => {
                let start = i;
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                    i += 1;
                }
                let word = &text[start..i];
                // `@Bits` follows a bit field; every other annotation is `@FFI.*`.
                let name = match word {
                    "@Bits" => "Bits",
                    _ => word
                        .strip_prefix("@FFI.")
                        .ok_or_else(|| err(line, format!("unknown annotation `{word}`")))?,
                };
                out.push(Spanned {
                    tok: Tok::Ann(name.to_owned()),
                    line,
                });
            }
            b'"' => {
                let start = i + 1;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\n' {
                        return Err(err(line, "unterminated string literal"));
                    }
                    i += 1;
                }
                if i >= bytes.len() {
                    return Err(err(line, "unterminated string literal"));
                }
                out.push(Spanned {
                    tok: Tok::Str(text[start..i].to_owned()),
                    line,
                });
                i += 1;
            }
            b'{' | b'}' | b'(' | b')' | b'[' | b']' | b',' | b':' | b'=' => {
                out.push(Spanned {
                    tok: Tok::Punct(c as char),
                    line,
                });
                i += 1;
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'x')
                {
                    i += 1;
                }
                out.push(Spanned {
                    tok: Tok::Int(text[start..i].to_owned()),
                    line,
                });
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                out.push(Spanned {
                    tok: Tok::Ident(text[start..i].to_owned()),
                    line,
                });
            }
            _ => return Err(err(line, format!("unexpected character `{}`", c as char))),
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Spanned>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|s| &s.tok)
    }

    fn line(&self) -> usize {
        self.toks
            .get(self.pos)
            .or_else(|| self.toks.last())
            .map_or(1, |s| s.line)
    }

    fn bump(&mut self) -> Option<Tok> {
        let tok = self.toks.get(self.pos).map(|s| s.tok.clone());
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn eat_punct(&mut self, c: char) -> bool {
        if matches!(self.peek(), Some(Tok::Punct(p)) if *p == c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, c: char) -> Result<(), FfiError> {
        if self.eat_punct(c) {
            Ok(())
        } else {
            Err(err(self.line(), format!("expected `{c}`")))
        }
    }

    fn ident(&mut self) -> Result<String, FfiError> {
        match self.bump() {
            Some(Tok::Ident(name)) => Ok(name),
            _ => Err(err(self.line(), "expected an identifier")),
        }
    }
}

/// Parse a `.klffi` module from source text.
///
/// # Errors
/// Returns [`FfiError`] on any lexical or structural error.
pub fn parse(text: &str) -> Result<FfiModule, FfiError> {
    let toks = lex(text)?;
    let mut parser = Parser { toks, pos: 0 };
    let mut items = Vec::new();
    while parser.peek().is_some() {
        items.push(parse_item(&mut parser)?);
    }
    Ok(FfiModule { items })
}

fn parse_item(parser: &mut Parser) -> Result<FfiItem, FfiError> {
    if matches!(parser.peek(), Some(Tok::Ident(word)) if word == "let") {
        return parse_constant(parser).map(FfiItem::Constant);
    }
    let mut annotations = Vec::new();
    while let Some(Tok::Ann(_)) = parser.peek() {
        annotations.push(parse_annotation(parser)?);
    }
    if annotations.is_empty() {
        return Err(err(
            parser.line(),
            "a declaration needs an `@FFI.` annotation",
        ));
    }
    match parser.bump() {
        Some(Tok::Ident(word)) if word == "struct" => {
            parse_struct(parser, annotations).map(FfiItem::Struct)
        }
        Some(Tok::Ident(word)) if word == "function" => {
            parse_function(parser, annotations).map(FfiItem::Function)
        }
        _ => Err(err(
            parser.line(),
            "expected `struct` or `function` after annotations",
        )),
    }
}

fn parse_annotation(parser: &mut Parser) -> Result<Annotation, FfiError> {
    let Some(Tok::Ann(name)) = parser.bump() else {
        return Err(err(parser.line(), "expected an annotation"));
    };
    let kind = AnnotationKind::from_spelling(&name)
        .ok_or_else(|| err(parser.line(), format!("unknown annotation `@FFI.{name}`")))?;
    let mut pairs = Vec::new();
    if parser.eat_punct('{') && !parser.eat_punct('}') {
        loop {
            let key = parser.ident()?;
            parser.expect_punct(':')?;
            let value = parse_value(parser)?;
            pairs.push((key, value));
            if parser.eat_punct(',') {
                continue;
            }
            parser.expect_punct('}')?;
            break;
        }
    }
    Ok(Annotation { kind, pairs })
}

fn parse_value(parser: &mut Parser) -> Result<String, FfiError> {
    if parser.eat_punct('[') {
        let mut elements = Vec::new();
        if !parser.eat_punct(']') {
            loop {
                elements.push(parse_scalar_value(parser)?);
                if parser.eat_punct(',') {
                    continue;
                }
                parser.expect_punct(']')?;
                break;
            }
        }
        return Ok(format!("[{}]", elements.join(", ")));
    }
    parse_scalar_value(parser)
}

fn parse_scalar_value(parser: &mut Parser) -> Result<String, FfiError> {
    match parser.bump() {
        Some(Tok::Ident(text) | Tok::Int(text)) => Ok(text),
        Some(Tok::Str(text)) => Ok(format!("\"{text}\"")),
        _ => Err(err(parser.line(), "expected a value")),
    }
}

fn parse_struct(parser: &mut Parser, annotations: Vec<Annotation>) -> Result<FfiStruct, FfiError> {
    let name = parser.ident()?;
    let mut fields = Vec::new();
    if parser.eat_punct('{') {
        while !parser.eat_punct('}') {
            let keyword = parser.ident()?;
            if keyword != "let" {
                return Err(err(parser.line(), "struct fields start with `let`"));
            }
            let field_name = parser.ident()?;
            parser.expect_punct(':')?;
            let ty = parser.ident()?;
            let mut bits = None;
            if matches!(parser.peek(), Some(Tok::Ann(a)) if a == "Bits") {
                parser.bump();
                parser.expect_punct('(')?;
                let Some(Tok::Int(text)) = parser.bump() else {
                    return Err(err(parser.line(), "`@Bits` needs an integer"));
                };
                bits = Some(
                    text.parse()
                        .map_err(|_| err(parser.line(), "`@Bits` width is not an integer"))?,
                );
                parser.expect_punct(')')?;
            }
            fields.push(FfiField {
                name: field_name,
                ty,
                bits,
            });
        }
    }
    Ok(FfiStruct {
        annotations,
        name,
        fields,
    })
}

fn parse_function(
    parser: &mut Parser,
    annotations: Vec<Annotation>,
) -> Result<FfiFunction, FfiError> {
    let name = parser.ident()?;
    parser.expect_punct('(')?;
    let mut parameters = Vec::new();
    if !parser.eat_punct(')') {
        loop {
            let param = parser.ident()?;
            parser.expect_punct(':')?;
            let ty = parser.ident()?;
            parameters.push((param, ty));
            if parser.eat_punct(',') {
                continue;
            }
            parser.expect_punct(')')?;
            break;
        }
    }
    let result = if matches!(parser.peek(), Some(Tok::Arrow)) {
        parser.bump();
        parser.ident()?
    } else {
        "Void".to_owned()
    };
    Ok(FfiFunction {
        annotations,
        name,
        parameters,
        result,
    })
}

fn parse_constant(parser: &mut Parser) -> Result<FfiConstant, FfiError> {
    parser.bump();
    let name = parser.ident()?;
    parser.expect_punct(':')?;
    let ty = parser.ident()?;
    parser.expect_punct('=')?;
    let value = parse_scalar_value(parser)?;
    Ok(FfiConstant { name, ty, value })
}
