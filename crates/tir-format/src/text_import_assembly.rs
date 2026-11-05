use crate::text::{Token, TokenKind, decode_kira_string_literal_contents, lex};
use crate::{
    BinaryImport, BinaryImportEntityKind, BinaryImportKind, BinaryImportedEntity, TextItem,
    TextParseError, parse_type,
};
use tir_diag::Span;
use tir_types::Type;

pub(crate) struct ParsedImport {
    pub(crate) record: BinaryImport,
    pub(crate) entity_types: Vec<Type>,
    pub(crate) item: TextItem,
}

pub(crate) fn parse_import(source: &str, item: TextItem) -> Result<ParsedImport, TextParseError> {
    let base = item.span;
    let tokens = lex(source).map_err(|error| text_error(base, error.span, error.message))?;
    let tokens = tokens
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::End))
        .collect::<Vec<_>>();
    let mut cursor = ImportCursor {
        tokens: &tokens,
        index: 0,
        base,
    };
    cursor.expect_atom("import")?;
    let (kind, name, abi_uuid, weak) = match cursor.atom_value()?.as_str() {
        "ridl" => {
            cursor.expect_punctuation('@')?;
            let name = cursor.path()?;
            cursor.expect_atom("abi")?;
            let abi = cursor.string_value()?;
            (BinaryImportKind::Ridl, name, parse_uuid(&abi, base)?, false)
        }
        "package" => {
            cursor.expect_punctuation('@')?;
            (BinaryImportKind::Package, cursor.path()?, [0; 16], false)
        }
        "platform" => {
            let name = cursor.string_value()?;
            let weak = cursor.eat_atom("weak");
            (BinaryImportKind::Platform, name, [0; 16], weak)
        }
        _ => return Err(cursor.error("unknown import kind")),
    };
    let mut entities = Vec::new();
    let mut entity_types = Vec::new();
    cursor.skip_newlines();
    if cursor.eat_punctuation('{') {
        loop {
            cursor.skip_newlines();
            if cursor.eat_punctuation('}') {
                break;
            }
            let entity_word = cursor.atom_value()?;
            let (entity_kind, has_platform_symbol) = match (kind, entity_word.as_str()) {
                (BinaryImportKind::Platform, "func") => (BinaryImportEntityKind::Function, true),
                (BinaryImportKind::Platform, "data") => {
                    (BinaryImportEntityKind::PlatformData, true)
                }
                (BinaryImportKind::Platform, _) => {
                    return Err(cursor.error_at(
                        cursor.tokens[cursor.index - 1].span,
                        "platform imports support only func and data entities",
                    ));
                }
                (_, "func") => (BinaryImportEntityKind::Function, false),
                (_, "global") => (BinaryImportEntityKind::Global, false),
                (_, "type") => (BinaryImportEntityKind::Type, false),
                (_, "witness") => (BinaryImportEntityKind::Witness, false),
                (_, "descriptor") => (BinaryImportEntityKind::Descriptor, false),
                _ => return Err(cursor.error("unknown imported entity kind")),
            };
            cursor.expect_punctuation('@')?;
            let entity_name = cursor.path()?;
            let platform_symbol = if has_platform_symbol {
                cursor.expect_atom("symbol")?;
                Some(cursor.string_value()?)
            } else {
                None
            };
            cursor.expect_punctuation(':')?;
            let ty = cursor.type_value(source)?;
            entities.push(BinaryImportedEntity {
                kind: entity_kind,
                name: entity_name,
                platform_symbol,
                type_index: 0,
            });
            entity_types.push(ty);
            if cursor.eat_punctuation(',') {
                cursor.skip_newlines();
            } else {
                cursor.skip_newlines();
            }
        }
    }
    cursor.skip_newlines();
    if !cursor.is_done() {
        return Err(cursor.error("unexpected token in import declaration"));
    }
    Ok(ParsedImport {
        record: BinaryImport {
            kind,
            name,
            abi_uuid,
            weak,
            entities,
        },
        entity_types,
        item,
    })
}

fn parse_uuid(value: &str, base: Span) -> Result<[u8; 16], TextParseError> {
    let bytes = value.as_bytes();
    if bytes.len() != 36
        || ![8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
    {
        return Err(base_error(
            base,
            "RIDL import ABI must be a UUID in 8-4-4-4-12 form",
        ));
    }
    let digits = bytes
        .iter()
        .copied()
        .filter(|byte| *byte != b'-')
        .collect::<Vec<_>>();
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return Err(base_error(
            base,
            "RIDL import ABI contains a non-hexadecimal digit",
        ));
    }
    let mut uuid = [0; 16];
    for (index, pair) in digits.chunks_exact(2).enumerate() {
        let high = char::from(pair[0]).to_digit(16);
        let low = char::from(pair[1]).to_digit(16);
        let Some(value) = high.zip(low).map(|(high, low)| ((high << 4) | low) as u8) else {
            return Err(base_error(
                base,
                "RIDL import ABI contains a non-hexadecimal digit",
            ));
        };
        uuid[index] = value;
    }
    Ok(uuid)
}

