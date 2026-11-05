use tir_diag::Span;
use tir_format::text::{Token, TokenKind, decode_kira_string_literal_contents, lex};
use tir_format::{TypeParseError, parse_tvisa_type};
use tir_types::Type;

use crate::gpu_entries::GpuEntryKind;
use crate::text::{TextParseError, error};
use crate::text_entry_validation::validate_record_fields;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextGpuEntry {
    pub function: String,
    pub kind: GpuEntryKind,
    pub fields: Vec<TextEntryField>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextEntryField {
    pub name: String,
    pub value: TextEntryValue,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextEntryValue {
    pub kind: TextEntryValueKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextEntryValueKind {
    Atom(String),
    String(String),
    Type(Type),
    Call(String, Vec<TextEntryValue>),
    Array(Vec<TextEntryValue>),
    Object(Vec<TextEntryField>),
}

pub(crate) fn parse_gpu_entry_text(
    source: &str,
    source_offset: usize,
) -> Result<TextGpuEntry, TextParseError> {
    let tokens = lex(source)
        .map_err(|failure| error(shift_span(failure.span, source_offset), failure.message))?;
    let mut parser = EntryParser {
        source,
        tokens: &tokens,
        index: 0,
        source_offset,
    };
    parser.expect_atom("entry")?;
    let kind_span = parser.peek().span;
    let kind = match parser.atom("expected GPU entry kind")?.as_str() {
        "compute" => GpuEntryKind::Compute,
        "object" => GpuEntryKind::Object,
        "mesh" => GpuEntryKind::Mesh,
        "vertex" => GpuEntryKind::Vertex,
        "fragment" => GpuEntryKind::Fragment,
        "tile" => GpuEntryKind::Tile,
        "raygen" => GpuEntryKind::Raygen,
        "intersection" => GpuEntryKind::Intersection,
        "anyhit" => GpuEntryKind::Anyhit,
        "closesthit" => GpuEntryKind::Closesthit,
        "miss" => GpuEntryKind::Miss,
        "callable" => GpuEntryKind::Callable,
        "node" => GpuEntryKind::Node,
        "visible" => GpuEntryKind::Visible,
        _ => {
            return Err(error(
                shift_span(kind_span, source_offset),
                "unknown GPU entry kind",
            ));
        }
    };
    let function = parser.symbol()?;
    parser.expect_atom("record")?;
    parser.expect_punctuation('{')?;
    let mut fields = Vec::new();
    parser.skip_newlines();
    while !parser.eat_punctuation('}') {
        if parser.at_end() {
            return Err(parser.failure("unterminated GPU entry record"));
        }
        let field = parser.field(true, false)?;
        fields.push(field);
        if !parser.eat_newline() {
            return Err(parser.failure("GPU entry fields must each occupy one line"));
        }
        parser.skip_newlines();
    }
    parser.skip_newlines();
    if !parser.at_end() {
        return Err(parser.failure("unexpected token after GPU entry record"));
    }
    let span = Span::new(source_offset, source_offset + source.len());
    validate_record_fields(kind, &fields, span)?;
    Ok(TextGpuEntry {
        function,
        kind,
        fields,
        span,
    })
}

struct EntryParser<'a> {
    source: &'a str,
    tokens: &'a [Token],
    index: usize,
    source_offset: usize,
}

impl EntryParser<'_> {
    fn symbol(&mut self) -> Result<String, TextParseError> {
        let start = self.peek().span;
        self.expect_punctuation('@')?;
        let name_start = self.index;
        while !self.at_end() {
            if self.is_atom("record") && self.index > name_start {
                let previous_end = self.tokens[self.index - 1].span.end;
                let record_start = self.peek().span.start;
                if self.source[previous_end..record_start]
                    .chars()
                    .any(char::is_whitespace)
                {
                    break;
                }
            }
            if matches!(self.peek().kind, TokenKind::Newline) {
                return Err(self.failure("GPU entry symbol and record keyword must share a line"));
            }
            self.index += 1;
        }
        if name_start == self.index {
            return Err(self.failure("expected GPU entry function symbol"));
        }
        let last = &self.tokens[self.index - 1];
        let symbol = self.source[start.start..last.span.end].to_owned();
        if symbol.bytes().any(|byte| byte.is_ascii_whitespace()) || symbol.len() == 1 {
            return Err(error(
                self.absolute(start),
                "invalid GPU entry function symbol",
            ));
        }
        Ok(symbol[1..].to_owned())
    }

    fn field(
        &mut self,
        parse_type: bool,
        allow_newline_after_colon: bool,
    ) -> Result<TextEntryField, TextParseError> {
        let start = self.peek().span;
        let name = self.atom("expected GPU entry field name")?;
        self.expect_punctuation(':')?;
        if !allow_newline_after_colon && matches!(self.peek().kind, TokenKind::Newline) {
            return Err(self.failure("GPU entry field value must be on the same line"));
        }
        let value = self.value(parse_type.then_some(name.as_str()))?;
        Ok(TextEntryField {
            name,
            span: Span::new(self.absolute(start).start, value.span.end),
            value,
        })
    }

    fn value(&mut self, field_name: Option<&str>) -> Result<TextEntryValue, TextParseError> {
        self.skip_newlines();
        if field_name == Some("type") {
            return self.type_value();
        }
        if field_name == Some("default") && !matches!(self.peek().kind, TokenKind::Punctuation('['))
        {
            return self.default_literal();
        }
        let start = self.peek().span;
        let kind = match self.peek().kind.clone() {
            TokenKind::String(raw) => {
                self.index += 1;
                let value = decode_kira_string_literal_contents(&raw)
                    .map_err(|failure| error(self.absolute(start), &failure.to_string()))?;
                TextEntryValueKind::String(value)
            }
            TokenKind::Atom(atom) => {
                self.index += 1;
                if self.eat_punctuation('(') {
                    let arguments = self.value_list(')', None)?;
                    TextEntryValueKind::Call(atom, arguments)
                } else {
                    TextEntryValueKind::Atom(atom)
                }
            }
            TokenKind::Punctuation('[') => {
                self.index += 1;
                TextEntryValueKind::Array(
                    self.value_list(']', (field_name == Some("default")).then_some("default"))?,
                )
            }
            TokenKind::Punctuation('{') => {
                self.index += 1;
                TextEntryValueKind::Object(self.object_fields()?)
            }
            _ => return Err(self.failure("expected GPU entry value")),
        };
        let end = self.tokens[self.index.saturating_sub(1)].span;
        Ok(TextEntryValue {
            kind,
            span: Span::new(self.absolute(start).start, self.absolute(end).end),
        })
    }

    fn default_literal(&mut self) -> Result<TextEntryValue, TextParseError> {
        let start = self.peek().span;
        let mut parentheses = 0_i32;
        while !self.at_end() {
            match self.peek().kind {
                TokenKind::Punctuation('(') => parentheses += 1,
                TokenKind::Punctuation(')') if parentheses > 0 => parentheses -= 1,
                TokenKind::Punctuation(')')
                | TokenKind::Punctuation(',')
                | TokenKind::Punctuation(']')
                | TokenKind::Punctuation('}')
                | TokenKind::Newline
                    if parentheses == 0 =>
                {
                    break;
                }
                _ => {}
            }
            self.index += 1;
        }
        if start.start == self.peek().span.start {
            return Err(self.failure("expected specialization default literal"));
        }
        let end = self.tokens[self.index - 1].span.end;
        let literal = self
            .source
            .get(start.start..end)
            .ok_or_else(|| self.failure("specialization default has an invalid source span"))?
            .trim()
            .to_owned();
        Ok(TextEntryValue {
            kind: TextEntryValueKind::Atom(literal),
            span: Span::new(self.absolute(start).start, self.source_offset + end),
        })
    }

    fn type_value(&mut self) -> Result<TextEntryValue, TextParseError> {
        let start_index = self.index;
        let mut angle_depth = 0_i32;
        while !self.at_end() {
            match self.peek().kind {
                TokenKind::Punctuation('<') => angle_depth += 1,
                TokenKind::Punctuation('>') => angle_depth -= 1,
                TokenKind::Punctuation(',')
                | TokenKind::Punctuation('}')
                | TokenKind::Punctuation(']')
                    if angle_depth == 0 =>
                {
                    break;
                }
                TokenKind::Newline if angle_depth == 0 => break,
                _ => {}
            }
            self.index += 1;
        }
        if start_index == self.index || angle_depth != 0 {
            return Err(self.failure("invalid TVISA type in GPU entry field"));
        }
        let local_span = Span::new(
            self.tokens[start_index].span.start,
            self.tokens[self.index - 1].span.end,
        );
        let absolute = self.absolute(local_span);
        let ty = parse_tvisa_type(&self.source[local_span.start..local_span.end]).map_err(
            |failure: TypeParseError| {
                error(
                    Span::new(
                        absolute.start + failure.span.start,
                        absolute.start + failure.span.end,
                    ),
                    &failure.message,
                )
            },
        )?;
        Ok(TextEntryValue {
            kind: TextEntryValueKind::Type(ty),
            span: absolute,
        })
    }

    fn value_list(
        &mut self,
        close: char,
        value_context: Option<&str>,
    ) -> Result<Vec<TextEntryValue>, TextParseError> {
        let mut values = Vec::new();
        self.skip_newlines();
        if self.eat_punctuation(close) {
            return Ok(values);
        }
        loop {
            values.push(self.value(value_context)?);
            self.skip_newlines();
            if self.eat_punctuation(close) {
                return Ok(values);
            }
            self.expect_punctuation(',')?;
            self.skip_newlines();
        }
    }

    fn object_fields(&mut self) -> Result<Vec<TextEntryField>, TextParseError> {
        let mut fields = Vec::new();
        self.skip_newlines();
        if self.eat_punctuation('}') {
            return Ok(fields);
        }
        loop {
            fields.push(self.field(true, true)?);
            self.skip_newlines();
            if self.eat_punctuation('}') {
                return Ok(fields);
            }
            self.expect_punctuation(',')?;
            self.skip_newlines();
        }
    }

    fn expect_atom(&mut self, expected: &str) -> Result<(), TextParseError> {
        if self.atom("expected identifier")? == expected {
            Ok(())
        } else {
            Err(self.failure("unexpected identifier in GPU entry"))
        }
    }

    fn atom(&mut self, message: &str) -> Result<String, TextParseError> {
        let span = self.peek().span;
        if let TokenKind::Atom(value) = self.peek().kind.clone() {
            self.index += 1;
            Ok(value)
        } else {
            Err(error(self.absolute(span), message))
        }
    }

    fn expect_punctuation(&mut self, expected: char) -> Result<(), TextParseError> {
        if self.eat_punctuation(expected) {
            Ok(())
        } else {
            Err(self.failure("unexpected punctuation in GPU entry"))
        }
    }

    fn eat_punctuation(&mut self, expected: char) -> bool {
        if matches!(self.tokens.get(self.index).map(|token| &token.kind), Some(TokenKind::Punctuation(value)) if *value == expected)
        {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn eat_newline(&mut self) -> bool {
        if matches!(
            self.tokens.get(self.index).map(|token| &token.kind),
            Some(TokenKind::Newline)
        ) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn skip_newlines(&mut self) {
        while self.eat_newline() {}
    }

    fn is_atom(&self, value: &str) -> bool {
        matches!(self.tokens.get(self.index).map(|token| &token.kind), Some(TokenKind::Atom(atom)) if atom == value)
    }

    fn at_end(&self) -> bool {
        matches!(
            self.tokens.get(self.index).map(|token| &token.kind),
            Some(TokenKind::End) | None
        )
    }

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.index)
            .or_else(|| self.tokens.last())
            .expect("lexer always appends an end token")
    }

    fn absolute(&self, span: Span) -> Span {
        shift_span(span, self.source_offset)
    }

    fn failure(&self, message: &str) -> TextParseError {
        error(self.absolute(self.peek().span), message)
    }
}

fn shift_span(span: Span, offset: usize) -> Span {
    Span::new(
        span.start.saturating_add(offset),
        span.end.saturating_add(offset),
    )
}
