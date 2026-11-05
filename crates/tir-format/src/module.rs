use std::fmt;

use tir_diag::Span;
use tir_target::{Platform, Profile};

use crate::text::{
    Token, TokenKind, decode_kira_byte_string, decode_kira_string_literal_contents, lex,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TirVersion {
    pub year: u8,
    pub month: u8,
    pub week: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticsMode {
    Defined,
    Permissive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProducerRecord {
    pub name: String,
    pub version: String,
    pub languages: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Initializer {
    pub path: String,
    pub priority: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleHeader {
    pub path: String,
    pub version: TirVersion,
    pub profile: Profile,
    pub platform: Option<Platform>,
    pub semantics: Option<SemanticsMode>,
    pub producer: Option<ProducerRecord>,
    pub attributes: Vec<(String, String)>,
    pub initializers: Vec<Initializer>,
    pub finalizers: Vec<Initializer>,
    pub source_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextItemKind {
    Import,
    Type,
    Global,
    Function,
    Entry,
    Variants,
    Trait,
    Conformance,
    VTable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextItem {
    pub kind: TextItemKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextModule {
    pub header: ModuleHeader,
    pub items: Vec<TextItem>,
    source: String,
}

impl TextModule {
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn item_text(&self, item: TextItem) -> &str {
        self.source
            .get(item.span.start..item.span.end)
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextParseError {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for TextParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TextParseError {}

#[derive(Clone)]
struct Line {
    tokens: Vec<Token>,
    span: Span,
}

struct PendingItem {
    kind: TextItemKind,
    span: Span,
    depth: i32,
    needs_policy: bool,
    has_policy: bool,
}

pub fn parse_text_module(source: &str) -> Result<TextModule, TextParseError> {
    let tokens = lex(source).map_err(|error| TextParseError {
        span: error.span,
        message: error.message.to_owned(),
    })?;
    validate_string_escapes(&tokens)?;
    let lines = split_lines(&tokens, source.len());
    let mut header = None;
    let mut items = Vec::new();
    let mut pending: Option<PendingItem> = None;

    for line in lines {
        if line.tokens.is_empty() {
            continue;
        }
        if header.is_none() {
            header = Some(parse_header(&line.tokens)?);
            continue;
        }

        if let Some(item) = pending.as_mut() {
            if item.depth == 0 && item.needs_policy && !starts_with(&line.tokens, "policy") {
                if !item.has_policy {
                    return Err(parse_error(
                        item.span,
                        "type declaration is missing its policy",
                    ));
                }
                finish_pending(&mut pending, &mut items);
                process_top_level_line(line, &mut pending, &mut items)?;
                continue;
            }
            item.span.end = line.span.end;
            if item.needs_policy && starts_with(&line.tokens, "policy") {
                item.has_policy = true;
            }
            adjust_depth(item, &line.tokens)?;
            if item.depth == 0 && (!item.needs_policy || item.has_policy) {
                finish_pending(&mut pending, &mut items);
            }
            continue;
        }

        if is_header_attribute(&line.tokens) {
            let Some(header) = header.as_mut() else {
                return Err(parse_error(line.span, "missing module header"));
            };
            parse_header_attribute(header, &line.tokens)?;
        } else {
            process_top_level_line(line, &mut pending, &mut items)?;
        }
    }

    if let Some(item) = pending {
        if item.depth != 0 {
            return Err(parse_error(item.span, "unterminated top-level item"));
        }
        if item.needs_policy && !item.has_policy {
            return Err(parse_error(
                item.span,
                "type declaration is missing its policy",
            ));
        }
        items.push(TextItem {
            kind: item.kind,
            span: item.span,
        });
    }

    let header = header.ok_or_else(|| parse_error(Span::new(0, 0), "missing module header"))?;
    Ok(TextModule {
        header,
        items,
        source: source.to_owned(),
    })
}

fn validate_string_escapes(tokens: &[Token]) -> Result<(), TextParseError> {
    for (index, token) in tokens.iter().enumerate() {
        let TokenKind::String(raw) = &token.kind else {
            continue;
        };
        let result = if is_byte_string_operand(tokens, index) {
            decode_kira_byte_string(raw)
                .map(|_| ())
                .map_err(|error| error.to_string())
        } else {
            decode_kira_string_literal_contents(raw)
                .map(|_| ())
                .map_err(|error| error.to_string())
        };
        if let Err(message) = result {
            return Err(TextParseError {
                span: token.span,
                message: format!("invalid string escape: {message}"),
            });
        }
    }
    Ok(())
}

fn is_byte_string_operand(tokens: &[Token], string_index: usize) -> bool {
    if string_index < 2 || atom(&tokens[string_index - 1]) != Some("b") {
        return false;
    }
    let line_start = tokens[..string_index]
        .iter()
        .rposition(|token| matches!(token.kind, TokenKind::Newline))
        .map_or(0, |index| index + 1);
    let prefix = &tokens[line_start..string_index - 1];
    if string_index >= line_start + 3
        && matches!(tokens[string_index - 2].kind, TokenKind::Punctuation('='))
    {
        return true;
    }
    prefix.windows(3).any(|window| {
        atom(&window[0]) == Some("const")
            && matches!(window[1].kind, TokenKind::Punctuation('.'))
            && atom(&window[2]) == Some("bytes")
    })
}

fn process_top_level_line(
    line: Line,
    pending: &mut Option<PendingItem>,
    items: &mut Vec<TextItem>,
) -> Result<(), TextParseError> {
    let first = atom(&line.tokens[0])
        .ok_or_else(|| parse_error(line.tokens[0].span, "expected a top-level TIR item"))?;
    let kind = match first {
        "import" => TextItemKind::Import,
        "type" => TextItemKind::Type,
        "global" => TextItemKind::Global,
        "func" => TextItemKind::Function,
        "entry" => TextItemKind::Entry,
        "variants" => TextItemKind::Variants,
        "trait" => TextItemKind::Trait,
        "conformance" => TextItemKind::Conformance,
        "vtable" => TextItemKind::VTable,
        _ => {
            return Err(parse_error(
                line.tokens[0].span,
                "unknown top-level TIR item",
            ));
        }
    };
    let mut item = PendingItem {
        kind,
        span: line.span,
        depth: 0,
        needs_policy: kind == TextItemKind::Type,
        has_policy: false,
    };
    item.has_policy = item.needs_policy && has_top_level_policy(&line.tokens);
    adjust_depth(&mut item, &line.tokens)?;
    if item.depth == 0 && (!item.needs_policy || item.has_policy) {
        items.push(TextItem {
            kind: item.kind,
            span: item.span,
        });
    } else {
        *pending = Some(item);
    }
    Ok(())
}

fn finish_pending(pending: &mut Option<PendingItem>, items: &mut Vec<TextItem>) {
    if let Some(item) = pending.take() {
        items.push(TextItem {
            kind: item.kind,
            span: item.span,
        });
    }
}

fn adjust_depth(item: &mut PendingItem, tokens: &[Token]) -> Result<(), TextParseError> {
    for token in tokens {
        if let TokenKind::Punctuation(character) = &token.kind
            && matches!(character, '{' | '}')
        {
            item.depth += if *character == '{' { 1 } else { -1 };
            if item.depth < 0 {
                return Err(parse_error(token.span, "unexpected closing brace"));
            }
        }
    }
    Ok(())
}

fn has_top_level_policy(tokens: &[Token]) -> bool {
    let mut depth = 0i32;
    for token in tokens {
        if depth == 0 && atom(token) == Some("policy") {
            return true;
        }
        if let TokenKind::Punctuation(character) = &token.kind {
            match character {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
    }
    false
}

fn parse_header(tokens: &[Token]) -> Result<ModuleHeader, TextParseError> {
    let mut cursor = Cursor { tokens, index: 0 };
    cursor.expect_atom("module")?;
    cursor.expect_punctuation('@')?;
    let path = cursor.path()?;
    cursor.expect_atom("tir")?;
    let version = cursor.version()?;
    cursor.expect_atom("profile")?;
    let profile = match cursor.atom_value()?.as_str() {
        "app" => Profile::App,
        "system" => Profile::System,
        "driver" => Profile::Driver,
        "kernel" => Profile::Kernel,
        _ => return Err(cursor.error("unknown module profile")),
    };
    let platform = if cursor.eat_atom("platform") {
        Some(
            parse_platform(&cursor.atom_value()?)
                .ok_or_else(|| cursor.error("unknown platform"))?,
        )
    } else {
        None
    };
    let semantics = if cursor.eat_atom("semantics") {
        Some(match cursor.atom_value()?.as_str() {
            "defined" => SemanticsMode::Defined,
            "permissive" => SemanticsMode::Permissive,
            _ => return Err(cursor.error("unknown semantic mode")),
        })
    } else {
        None
    };
    let mut header = ModuleHeader {
        path,
        version,
        profile,
        platform,
        semantics,
        producer: None,
        attributes: Vec::new(),
        initializers: Vec::new(),
        finalizers: Vec::new(),
        source_hash: None,
    };
    while cursor.index < tokens.len() {
        parse_header_attribute_from_cursor(&mut header, &mut cursor)?;
    }
    Ok(header)
}

fn parse_header_attribute(
    header: &mut ModuleHeader,
    tokens: &[Token],
) -> Result<(), TextParseError> {
    let mut cursor = Cursor { tokens, index: 0 };
    parse_header_attribute_from_cursor(header, &mut cursor)?;
    if cursor.index != tokens.len() {
        return Err(cursor.error("unexpected token in module header attribute"));
    }
    Ok(())
}

fn parse_header_attribute_from_cursor(
    header: &mut ModuleHeader,
    cursor: &mut Cursor<'_>,
) -> Result<(), TextParseError> {
    let attribute = cursor.atom_value()?;
    match attribute.as_str() {
        "producer" => {
            if header.producer.is_some() {
                return Err(cursor.error("duplicate producer record"));
            }
            let name = cursor.string_value()?;
            let version = cursor.string_value()?;
            cursor.expect_atom("languages")?;
            cursor.expect_punctuation('[')?;
            let mut languages = Vec::new();
            if !cursor.eat_punctuation(']') {
                loop {
                    languages.push(cursor.atom_value()?);
                    if cursor.eat_punctuation(']') {
                        break;
                    }
                    cursor.expect_punctuation(',')?;
                }
            }
            header.producer = Some(ProducerRecord {
                name,
                version,
                languages,
            });
        }
        "attribute" => {
            let key = cursor.string_value()?;
            cursor.expect_punctuation('=')?;
            let value = cursor.string_value()?;
            header.attributes.push((key, value));
        }
        "init" | "fini" => {
            cursor.expect_punctuation('@')?;
            let path = cursor.path()?;
            cursor.expect_atom("priority")?;
            let priority_text = cursor.atom_value()?;
            let priority = crate::numeric_literals::parse_signed_integer(&priority_text)
                .and_then(|value| u16::try_from(value).ok())
                .ok_or_else(|| cursor.error("initializer priority is out of range"))?;
            let entries = if attribute == "init" {
                &mut header.initializers
            } else {
                &mut header.finalizers
            };
            entries.push(Initializer { path, priority });
        }
        "source" => {
            let source_hash = cursor.string_value()?;
            if header.source_hash.replace(source_hash).is_some() {
                return Err(cursor.error("duplicate source hash"));
            }
        }
        _ => return Err(cursor.error("unknown module header attribute")),
    }
    Ok(())
}

fn is_header_attribute(tokens: &[Token]) -> bool {
    matches!(
        tokens.first().and_then(atom),
        Some("producer" | "attribute" | "init" | "fini" | "source")
    )
}

fn parse_platform(value: &str) -> Option<Platform> {
    Some(match value {
        "portable" => Platform::Portable,
        "tessera" => Platform::Tessera,
        "macos" => Platform::Macos,
        "ios" => Platform::Ios,
        "windows" => Platform::Windows,
        "linux" => Platform::Linux,
        "android" => Platform::Android,
        "web" => Platform::Web,
        "wasi" => Platform::Wasi,
        _ => return None,
    })
}

fn split_lines(tokens: &[Token], source_len: usize) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut line_tokens = Vec::new();
    let mut line_start = 0;
    for token in tokens {
        match &token.kind {
            TokenKind::Newline => {
                lines.push(Line {
                    tokens: std::mem::take(&mut line_tokens),
                    span: Span::new(line_start, token.span.end),
                });
                line_start = token.span.end;
            }
            TokenKind::End => {
                if line_start < source_len || !line_tokens.is_empty() {
                    lines.push(Line {
                        tokens: std::mem::take(&mut line_tokens),
                        span: Span::new(line_start, source_len),
                    });
                }
            }
            _ => line_tokens.push(token.clone()),
        }
    }
    lines
}

fn starts_with(tokens: &[Token], expected: &str) -> bool {
    tokens.first().and_then(atom) == Some(expected)
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(atom) = &token.kind {
        Some(atom)
    } else {
        None
    }
}

fn parse_error(span: Span, message: &str) -> TextParseError {
    TextParseError {
        span,
        message: message.to_owned(),
    }
}

struct Cursor<'a> {
    tokens: &'a [Token],
    index: usize,
}

impl Cursor<'_> {
    fn atom_value(&mut self) -> Result<String, TextParseError> {
        let token = self.next()?;
        atom(token)
            .map(str::to_owned)
            .ok_or_else(|| parse_error(token.span, "expected identifier"))
    }

    fn string_value(&mut self) -> Result<String, TextParseError> {
        let token = self.next()?;
        if let TokenKind::String(value) = &token.kind {
            decode_kira_string_literal_contents(value).map_err(|error| TextParseError {
                span: token.span,
                message: format!("invalid string escape: {error}"),
            })
        } else {
            Err(parse_error(token.span, "expected string literal"))
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

    fn version(&mut self) -> Result<TirVersion, TextParseError> {
        let year = self.version_part()?;
        self.expect_punctuation('.')?;
        let month = self.version_part()?;
        self.expect_punctuation('.')?;
        let week = self.version_part()?;
        if !(1..=99).contains(&year) || !(1..=12).contains(&month) || !(1..=5).contains(&week) {
            return Err(self.error("TIR version is outside the supported range"));
        }
        Ok(TirVersion {
            year: year as u8,
            month: month as u8,
            week: week as u8,
        })
    }

    fn version_part(&mut self) -> Result<u16, TextParseError> {
        self.atom_value()?
            .parse::<u16>()
            .map_err(|_| self.error("invalid TIR version"))
    }

    fn expect_atom(&mut self, expected: &str) -> Result<(), TextParseError> {
        if self.eat_atom(expected) {
            Ok(())
        } else {
            Err(self.error("unexpected identifier"))
        }
    }

    fn eat_atom(&mut self, expected: &str) -> bool {
        if self.tokens.get(self.index).and_then(atom) == Some(expected) {
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
            Err(self.error("unexpected punctuation"))
        }
    }

    fn eat_punctuation(&mut self, expected: char) -> bool {
        if matches!(self.tokens.get(self.index).map(|token| &token.kind), Some(TokenKind::Punctuation(actual)) if *actual == expected)
        {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn next(&mut self) -> Result<&Token, TextParseError> {
        let token = self
            .tokens
            .get(self.index)
            .ok_or_else(|| self.error("unexpected end of line"))?;
        self.index += 1;
        Ok(token)
    }

    fn error(&self, message: &str) -> TextParseError {
        let span = self
            .tokens
            .get(self.index)
            .map_or(Span::new(0, 0), |token| token.span);
        parse_error(span, message)
    }
}
