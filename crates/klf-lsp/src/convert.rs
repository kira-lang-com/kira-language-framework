//! Between protocol values and the analysis host's byte offsets and paths
//! (`KLF-EDITOR.md` sections 8.2 and 12).

use klf_facts::{SymbolKind, Token, TokenClass, TokenModifier};
use klf_ide::{FileSpan, Snapshot};
use klf_text::{Encoding, LineIndex};
use lsp_types::{
    Location, Position, Range, SemanticToken, SemanticTokenModifier, SemanticTokenType,
    SemanticTokensLegend, Uri,
};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The token types sent, in legend order: LSP's own names, then the two
/// custom ones of section 12.2.
const TOKEN_TYPES: &[&str] = &[
    "comment",
    "string",
    "escapeSequence",
    "number",
    "keyword",
    "operator",
    "namespace",
    "type",
    "struct",
    "class",
    "enum",
    "interface",
    "typeParameter",
    "function",
    "method",
    "macro",
    "variable",
    "parameter",
    "property",
    "enumMember",
    "label",
    "decorator",
];

#[must_use]
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES
            .iter()
            .map(|name| SemanticTokenType::new(name))
            .collect(),
        token_modifiers: TokenModifier::ALL
            .iter()
            .map(|modifier| SemanticTokenModifier::from(lower_first(modifier.name())))
            .collect(),
    }
}

fn lower_first(name: &str) -> String {
    let mut lowered = name.to_owned();
    if let Some(first) = lowered.get_mut(0..1) {
        first.make_ascii_lowercase();
    }
    lowered
}

/// The legend index and the modifier bits a class adds, or `None` for the
/// classes that are not sent.
fn token_type(class: TokenClass) -> Option<(u32, u32)> {
    let readonly = TokenModifier::Readonly.bit();
    let (name, modifiers) = match class {
        TokenClass::Comment => ("comment", 0),
        TokenClass::DocComment => ("comment", TokenModifier::Documentation.bit()),
        TokenClass::String | TokenClass::Character => ("string", 0),
        TokenClass::Escape => ("escapeSequence", 0),
        TokenClass::Number => ("number", 0),
        TokenClass::Boolean => ("keyword", readonly),
        TokenClass::Keyword => ("keyword", 0),
        TokenClass::Operator => ("operator", 0),
        TokenClass::Namespace => ("namespace", 0),
        TokenClass::Type => ("type", 0),
        TokenClass::Struct => ("struct", 0),
        TokenClass::Class => ("class", 0),
        TokenClass::Enum => ("enum", 0),
        TokenClass::Interface => ("interface", 0),
        TokenClass::TypeParameter => ("typeParameter", 0),
        TokenClass::Function => ("function", 0),
        TokenClass::Method => ("method", 0),
        TokenClass::Macro => ("macro", 0),
        TokenClass::Variable => ("variable", 0),
        TokenClass::Parameter => ("parameter", 0),
        TokenClass::Property => ("property", 0),
        TokenClass::EnumMember => ("enumMember", 0),
        TokenClass::Constant => ("variable", readonly),
        TokenClass::Label => ("label", 0),
        TokenClass::Annotation => ("decorator", 0),
        TokenClass::Builtin => ("type", TokenModifier::DefaultLibrary.bit()),
        TokenClass::Punctuation | TokenClass::Error => return None,
    };
    let index = TOKEN_TYPES.iter().position(|known| *known == name)?;
    Some((u32::try_from(index).ok()?, modifiers))
}

/// Encodes tokens relative to each other (LSP 3.17 semantic tokens),
/// splitting a token that spans lines unless the client takes those.
#[must_use]
pub fn encode_tokens(
    tokens: &[Token],
    index: &LineIndex,
    encoding: Encoding,
    multiline: bool,
) -> Vec<SemanticToken> {
    let mut encoded = Vec::with_capacity(tokens.len());
    let (mut line, mut column) = (0, 0);
    for token in tokens {
        let Some((kind, modifiers)) = token_type(token.class) else {
            continue;
        };
        let mut pieces = Vec::new();
        let first = index.line_of(token.start);
        let last = index.line_of(token.end.saturating_sub(1).max(token.start));
        if multiline || first == last {
            pieces.push((token.start, token.end));
        } else {
            for current in first..=last {
                let start = token.start.max(index.line_start(current));
                let end = token.end.min(index.line_end(current));
                if end > start {
                    pieces.push((start, end));
                }
            }
        }
        for (start, end) in pieces {
            let from = index.position(start, encoding);
            let to = index.position(end, encoding);
            let length = if to.line == from.line {
                to.column - from.column
            } else {
                index.line_width(from.line, encoding) - from.column
            };
            let delta_line = from.line - line;
            let delta_start = if delta_line == 0 {
                from.column - column
            } else {
                from.column
            };
            encoded.push(SemanticToken {
                delta_line,
                delta_start,
                length,
                token_type: kind,
                token_modifiers_bitset: token.modifiers | modifiers,
            });
            (line, column) = (from.line, from.column);
        }
    }
    encoded
}

