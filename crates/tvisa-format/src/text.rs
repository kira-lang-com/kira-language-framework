use std::fmt;

use tir_diag::Span;
use tir_target::Profile;

use tir_format::text::{Token, TokenKind, lex};

use crate::signature::{TextSignature, parse_signature};
use crate::text_entries::{TextGpuEntry, parse_gpu_entry_text};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TvisaVersion {
    pub year: u8,
    pub month: u8,
    pub week: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageKind {
    Executable,
    Framework,
    Plugin,
    Driver,
    Kernel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextImageHeader {
    pub version: TvisaVersion,
    pub profile: Profile,
    pub image: String,
    pub kind: ImageKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextItemKind {
    Signature,
    Import,
    Data,
    Function,
    Variants,
    Dispatch,
    Entry,
    Metadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextItem {
    pub kind: TextItemKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextImage {
    pub header: TextImageHeader,
    pub items: Vec<TextItem>,
    pub signatures: Vec<TextSignature>,
    pub entries: Vec<TextGpuEntry>,
    source: String,
}

impl TextImage {
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
}

struct PendingItem {
    kind: TextItemKind,
    start: usize,
    end: usize,
    depth: i32,
    root_groups: usize,
    has_slots: bool,
}

pub fn parse_text_image(source: &str) -> Result<TextImage, TextParseError> {
    let tokens = lex(source).map_err(|error| TextParseError {
        span: error.span,
        message: error.message.to_owned(),
    })?;
    let mut header = None;
    let mut items = Vec::new();
    let mut pending: Option<PendingItem> = None;
    for line in split_lines(&tokens) {
        if line.tokens.is_empty() {
            continue;
        }
        if header.is_none() {
            header = Some(parse_header(&line.tokens)?);
            continue;
        }
        if let Some(item) = pending.as_ref() {
            if matches!(item.kind, TextItemKind::Signature | TextItemKind::Data)
                && item.depth == 0
                && is_item_start(&line.tokens)
            {
                finish_item(&mut pending, &mut items);
            } else {
                let item = pending.as_mut().expect("pending TVISA item exists");
                if item.depth == 0
                    && item.root_groups < required_root_groups(item.kind, item.has_slots)
                    && is_item_start(&line.tokens)
                {
                    return Err(error(
                        Span::new(item.start, item.end),
                        "TVISA item requires a body",
                    ));
                }
                item.end = line.tokens.last().map_or(item.end, |token| token.span.end);
                if item.kind == TextItemKind::Function && item.depth == 0 && item.root_groups == 0 {
                    item.has_slots |= contains_atom(&line.tokens, "slots");
                }
                adjust_braces(item, &line.tokens)?;
                if !matches!(item.kind, TextItemKind::Signature | TextItemKind::Data)
                    && item.depth == 0
                    && item.root_groups >= required_root_groups(item.kind, item.has_slots)
                {
                    finish_item(&mut pending, &mut items);
                }
                continue;
            }
        }
        let kind = item_kind(&line.tokens)?;
        let start = line.tokens[0].span.start;
        let end = line.tokens.last().map_or(start, |token| token.span.end);
        let mut item = PendingItem {
            kind,
            start,
            end,
            depth: 0,
            root_groups: 0,
            has_slots: contains_atom(&line.tokens, "slots"),
        };
        adjust_braces(&mut item, &line.tokens)?;
        if matches!(item.kind, TextItemKind::Signature | TextItemKind::Data) {
            pending = Some(item);
        } else if item.depth == 0
            && item.root_groups >= required_root_groups(item.kind, item.has_slots)
        {
            items.push(TextItem {
                kind,
                span: Span::new(start, end),
            });
        } else {
            pending = Some(item);
        }
    }
    if let Some(item) = pending {
        if item.depth != 0 {
            return Err(error(
                Span::new(item.start, item.end),
                "unterminated TVISA item",
            ));
        }
        if item.kind != TextItemKind::Signature
            && item.root_groups < required_root_groups(item.kind, item.has_slots)
        {
            return Err(error(
                Span::new(item.start, item.end),
                "TVISA item requires a body",
            ));
        }
        items.push(TextItem {
            kind: item.kind,
            span: Span::new(item.start, item.end),
        });
    }
    let header = header.ok_or_else(|| error(Span::new(0, 0), "missing TVISA header"))?;
    let mut signatures = Vec::new();
    for item in items
        .iter()
        .filter(|item| item.kind == TextItemKind::Signature)
    {
        let source_slice = source
            .get(item.span.start..item.span.end)
            .unwrap_or_default();
        let mut signature = parse_signature(source_slice).map_err(|mut parse_error| {
            parse_error.span = Span::new(
                parse_error.span.start.saturating_add(item.span.start),
                parse_error.span.end.saturating_add(item.span.start),
            );
            parse_error
        })?;
        signature.span = Span::new(
            signature.span.start.saturating_add(item.span.start),
            signature.span.end.saturating_add(item.span.start),
        );
        if signatures
            .iter()
            .any(|existing: &TextSignature| existing.name == signature.name)
        {
            return Err(error(
                signature.span,
                &format!("duplicate TVISA signature ${}", signature.name),
            ));
        }
        signatures.push(signature);
    }
    let mut entries = Vec::new();
    for item in items.iter().filter(|item| item.kind == TextItemKind::Entry) {
        let source_slice = source
            .get(item.span.start..item.span.end)
            .unwrap_or_default();
        let entry = parse_gpu_entry_text(source_slice, item.span.start)?;
        if entries
            .iter()
            .any(|existing: &TextGpuEntry| existing.function == entry.function)
        {
            return Err(error(
                entry.span,
                &format!("duplicate TVISA GPU entry {}", entry.function),
            ));
        }
        entries.push(entry);
    }
    Ok(TextImage {
        header,
        items,
        signatures,
        entries,
        source: source.to_owned(),
    })
}

fn required_root_groups(kind: TextItemKind, has_slots: bool) -> usize {
    match kind {
        TextItemKind::Function if has_slots => 2,
        TextItemKind::Function
        | TextItemKind::Variants
        | TextItemKind::Dispatch
        | TextItemKind::Entry => 1,
        TextItemKind::Signature
        | TextItemKind::Import
        | TextItemKind::Data
        | TextItemKind::Metadata => 0,
    }
}

fn is_item_start(tokens: &[Token]) -> bool {
    matches!(
        tokens.first().and_then(|token| match &token.kind {
            TokenKind::Atom(atom) => Some(atom.as_str()),
            _ => None,
        }),
        Some("sig" | "import" | "data" | "func" | "variants" | "dispatch" | "entry" | "metadata")
    )
}

fn contains_atom(tokens: &[Token], value: &str) -> bool {
    tokens
        .iter()
        .any(|token| matches!(&token.kind, TokenKind::Atom(atom) if atom == value))
}

fn item_kind(tokens: &[Token]) -> Result<TextItemKind, TextParseError> {
    let token = &tokens[0];
    let TokenKind::Atom(name) = &token.kind else {
        return Err(error(token.span, "expected a TVISA item"));
    };
    match name.as_str() {
        "sig" => Ok(TextItemKind::Signature),
        "import" => Ok(TextItemKind::Import),
        "data" => Ok(TextItemKind::Data),
        "func" => Ok(TextItemKind::Function),
        "variants" => Ok(TextItemKind::Variants),
        "dispatch" => Ok(TextItemKind::Dispatch),
        "entry" => Ok(TextItemKind::Entry),
        "metadata" => Ok(TextItemKind::Metadata),
        _ => Err(error(token.span, "unknown TVISA item")),
    }
}

fn adjust_braces(item: &mut PendingItem, tokens: &[Token]) -> Result<(), TextParseError> {
    for token in tokens {
        if let TokenKind::Punctuation(character) = token.kind {
            if character == '{' {
                if item.depth == 0 {
                    item.root_groups += 1;
                }
                item.depth += 1;
            } else if character == '}' {
                item.depth -= 1;
                if item.depth < 0 {
                    return Err(error(token.span, "unexpected closing brace"));
                }
            }
        }
    }
    Ok(())
}

fn finish_item(pending: &mut Option<PendingItem>, items: &mut Vec<TextItem>) {
    if let Some(item) = pending.take() {
        items.push(TextItem {
            kind: item.kind,
            span: Span::new(item.start, item.end),
        });
    }
}

fn parse_header(tokens: &[Token]) -> Result<TextImageHeader, TextParseError> {
    let mut cursor = Cursor { tokens, index: 0 };
    cursor.expect_atom("tvisa")?;
    let year = cursor.number("invalid TVISA year")?;
    cursor.expect_punctuation('.')?;
    let month = cursor.number("invalid TVISA month")?;
    let has_week = cursor.eat_punctuation('.');
    let week = if has_week {
        cursor.number("invalid TVISA week")?
    } else {
        0
    };
    if !(1..=99).contains(&year)
        || !(1..=12).contains(&month)
        || (has_week && !(1..=5).contains(&week))
    {
        return Err(cursor.error("TVISA version is outside the supported range"));
    }
    cursor.expect_atom("profile")?;
    let profile = match cursor.atom()?.as_str() {
        "app" => Profile::App,
        "system" => Profile::System,
        "driver" => Profile::Driver,
        "kernel" => Profile::Kernel,
        _ => return Err(cursor.error("unknown TVISA profile")),
    };
    cursor.expect_atom("image")?;
    let image = cursor.string()?;
    cursor.expect_atom("kind")?;
    let kind = match cursor.atom()?.as_str() {
        "executable" => ImageKind::Executable,
        "framework" => ImageKind::Framework,
        "plugin" => ImageKind::Plugin,
        "driver" => ImageKind::Driver,
        "kernel" => ImageKind::Kernel,
        _ => return Err(cursor.error("unknown TVISA image kind")),
    };
    if cursor.index != tokens.len() {
        return Err(cursor.error("unexpected token after TVISA header"));
    }
    Ok(TextImageHeader {
        version: TvisaVersion {
            year: year as u8,
            month: month as u8,
            week: week as u8,
        },
        profile,
        image,
        kind,
    })
}

fn split_lines(tokens: &[Token]) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut line_tokens = Vec::new();
    for token in tokens {
        match token.kind {
            TokenKind::Newline | TokenKind::End => {
                lines.push(Line {
                    tokens: std::mem::take(&mut line_tokens),
                });
            }
            _ => line_tokens.push(token.clone()),
        }
    }
    lines
}

pub(crate) fn error(span: Span, message: &str) -> TextParseError {
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
    fn expect_atom(&mut self, expected: &str) -> Result<(), TextParseError> {
        if self.atom()? == expected {
            Ok(())
        } else {
            Err(self.error("unexpected identifier"))
        }
    }

    fn atom(&mut self) -> Result<String, TextParseError> {
        let token = self.next()?;
        match &token.kind {
            TokenKind::Atom(atom) => Ok(atom.clone()),
            _ => Err(error(token.span, "expected identifier")),
        }
    }

    fn string(&mut self) -> Result<String, TextParseError> {
        let token = self.next()?;
        match &token.kind {
            TokenKind::String(value) => Ok(value.clone()),
            _ => Err(error(token.span, "expected string literal")),
        }
    }

    fn number(&mut self, message: &str) -> Result<u16, TextParseError> {
        self.atom()?.parse::<u16>().map_err(|_| self.error(message))
    }

    fn expect_punctuation(&mut self, expected: char) -> Result<(), TextParseError> {
        if self.eat_punctuation(expected) {
            Ok(())
        } else {
            Err(self.error("unexpected punctuation"))
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

    fn next(&mut self) -> Result<&Token, TextParseError> {
        let token = self
            .tokens
            .get(self.index)
            .ok_or_else(|| self.error("unexpected end of TVISA header"))?;
        self.index += 1;
        Ok(token)
    }

    fn error(&self, message: &str) -> TextParseError {
        let span = self.tokens.get(self.index).map_or_else(
            || {
                self.tokens
                    .last()
                    .map_or(Span::new(0, 0), |token| token.span)
            },
            |token| token.span,
        );
        error(span, message)
    }
}