struct ImportCursor<'a> {
    tokens: &'a [Token],
    index: usize,
    base: Span,
}

impl ImportCursor<'_> {
    fn atom_value(&mut self) -> Result<String, TextParseError> {
        let token = self.next()?;
        if let TokenKind::Atom(value) = &token.kind {
            Ok(value.clone())
        } else {
            Err(self.error_at(token.span, "expected identifier"))
        }
    }

    fn string_value(&mut self) -> Result<String, TextParseError> {
        let token = self.next()?;
        if let TokenKind::String(value) = &token.kind {
            decode_kira_string_literal_contents(value).map_err(|error| {
                self.error_at(token.span, format!("invalid string escape: {error}"))
            })
        } else {
            Err(self.error_at(token.span, "expected string literal"))
        }
    }

    fn path(&mut self) -> Result<String, TextParseError> {
        let mut path = self.atom_value()?;
        while self.eat_punctuation('.') {
            path.push('.');
            path.push_str(&self.atom_value()?);
        }
        Ok(path)
    }

    fn expect_atom(&mut self, expected: &str) -> Result<(), TextParseError> {
        if self.atom_value()? == expected {
            Ok(())
        } else {
            Err(self.error(&format!("expected `{expected}`")))
        }
    }

    fn eat_atom(&mut self, expected: &str) -> bool {
        if self
            .tokens
            .get(self.index)
            .is_some_and(|token| matches!(&token.kind, TokenKind::Atom(value) if value == expected))
        {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn expect_punctuation(&mut self, expected: char) -> Result<(), TextParseError> {
        if self.eat_punctuation(expected) {
            Ok(())
        } else {
            Err(self.error(&format!("expected `{expected}`")))
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

    fn is_done(&self) -> bool {
        self.index == self.tokens.len()
    }

    fn skip_newlines(&mut self) {
        while self
            .tokens
            .get(self.index)
            .is_some_and(|token| matches!(token.kind, TokenKind::Newline))
        {
            self.index += 1;
        }
    }

    fn type_value(&mut self, source: &str) -> Result<Type, TextParseError> {
        self.skip_newlines();
        let start = self.index;
        let mut nesting = Vec::new();
        let mut end = start;
        while let Some(token) = self.tokens.get(end) {
            match token.kind {
                TokenKind::Newline if nesting.is_empty() => break,
                TokenKind::Punctuation(',') | TokenKind::Punctuation('}') if nesting.is_empty() => {
                    break;
                }
                TokenKind::Punctuation(open @ ('(' | '[' | '{' | '<')) => nesting.push(open),
                TokenKind::Punctuation(close @ (')' | ']' | '}' | '>')) => {
                    if nesting.last().is_some_and(|open| {
                        matches!(
                            (*open, close),
                            ('(', ')') | ('[', ']') | ('{', '}') | ('<', '>')
                        )
                    }) {
                        nesting.pop();
                    }
                }
                _ => {}
            }
            end += 1;
        }
        if start == end {
            return Err(self.error("imported entity is missing its type"));
        }
        let first = &self.tokens[start];
        let last = &self.tokens[end - 1];
        let text = source
            .get(first.span.start..last.span.end)
            .ok_or_else(|| self.error("imported entity type has an invalid source span"))?;
        self.index = end;
        parse_type(text).map_err(|error| {
            self.error_at(
                Span::new(
                    first.span.start.saturating_add(error.span.start),
                    first.span.start.saturating_add(error.span.end),
                ),
                error.message,
            )
        })
    }

    fn next(&mut self) -> Result<Token, TextParseError> {
        let Some(token) = self.tokens.get(self.index).cloned() else {
            return Err(self.error("unexpected end of import declaration"));
        };
        self.index += 1;
        Ok(token)
    }

    fn error(&self, message: &str) -> TextParseError {
        let span = self
            .tokens
            .get(self.index)
            .or_else(|| self.tokens.last())
            .map_or(self.base, |token| token.span);
        self.error_at(span, message)
    }

    fn error_at(&self, span: Span, message: impl Into<String>) -> TextParseError {
        text_error(self.base, span, message)
    }
}

fn text_error(base: Span, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            base.start.saturating_add(span.start),
            base.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}

fn base_error(base: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: base,
        message: message.into(),
    }
}
