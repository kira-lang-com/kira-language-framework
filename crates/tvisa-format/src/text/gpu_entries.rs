use tir_diag::Span;
use tir_format::text::{Token, TokenKind, lex};

use super::{TextItem, TextItemKind};

/// One textual GPU entry with parsed fields and absolute source byte spans.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextGpuEntry {
    pub span: Span,
    pub kind: Option<String>,
    pub function: Option<String>,
    pub header_valid: bool,
    pub fields: Vec<TextGpuEntryField>,
    pub error: Option<TextGpuEntryError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextGpuEntryField {
    pub name: String,
    pub value: TextGpuEntryValue,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextGpuEntryValue {
    pub kind: TextGpuEntryValueKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextGpuEntryValueKind {
    Scalar(String),
    Array(Vec<TextGpuEntryValue>),
    Object(Vec<TextGpuEntryField>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextGpuEntryError {
    pub span: Span,
    pub message: String,
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    cursor: usize,
}

pub(super) fn parse_all(source: &str, items: &[TextItem]) -> Vec<TextGpuEntry> {
    items
        .iter()
        .filter(|item| item.kind == TextItemKind::Entry)
        .map(|item| {
            let text = source
                .get(item.span.start..item.span.end)
                .unwrap_or_default();
            let mut entry = parse(text, item.span);
            if let Some(error) = &mut entry.error {
                offset_span(&mut error.span, item.span.start);
            }
            for field in &mut entry.fields {
                offset_field(field, item.span.start);
            }
            entry
        })
        .collect()
}

fn offset_span(span: &mut Span, base: usize) {
    span.start = span.start.saturating_add(base);
    span.end = span.end.saturating_add(base);
}

fn offset_field(field: &mut TextGpuEntryField, base: usize) {
    offset_span(&mut field.span, base);
    offset_value(&mut field.value, base);
}

fn offset_value(value: &mut TextGpuEntryValue, base: usize) {
    offset_span(&mut value.span, base);
    match &mut value.kind {
        TextGpuEntryValueKind::Scalar(_) => {}
        TextGpuEntryValueKind::Array(values) => {
            for value in values {
                offset_value(value, base);
            }
        }
        TextGpuEntryValueKind::Object(fields) => {
            for field in fields {
                offset_field(field, base);
            }
        }
    }
}

fn parse(source: &str, item_span: Span) -> TextGpuEntry {
    let mut entry = TextGpuEntry {
        span: item_span,
        kind: None,
        function: None,
        header_valid: false,
        fields: Vec::new(),
        error: None,
    };
    let tokens = match lex(source) {
        Ok(tokens) => tokens,
        Err(error) => {
            entry.error = Some(TextGpuEntryError {
                span: error.span,
                message: error.message.to_owned(),
            });
            return entry;
        }
    };
    entry.kind = tokens.get(1).and_then(atom).map(str::to_owned);
    let Some(at) = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('@')))
    else {
        entry.error = Some(error(
            Span::new(0, source.len()),
            "missing GPU entry function",
        ));
        return entry;
    };
    let mut cursor = at + 1;
    let Some(mut function) = tokens.get(cursor).and_then(atom).map(str::to_owned) else {
        entry.error = Some(error(
            token_span(&tokens, cursor, source.len()),
            "expected GPU entry function name",
        ));
        return entry;
    };
    cursor += 1;
    while matches!(
        tokens.get(cursor).map(|token| &token.kind),
        Some(TokenKind::Punctuation('.'))
    ) {
        cursor += 1;
        let Some(part) = tokens.get(cursor).and_then(atom) else {
            entry.error = Some(error(
                token_span(&tokens, cursor, source.len()),
                "expected function name after '.'",
            ));
            return entry;
        };
        function.push('.');
        function.push_str(part);
        cursor += 1;
    }
    entry.function = Some(function);
    if !tokens
        .get(cursor)
        .and_then(atom)
        .is_some_and(|word| word == "record")
        || !matches!(
            tokens.get(cursor + 1).map(|token| &token.kind),
            Some(TokenKind::Punctuation('{'))
        )
    {
        entry.error = Some(error(
            token_span(&tokens, cursor, source.len()),
            "expected 'record {' after GPU entry function",
        ));
        return entry;
    }

    entry.header_valid = true;
    let mut parser = Parser {
        source,
        tokens,
        cursor: cursor + 2,
    };
    match parser.fields() {
        Ok(fields) => {
            parser.skip_newlines();
            if matches!(parser.peek().kind, TokenKind::End) {
                entry.fields = fields;
            } else {
                entry.error = Some(parser.error("unexpected tokens after record body"));
            }
        }
        Err(parse_error) => entry.error = Some(parse_error),
    }
    entry
}

fn error(span: Span, message: impl Into<String>) -> TextGpuEntryError {
    TextGpuEntryError {
        span,
        message: message.into(),
    }
}

fn token_span(tokens: &[Token], index: usize, source_len: usize) -> Span {
    tokens
        .get(index)
        .map_or(Span::new(source_len, source_len), |token| token.span)
}

impl Parser<'_> {
    fn fields(&mut self) -> Result<Vec<TextGpuEntryField>, TextGpuEntryError> {
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.punct('}') {
                self.cursor += 1;
                return Ok(fields);
            }
            if matches!(self.peek().kind, TokenKind::End) {
                return Err(self.error("unterminated record body"));
            }
            let start = self.peek().span.start;
            let name = self.atom()?.to_owned();
            self.expect(':')?;
            let value = self.value()?;
            fields.push(TextGpuEntryField {
                name,
                span: Span::new(start, value.span.end),
                value,
            });
            if matches!(self.peek().kind, TokenKind::Newline) {
                self.skip_newlines();
            } else if !self.punct('}') {
                return Err(self.error("GPU entry fields must be separated by newlines"));
            }
        }
    }

    fn value(&mut self) -> Result<TextGpuEntryValue, TextGpuEntryError> {
        match self.peek().kind {
            TokenKind::Punctuation('[') => self.array(),
            TokenKind::Punctuation('{') => self.object(),
            _ => self.scalar(),
        }
    }

    fn array(&mut self) -> Result<TextGpuEntryValue, TextGpuEntryError> {
        let start = self.peek().span.start;
        self.cursor += 1;
        let mut values = Vec::new();
        let mut after_comma = false;
        loop {
            self.skip_newlines();
            if self.punct(']') {
                if after_comma {
                    return Err(self.error("trailing comma in array"));
                }
                let end = self.peek().span.end;
                self.cursor += 1;
                return Ok(TextGpuEntryValue {
                    kind: TextGpuEntryValueKind::Array(values),
                    span: Span::new(start, end),
                });
            }
            values.push(self.value()?);
            after_comma = false;
            self.skip_newlines();
            if self.punct(',') {
                self.cursor += 1;
                after_comma = true;
            } else if !self.punct(']') {
                return Err(self.error("expected ',' or ']' in array"));
            }
        }
    }

    fn object(&mut self) -> Result<TextGpuEntryValue, TextGpuEntryError> {
        let start = self.peek().span.start;
        self.cursor += 1;
        let mut fields = Vec::new();
        let mut after_comma = false;
        loop {
            self.skip_newlines();
            if self.punct('}') {
                if after_comma {
                    return Err(self.error("trailing comma in object"));
                }
                let end = self.peek().span.end;
                self.cursor += 1;
                return Ok(TextGpuEntryValue {
                    kind: TextGpuEntryValueKind::Object(fields),
                    span: Span::new(start, end),
                });
            }
            let field_start = self.peek().span.start;
            let name = self.atom()?.to_owned();
            self.expect(':')?;
            let value = self.value()?;
            fields.push(TextGpuEntryField {
                name,
                span: Span::new(field_start, value.span.end),
                value,
            });
            after_comma = false;
            self.skip_newlines();
            if self.punct(',') {
                self.cursor += 1;
                after_comma = true;
            } else if !self.punct('}') {
                return Err(self.error("expected ',' or '}' in object"));
            }
        }
    }

    fn scalar(&mut self) -> Result<TextGpuEntryValue, TextGpuEntryError> {
        let first = self.peek().span;
        if matches!(self.peek().kind, TokenKind::String(_)) {
            self.cursor += 1;
            return Ok(TextGpuEntryValue {
                kind: TextGpuEntryValueKind::Scalar(self.text(first).to_owned()),
                span: first,
            });
        }
        let mut angle = 0usize;
        let mut paren = 0usize;
        let mut last = first;
        let mut consumed = false;
        while let Some(token) = self.tokens.get(self.cursor) {
            match token.kind {
                TokenKind::Newline | TokenKind::End if angle == 0 && paren == 0 => break,
                TokenKind::Punctuation(',')
                | TokenKind::Punctuation(']')
                | TokenKind::Punctuation('}')
                    if angle == 0 && paren == 0 =>
                {
                    break;
                }
                TokenKind::Punctuation('<') => angle += 1,
                TokenKind::Punctuation('>') => angle = angle.saturating_sub(1),
                TokenKind::Punctuation('(') => paren += 1,
                TokenKind::Punctuation(')') => paren = paren.saturating_sub(1),
                _ => {}
            }
            last = token.span;
            self.cursor += 1;
            consumed = true;
        }
        if !consumed {
            return Err(self.error("expected a value"));
        }
        Ok(TextGpuEntryValue {
            kind: TextGpuEntryValueKind::Scalar(
                self.source[first.start..last.end].trim().to_owned(),
            ),
            span: Span::new(first.start, last.end),
        })
    }

    fn atom(&mut self) -> Result<&str, TextGpuEntryError> {
        let token = self
            .tokens
            .get(self.cursor)
            .ok_or_else(|| self.error("expected identifier"))?;
        if let TokenKind::Atom(atom) = &token.kind {
            self.cursor += 1;
            Ok(atom)
        } else {
            Err(self.error("expected identifier"))
        }
    }

    fn expect(&mut self, punctuation: char) -> Result<(), TextGpuEntryError> {
        if self.punct(punctuation) {
            self.cursor += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected '{punctuation}'")))
        }
    }

    fn punct(&self, punctuation: char) -> bool {
        matches!(
            self.tokens.get(self.cursor).map(|token| &token.kind),
            Some(TokenKind::Punctuation(actual)) if *actual == punctuation
        )
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek().kind, TokenKind::Newline) {
            self.cursor += 1;
        }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.cursor.min(self.tokens.len() - 1)]
    }

    fn text(&self, span: Span) -> &str {
        self.source.get(span.start..span.end).unwrap_or_default()
    }

    fn error(&self, message: &str) -> TextGpuEntryError {
        TextGpuEntryError {
            span: self.peek().span,
            message: message.to_owned(),
        }
    }
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}
