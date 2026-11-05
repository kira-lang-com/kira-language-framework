use tir_diag::Span;
use tir_format::text::{Token, TokenKind, lex};

#[derive(Clone)]
pub(super) struct Field {
    pub(super) name: String,
    pub(super) value: Value,
    pub(super) span: Span,
}

#[derive(Clone)]
pub(super) struct Value {
    pub(super) kind: ValueKind,
    pub(super) span: Span,
}

#[derive(Clone)]
pub(super) enum ValueKind {
    Scalar(String),
    Array(Vec<Value>),
    Object(Vec<Field>),
}

pub(super) struct ParseError {
    pub(super) span: Span,
    pub(super) message: String,
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    cursor: usize,
}

pub(super) fn parse(source: &str) -> Result<Vec<Field>, ParseError> {
    let tokens = lex(source).map_err(|error| ParseError {
        span: error.span,
        message: error.message.to_owned(),
    })?;
    let Some(open) = tokens.windows(2).position(|pair| {
        matches!(&pair[0].kind, TokenKind::Atom(atom) if atom == "record")
            && matches!(pair[1].kind, TokenKind::Punctuation('{'))
    }) else {
        return Err(ParseError {
            span: Span::new(0, source.len()),
            message: "missing record body".to_owned(),
        });
    };
    let mut parser = Parser {
        source,
        tokens,
        cursor: open + 2,
    };
    let fields = parser.fields()?;
    parser.skip_newlines();
    if !matches!(parser.peek().kind, TokenKind::End) {
        return Err(parser.error("unexpected tokens after record body"));
    }
    Ok(fields)
}

impl Parser<'_> {
    fn fields(&mut self) -> Result<Vec<Field>, ParseError> {
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
            fields.push(Field {
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

    fn value(&mut self) -> Result<Value, ParseError> {
        match self.peek().kind {
            TokenKind::Punctuation('[') => self.array(),
            TokenKind::Punctuation('{') => self.object(),
            _ => self.scalar(),
        }
    }

    fn array(&mut self) -> Result<Value, ParseError> {
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
                return Ok(Value {
                    kind: ValueKind::Array(values),
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

    fn object(&mut self) -> Result<Value, ParseError> {
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
                return Ok(Value {
                    kind: ValueKind::Object(fields),
                    span: Span::new(start, end),
                });
            }
            let field_start = self.peek().span.start;
            let name = self.atom()?.to_owned();
            self.expect(':')?;
            let value = self.value()?;
            fields.push(Field {
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

    fn scalar(&mut self) -> Result<Value, ParseError> {
        let first = self.peek().span;
        if matches!(self.peek().kind, TokenKind::String(_)) {
            self.cursor += 1;
            return Ok(Value {
                kind: ValueKind::Scalar(self.text(first).to_owned()),
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
        Ok(Value {
            kind: ValueKind::Scalar(self.source[first.start..last.end].trim().to_owned()),
            span: Span::new(first.start, last.end),
        })
    }

    fn atom(&mut self) -> Result<&str, ParseError> {
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

    fn expect(&mut self, punctuation: char) -> Result<(), ParseError> {
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

    fn error(&self, message: &str) -> ParseError {
        ParseError {
            span: self.peek().span,
            message: message.to_owned(),
        }
    }
}