#[must_use]
pub fn position(index: &LineIndex, offset: u32, encoding: Encoding) -> Position {
    let at = index.position(offset, encoding);
    Position::new(at.line, at.column)
}

#[must_use]
pub fn range(index: &LineIndex, start: u32, end: u32, encoding: Encoding) -> Range {
    Range::new(
        position(index, start, encoding),
        position(index, end, encoding),
    )
}

#[must_use]
pub fn offset(index: &LineIndex, at: Position, encoding: Encoding) -> u32 {
    index.offset(
        klf_text::Position {
            line: at.line,
            column: at.character,
        },
        encoding,
    )
}

#[must_use]
pub fn text_range(range: Range) -> klf_text::Range {
    klf_text::Range {
        start: klf_text::Position {
            line: range.start.line,
            column: range.start.character,
        },
        end: klf_text::Position {
            line: range.end.line,
            column: range.end.character,
        },
    }
}

/// A span as a protocol location.
#[must_use]
pub fn location(snapshot: &Snapshot, span: &FileSpan, encoding: Encoding) -> Option<Location> {
    let index = snapshot.line_index(&span.path)?;
    Some(Location::new(
        uri(&span.path)?,
        range(&index, span.start, span.end, encoding),
    ))
}

#[must_use]
pub fn uri(path: &Path) -> Option<Uri> {
    let mut text = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            text.push(char::from(byte));
        } else {
            let _ = write!(text, "%{byte:02X}");
        }
    }
    Uri::from_str(&text).ok()
}

#[must_use]
pub fn path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut bytes = Vec::with_capacity(rest.len());
    let mut input = rest.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = char::from(input.next()?).to_digit(16)?;
            let low = char::from(input.next()?).to_digit(16)?;
            bytes.push(u8::try_from(high * 16 + low).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

#[must_use]
pub fn symbol_kind(kind: SymbolKind) -> lsp_types::SymbolKind {
    match kind {
        SymbolKind::Macro | SymbolKind::Stage => lsp_types::SymbolKind::FUNCTION,
        SymbolKind::Trait => lsp_types::SymbolKind::INTERFACE,
        SymbolKind::Resource => lsp_types::SymbolKind::VARIABLE,
        SymbolKind::Label => lsp_types::SymbolKind::KEY,
        SymbolKind::Test => lsp_types::SymbolKind::METHOD,
        other => serde_json::from_value(serde_json::json!(other.code() + 1))
            .unwrap_or(lsp_types::SymbolKind::VARIABLE),
    }
}

#[must_use]
pub fn completion_kind(kind: SymbolKind) -> lsp_types::CompletionItemKind {
    use lsp_types::CompletionItemKind as Item;
    match kind {
        SymbolKind::File => Item::FILE,
        SymbolKind::Module | SymbolKind::Namespace | SymbolKind::Package => Item::MODULE,
        SymbolKind::Class => Item::CLASS,
        SymbolKind::Method | SymbolKind::Test => Item::METHOD,
        SymbolKind::Property => Item::PROPERTY,
        SymbolKind::Field => Item::FIELD,
        SymbolKind::Constructor => Item::CONSTRUCTOR,
        SymbolKind::Enum => Item::ENUM,
        SymbolKind::Interface | SymbolKind::Trait => Item::INTERFACE,
        SymbolKind::Function | SymbolKind::Macro | SymbolKind::Stage => Item::FUNCTION,
        SymbolKind::Constant => Item::CONSTANT,
        SymbolKind::EnumMember => Item::ENUM_MEMBER,
        SymbolKind::Struct => Item::STRUCT,
        SymbolKind::Event => Item::EVENT,
        SymbolKind::Operator => Item::OPERATOR,
        SymbolKind::TypeParameter => Item::TYPE_PARAMETER,
        SymbolKind::Label => Item::REFERENCE,
        _ => Item::VARIABLE,
    }
}
