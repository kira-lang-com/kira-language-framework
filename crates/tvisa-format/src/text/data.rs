use tir_diag::Span;
use tir_format::text::{Token, TokenKind, decode_kira_string_literal_contents, lex};

use crate::{DataKind, DataLinkage, DataRelocationKind};

use super::{TextItem, TextItemKind, TextParseError, error};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextData {
    pub symbol: String,
    pub symbol_span: Span,
    pub linkage: DataLinkage,
    pub kind: DataKind,
    pub per_cpu: bool,
    pub alignment: u64,
    pub initializer: TextDataInitializer,
    pub relocations: Vec<TextDataRelocation>,
    pub span: Span,
}

impl TextData {
    #[must_use]
    pub fn size(&self) -> u64 {
        match &self.initializer {
            TextDataInitializer::Bytes { bytes, .. } => bytes.len() as u64,
            TextDataInitializer::Zero { size, .. }
            | TextDataInitializer::Aggregate { size, .. } => *size,
        }
    }

    #[must_use]
    pub const fn initializer_span(&self) -> Span {
        match &self.initializer {
            TextDataInitializer::Bytes { span, .. }
            | TextDataInitializer::Zero { span, .. }
            | TextDataInitializer::Aggregate { span, .. } => *span,
        }
    }

    #[must_use]
    pub const fn is_zero_initialized(&self) -> bool {
        matches!(&self.initializer, TextDataInitializer::Zero { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextDataInitializer {
    Bytes {
        bytes: Vec<u8>,
        span: Span,
    },
    Zero {
        size: u64,
        span: Span,
    },
    Aggregate {
        size: u64,
        elements: Vec<TextDataElement>,
        span: Span,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextDataElement {
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextDataRelocation {
    pub offset: u64,
    pub kind: DataRelocationKind,
    pub symbol: String,
    pub symbol_span: Span,
    pub addend: i64,
    pub tag: Option<u32>,
    pub span: Span,
}

pub(super) fn parse(source: &str, items: &[TextItem]) -> Result<Vec<TextData>, TextParseError> {
    let mut objects = Vec::new();
    for item in items.iter().filter(|item| item.kind == TextItemKind::Data) {
        let text = source
            .get(item.span.start..item.span.end)
            .unwrap_or_default();
        let tokens = lex(text)
            .map_err(|cause| error(shift(cause.span, item.span.start), &cause.message))?
            .into_iter()
            .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
            .collect();
        objects.push(Parser::new(tokens, item.span.start, item.span).parse()?);
    }
    Ok(objects)
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    base: usize,
    item_span: Span,
}

impl Parser {
    fn new(tokens: Vec<Token>, base: usize, item_span: Span) -> Self {
        Self {
            tokens,
            index: 0,
            base,
            item_span,
        }
    }

    fn parse(mut self) -> Result<TextData, TextParseError> {
        self.expect_atom("data")?;
        let (symbol, symbol_span) = self.symbol()?;
        let linkage = match self.peek_atom() {
            Some("local") => {
                self.index += 1;
                DataLinkage::Local
            }
            Some("image") => {
                self.index += 1;
                DataLinkage::Image
            }
            Some("exported") => {
                self.index += 1;
                DataLinkage::Exported
            }
            _ => DataLinkage::Local,
        };
        let kind = match self.atom()?.as_str() {
            "readonly" => DataKind::Readonly,
            "relro" => DataKind::Relro,
            "writable" => DataKind::Writable,
            "zero" => DataKind::Zero,
            "tls" => DataKind::Tls,
            "tls_zero" => DataKind::TlsZero,
            "gpu_constant" => DataKind::GpuConstant,
            _ => return Err(self.error_at(self.previous_span(), "unknown TVISA data kind")),
        };
        let per_cpu = self.eat_atom("percpu");
        self.expect_atom("align")?;
        let alignment = self.unsigned("data alignment must be an unsigned integer")?;
        let initializer = self.initializer()?;
        let mut relocations = Vec::new();
        while !self.finished() {
            relocations.push(self.relocation()?);
        }
        Ok(TextData {
            symbol,
            symbol_span,
            linkage,
            kind,
            per_cpu,
            alignment,
            initializer,
            relocations,
            span: self.item_span,
        })
    }

    fn initializer(&mut self) -> Result<TextDataInitializer, TextParseError> {
        if self.eat_atom("bytes") {
            let start = self.previous_span().start;
            let token = self.next()?;
            let TokenKind::String(raw) = token.kind else {
                return Err(self.error_at(token.span, "bytes initializer requires a string"));
            };
            let contents = decode_kira_string_literal_contents(&raw).map_err(|cause| {
                self.error_at(token.span, &format!("invalid string escape: {cause}"))
            })?;
            return Ok(TextDataInitializer::Bytes {
                bytes: contents.into_bytes(),
                span: self.absolute(Span::new(start, token.span.end)),
            });
        }
        if self.eat_atom("zero") {
            let start = self.previous_span().start;
            let size = self.unsigned("zero initializer requires a byte count")?;
            return Ok(TextDataInitializer::Zero {
                size,
                span: self.absolute(Span::new(start, self.previous_span().end)),
            });
        }
        if self.eat_punctuation('{') {
            let start = self.previous_span().start;
            self.expect_atom("size")?;
            let size = self.unsigned("aggregate size must be an unsigned integer")?;
            let mut elements = Vec::new();
            while !self.eat_punctuation('}') {
                if self.finished() {
                    return Err(error(self.item_span, "unterminated data initializer"));
                }
                let element_start = self.peek_span();
                let offset = self.unsigned("aggregate element requires a byte offset")?;
                self.expect_punctuation(':')?;
                self.expect_atom("hex")?;
                let token = self.next()?;
                let TokenKind::String(hex) = token.kind else {
                    return Err(
                        self.error_at(token.span, "aggregate element requires a hex string")
                    );
                };
                let bytes = decode_hex(&hex)
                    .ok_or_else(|| self.error_at(token.span, "invalid aggregate hex bytes"))?;
                elements.push(TextDataElement {
                    offset,
                    bytes,
                    span: self.absolute(Span::new(element_start.start, token.span.end)),
                });
            }
            return Ok(TextDataInitializer::Aggregate {
                size,
                elements,
                span: self.absolute(Span::new(start, self.previous_span().end)),
            });
        }
        Err(self.error("data initializer must be bytes, zero, or an aggregate"))
    }

    fn relocation(&mut self) -> Result<TextDataRelocation, TextParseError> {
        let start = self.peek_span();
        self.expect_atom("reloc")?;
        let offset = self.unsigned("relocation offset must be an unsigned integer")?;
        let kind_span = self.peek_span();
        let kind = match self.atom()?.as_str() {
            "ABS64" => DataRelocationKind::Abs64,
            "CODE64" => DataRelocationKind::Code64,
            "REL32" => DataRelocationKind::Rel32,
            "GPUENTRY64" => DataRelocationKind::GpuEntry64,
            "TPOFF64" => DataRelocationKind::Tpoff64,
            "IMPORT64" => DataRelocationKind::Import64,
            "IMPORTCODE64" => DataRelocationKind::ImportCode64,
            "DISPATCH64" => DataRelocationKind::Dispatch64,
            _ => return Err(self.error_at(kind_span, "unknown relocation kind")),
        };
        let (symbol, symbol_span) = self.symbol()?;
        let mut addend = 0;
        if self.eat_punctuation('+') || self.eat_punctuation('-') {
            let negative = matches!(
                self.tokens[self.index - 1].kind,
                TokenKind::Punctuation('-')
            );
            let magnitude_span = self.peek_span();
            let magnitude = self.unsigned("relocation addend must be a signed integer")?;
            addend = signed_addend(magnitude, negative).ok_or_else(|| {
                self.error_at(
                    magnitude_span,
                    "relocation addend is outside the signed 64-bit range",
                )
            })?;
        }
        let tag = if self.eat_atom("tag") {
            let span = self.peek_span();
            let value = self.atom()?;
            Some(parse_hex_u32(&value).ok_or_else(|| {
                self.error_at(span, "relocation tag must be a 32-bit hexadecimal value")
            })?)
        } else {
            None
        };
        Ok(TextDataRelocation {
            offset,
            kind,
            symbol,
            symbol_span,
            addend,
            tag,
            span: self.absolute(Span::new(start.start, self.previous_span().end)),
        })
    }

    fn symbol(&mut self) -> Result<(String, Span), TextParseError> {
        let start = self.next()?;
        if !matches!(start.kind, TokenKind::Punctuation('@')) {
            return Err(self.error_at(start.span, "expected TVISA symbol"));
        }
        let first = self.next()?;
        let TokenKind::Atom(first_name) = first.kind else {
            return Err(self.error_at(first.span, "expected TVISA symbol name"));
        };
        let mut name = first_name;
        let mut end = first.span.end;
        while self.eat_punctuation('.') {
            let component = self.next()?;
            let TokenKind::Atom(component_name) = component.kind else {
                return Err(self.error_at(component.span, "expected qualified symbol component"));
            };
            name.push('.');
            name.push_str(&component_name);
            end = component.span.end;
        }
        Ok((name, self.absolute(Span::new(start.span.start, end))))
    }

    fn expect_atom(&mut self, expected: &str) -> Result<(), TextParseError> {
        let span = self.peek_span();
        if self.atom()? == expected {
            Ok(())
        } else {
            Err(self.error_at(span, &format!("expected {expected}")))
        }
    }

    fn expect_punctuation(&mut self, expected: char) -> Result<(), TextParseError> {
        let token = self.next()?;
        if matches!(token.kind, TokenKind::Punctuation(value) if value == expected) {
            Ok(())
        } else {
            Err(self.error_at(token.span, &format!("expected '{expected}'")))
        }
    }

    fn atom(&mut self) -> Result<String, TextParseError> {
        let token = self.next()?;
        if let TokenKind::Atom(value) = token.kind {
            Ok(value)
        } else {
            Err(self.error_at(token.span, "expected TVISA data identifier"))
        }
    }

    fn unsigned(&mut self, message: &str) -> Result<u64, TextParseError> {
        let token = self.next()?;
        let value = match &token.kind {
            TokenKind::Atom(value) => tir_format::numeric_literals::parse_unsigned_integer(value),
            _ => None,
        }
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| self.error_at(token.span, message))?;
        Ok(value)
    }

    fn next(&mut self) -> Result<Token, TextParseError> {
        let token = self
            .tokens
            .get(self.index)
            .cloned()
            .ok_or_else(|| self.error("unexpected end of TVISA data item"))?;
        self.index += 1;
        Ok(token)
    }

    fn eat_atom(&mut self, expected: &str) -> bool {
        if self.peek_atom() == Some(expected) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn eat_punctuation(&mut self, expected: char) -> bool {
        if self.tokens.get(self.index).is_some_and(
            |token| matches!(token.kind, TokenKind::Punctuation(value) if value == expected),
        ) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn peek_atom(&self) -> Option<&str> {
        self.tokens
            .get(self.index)
            .and_then(|token| match &token.kind {
                TokenKind::Atom(value) => Some(value.as_str()),
                _ => None,
            })
    }

    fn finished(&self) -> bool {
        self.index == self.tokens.len()
    }

    fn previous_span(&self) -> Span {
        self.tokens
            .get(self.index.saturating_sub(1))
            .map_or(Span::new(0, 0), |token| token.span)
    }

    fn peek_span(&self) -> Span {
        self.tokens
            .get(self.index)
            .map_or(Span::new(0, 0), |token| token.span)
    }

    fn absolute(&self, span: Span) -> Span {
        shift(span, self.base)
    }

    fn error(&self, message: &str) -> TextParseError {
        self.error_at(self.peek_span(), message)
    }

    fn error_at(&self, span: Span, message: &str) -> TextParseError {
        error(self.absolute(span), message)
    }
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

fn parse_hex_u32(value: &str) -> Option<u32> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))?;
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten()
}

fn signed_addend(magnitude: u64, negative: bool) -> Option<i64> {
    if negative && magnitude == (1_u64 << 63) {
        return Some(i64::MIN);
    }
    let value = i64::try_from(magnitude).ok()?;
    Some(if negative { -value } else { value })
}

fn shift(span: Span, base: usize) -> Span {
    Span::new(
        span.start.saturating_add(base),
        span.end.saturating_add(base),
    )
}
