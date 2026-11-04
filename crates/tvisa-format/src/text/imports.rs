use tir_diag::Span;
use tir_format::text::{Token, TokenKind, lex};

use crate::imports::ImportModuleKind;

use super::{TextItem, TextItemKind, TextParseError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextImport {
    pub symbol: String,
    pub symbol_span: Span,
    pub module: TextImportModule,
    pub payload: TextImportPayload,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextImportModule {
    pub kind: ImportModuleKind,
    pub name: String,
    pub abi_uuid: [u8; 16],
    pub weak: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextImportPayload {
    Function {
        signature: String,
        signature_span: Span,
        tag: u32,
    },
    Data {
        size: u64,
        alignment: u64,
    },
}

pub(super) fn parse(source: &str, items: &[TextItem]) -> Result<Vec<TextImport>, TextParseError> {
    let mut imports = Vec::new();
    for item in items
        .iter()
        .filter(|item| item.kind == TextItemKind::Import)
    {
        let text = source
            .get(item.span.start..item.span.end)
            .unwrap_or_default();
        let tokens = lex(text)
            .map_err(|error| TextParseError {
                span: shift(error.span, item.span.start),
                message: error.message.to_owned(),
            })?
            .into_iter()
            .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
            .collect::<Vec<_>>();
        let mut parser = Parser {
            tokens,
            index: 0,
            base: item.span.start,
        };
        parser.expect_atom("import")?;
        let kind = parser.atom()?;
        let (symbol, symbol_span) = parser.symbol()?;
        let module = parser.module()?;
        let payload = match kind.as_str() {
            "function" => {
                parser.expect_atom("sig")?;
                parser.expect_punctuation('$')?;
                let (signature, signature_span) = parser.ident()?;
                parser.expect_atom("tag")?;
                let tag_span = parser.peek_span();
                let tag = parse_hex_u32(&parser.atom()?).ok_or_else(|| {
                    parser.error_at(tag_span, "TVISA import tag must be 32-bit hexadecimal")
                })?;
                TextImportPayload::Function {
                    signature,
                    signature_span,
                    tag,
                }
            }
            "data" => {
                let size = parser.integer("invalid TVISA imported-data size")?;
                let alignment = parser.integer("invalid TVISA imported-data alignment")?;
                if alignment == 0 || !alignment.is_power_of_two() {
                    return Err(parser.error_at(
                        parser.previous_span(),
                        "TVISA imported-data alignment must be a positive power of two",
                    ));
                }
                TextImportPayload::Data { size, alignment }
            }
            _ => return Err(parser.error("expected function or data import")),
        };
        if !parser.is_finished() {
            return Err(parser.error("unexpected token after TVISA import"));
        }
        imports.push(TextImport {
            symbol,
            symbol_span,
            module,
            payload,
            span: item.span,
        });
    }
    Ok(imports)
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    base: usize,
}

impl Parser {
    fn expect_atom(&mut self, expected: &str) -> Result<(), TextParseError> {
        let span = self.peek_span();
        if self.atom()? == expected {
            Ok(())
        } else {
            Err(self.error_at(span, "unexpected TVISA import identifier"))
        }
    }

    fn atom(&mut self) -> Result<String, TextParseError> {
        let token = self.next()?;
        match token.kind {
            TokenKind::Atom(value) => Ok(value),
            _ => Err(self.error_at(token.span, "expected TVISA import identifier")),
        }
    }

    fn ident(&mut self) -> Result<(String, Span), TextParseError> {
        let token = self.next()?;
        match token.kind {
            TokenKind::Atom(value) => Ok((value, shift(token.span, self.base))),
            _ => Err(self.error_at(token.span, "expected TVISA import identifier")),
        }
    }

    fn string(&mut self) -> Result<String, TextParseError> {
        let token = self.next()?;
        match token.kind {
            TokenKind::String(value) => Ok(value),
            _ => Err(self.error_at(token.span, "expected TVISA import string")),
        }
    }

    fn module(&mut self) -> Result<TextImportModule, TextParseError> {
        let kind_span = self.peek_span();
        let kind = match self.atom()?.as_str() {
            "ridl" => ImportModuleKind::Ridl,
            "package" => ImportModuleKind::Package,
            "platform" => ImportModuleKind::Platform,
            _ => {
                return Err(self.error_at(
                    kind_span,
                    "expected ridl, package, or platform TVISA import module",
                ));
            }
        };
        let name = self.string()?;
        if name.is_empty() {
            return Err(self.error_at(
                self.previous_span(),
                "TVISA import module name must be nonempty",
            ));
        }
        let weak = if self.eat_atom("weak") {
            if kind != ImportModuleKind::Platform {
                return Err(self.error_at(
                    self.previous_span(),
                    "only platform TVISA imports may be weak",
                ));
            }
            true
        } else {
            false
        };
        self.expect_atom("abi")?;
        let abi_span = self.peek_span();
        let abi_text = self.string()?;
        let abi_uuid = parse_uuid(&abi_text).ok_or_else(|| {
            self.error_at(abi_span, "TVISA import ABI must be a canonical UUID string")
        })?;
        let is_zero = abi_uuid.iter().all(|byte| *byte == 0);
        if (kind == ImportModuleKind::Ridl) == is_zero {
            return Err(self.error_at(
                abi_span,
                "TVISA import ABI UUID must be nonzero for RIDL and zero otherwise",
            ));
        }
        Ok(TextImportModule {
            kind,
            name,
            abi_uuid,
            weak,
        })
    }

    fn symbol(&mut self) -> Result<(String, Span), TextParseError> {
        let start = self.next()?;
        if !matches!(start.kind, TokenKind::Punctuation('@')) {
            return Err(self.error_at(start.span, "expected TVISA import symbol"));
        }
        let first = self.next()?;
        let TokenKind::Atom(first_name) = first.kind else {
            return Err(self.error_at(first.span, "expected TVISA import symbol name"));
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
        Ok((name, shift(Span::new(start.span.start, end), self.base)))
    }

    fn integer(&mut self, message: &str) -> Result<u64, TextParseError> {
        let span = self.peek_span();
        let atom = self.atom()?;
        parse_integer(&atom).ok_or_else(|| self.error_at(span, message))
    }

    fn expect_punctuation(&mut self, expected: char) -> Result<(), TextParseError> {
        let span = self.peek_span();
        if self.eat_punctuation(expected) {
            Ok(())
        } else {
            Err(self.error_at(span, "unexpected TVISA import punctuation"))
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

    fn eat_atom(&mut self, expected: &str) -> bool {
        if matches!(self.tokens.get(self.index).map(|token| &token.kind), Some(TokenKind::Atom(value)) if value == expected)
        {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn next(&mut self) -> Result<Token, TextParseError> {
        let token = self
            .tokens
            .get(self.index)
            .cloned()
            .ok_or_else(|| self.error_at(self.peek_span(), "unexpected end of TVISA import"))?;
        self.index += 1;
        Ok(token)
    }

    fn peek_span(&self) -> Span {
        self.tokens.get(self.index).map_or_else(
            || {
                self.tokens
                    .last()
                    .map_or(Span::new(0, 0), |token| token.span)
            },
            |token| token.span,
        )
    }

    fn previous_span(&self) -> Span {
        self.tokens
            .get(self.index.saturating_sub(1))
            .map_or(Span::new(0, 0), |token| token.span)
    }

    fn error(&self, message: &str) -> TextParseError {
        self.error_at(self.peek_span(), message)
    }

    fn error_at(&self, span: Span, message: &str) -> TextParseError {
        TextParseError {
            span: shift(span, self.base),
            message: message.to_owned(),
        }
    }

    fn is_finished(&self) -> bool {
        self.index == self.tokens.len()
    }
}

fn parse_integer(value: &str) -> Option<u64> {
    let clean = value.replace('_', "");
    if let Some(hex) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok()
    } else if let Some(binary) = clean
        .strip_prefix("0b")
        .or_else(|| clean.strip_prefix("0B"))
    {
        u64::from_str_radix(binary, 2).ok()
    } else if let Some(octal) = clean
        .strip_prefix("0o")
        .or_else(|| clean.strip_prefix("0O"))
    {
        u64::from_str_radix(octal, 8).ok()
    } else {
        clean.parse().ok()
    }
}

fn parse_hex_u32(value: &str) -> Option<u32> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))?;
    u32::from_str_radix(&value.replace('_', ""), 16).ok()
}

fn parse_uuid(value: &str) -> Option<[u8; 16]> {
    if value.len() != 36
        || ![8, 13, 18, 23]
            .into_iter()
            .all(|index| value.as_bytes().get(index) == Some(&b'-'))
    {
        return None;
    }
    let digits = value
        .bytes()
        .filter(|byte| *byte != b'-')
        .collect::<Vec<_>>();
    if digits
        .iter()
        .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(byte))
    {
        return None;
    }
    let mut uuid = [0; 16];
    for (index, pair) in digits.chunks_exact(2).enumerate() {
        let high = (pair[0] as char).to_digit(16)? as u8;
        let low = (pair[1] as char).to_digit(16)? as u8;
        uuid[index] = (high << 4) | low;
    }
    Some(uuid)
}

fn shift(span: Span, base: usize) -> Span {
    Span::new(
        span.start.saturating_add(base),
        span.end.saturating_add(base),
    )
}
