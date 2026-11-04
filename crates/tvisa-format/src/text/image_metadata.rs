use std::collections::HashSet;

use tir_diag::Span;
use tir_format::text::{Token, TokenKind, lex};
use tir_target::Profile;

use super::{ImageKind, TextImageHeader, TextItem, TextItemKind, TextParseError, error};

const IMAGE_FIELDS: [&str; 8] = [
    "package_digest",
    "tirc_build",
    "module_hashes",
    "entry_function",
    "initializers",
    "finalizers",
    "gpu_capabilities",
    "minimum_driver_api",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextImageFunction {
    pub function: String,
    pub priority: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextImageMetadata {
    pub package_digest: [u8; 32],
    pub tirc_build: String,
    pub module_hashes: Vec<[u8; 32]>,
    pub entry_function: Option<String>,
    pub initializers: Vec<TextImageFunction>,
    pub finalizers: Vec<TextImageFunction>,
    pub gpu_capabilities: Vec<String>,
    pub minimum_driver_api: Option<(u8, u8)>,
}

impl TextImageMetadata {
    pub(crate) fn gpu_capability_mask(&self) -> Option<u64> {
        self.gpu_capabilities
            .iter()
            .try_fold(0_u64, |mask, capability| {
                let bit = crate::gpu_entries::GPU_CAPABILITY_NAMES
                    .iter()
                    .position(|name| *name == capability)?;
                Some(mask | (1_u64 << bit))
            })
    }
}

pub(super) fn parse(
    source: &str,
    items: &[TextItem],
    header: &TextImageHeader,
) -> Result<TextImageMetadata, TextParseError> {
    let mut image_record = None;
    for item in items
        .iter()
        .filter(|item| item.kind == TextItemKind::Metadata)
    {
        let text = source
            .get(item.span.start..item.span.end)
            .unwrap_or_default();
        if !is_image_metadata(text) {
            continue;
        }
        if image_record.is_some() {
            return Err(error(
                item.span,
                "TVISA image has multiple metadata image records",
            ));
        }
        image_record = Some((text, item.span.start));
    }
    let Some((text, offset)) = image_record else {
        return Err(error(
            Span::new(0, 0),
            "TVISA image requires one metadata image record",
        ));
    };
    let metadata = Parser::new(text)?.parse().map_err(|mut issue| {
        issue.span = Span::new(
            issue.span.start.saturating_add(offset),
            issue.span.end.saturating_add(offset),
        );
        issue
    })?;
    validate(&metadata, header)
        .map_err(|message| error(Span::new(offset, offset + text.len()), &message))?;
    Ok(metadata)
}

fn is_image_metadata(source: &str) -> bool {
    let Ok(tokens) = lex(source) else {
        return false;
    };
    let tokens: Vec<_> = tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect();
    tokens.len() >= 2 && atom(tokens[0]) == Some("metadata") && atom(tokens[1]) == Some("image")
}

fn validate(metadata: &TextImageMetadata, header: &TextImageHeader) -> Result<(), String> {
    if metadata.tirc_build.is_empty() {
        return Err("metadata image tirc_build must be nonempty".to_owned());
    }
    if metadata.module_hashes.is_empty()
        || metadata
            .module_hashes
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err("metadata image module_hashes must be unique, sorted and nonempty".to_owned());
    }
    if metadata
        .gpu_capabilities
        .iter()
        .any(|name| capability_index(name) == usize::MAX)
        || metadata
            .gpu_capabilities
            .windows(2)
            .any(|pair| capability_index(&pair[0]) >= capability_index(&pair[1]))
    {
        return Err(
            "metadata image gpu_capabilities must follow TIR capability-bit order".to_owned(),
        );
    }
    let expected_api = header.kind == ImageKind::Driver;
    if expected_api != metadata.minimum_driver_api.is_some() {
        return Err("minimum_driver_api is present only for driver images".to_owned());
    }
    if (header.kind == ImageKind::Executable) != metadata.entry_function.is_some() {
        return Err("only executable images have entry_function".to_owned());
    }
    let profile_matches = matches!(
        (header.kind, header.profile),
        (
            ImageKind::Executable | ImageKind::Framework | ImageKind::Plugin,
            Profile::App | Profile::System
        ) | (ImageKind::Driver, Profile::Driver)
            | (ImageKind::Kernel, Profile::Kernel)
    );
    if !profile_matches {
        return Err("TVISA image kind does not match its profile".to_owned());
    }
    Ok(())
}

fn capability_index(name: &str) -> usize {
    crate::gpu_entries::GPU_CAPABILITY_NAMES
        .iter()
        .position(|capability| *capability == name)
        .unwrap_or(usize::MAX)
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
}

impl Parser {
    fn new(source: &str) -> Result<Self, TextParseError> {
        let tokens = lex(source).map_err(|issue| TextParseError {
            span: issue.span,
            message: issue.message.to_owned(),
        })?;
        Ok(Self {
            tokens: tokens
                .into_iter()
                .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
                .collect(),
            index: 0,
        })
    }

    fn parse(mut self) -> Result<TextImageMetadata, TextParseError> {
        self.expect_atom("metadata")?;
        self.expect_atom("image")?;
        self.expect_punctuation('{')?;
        let mut seen = HashSet::new();
        let mut field_index = 0;
        let mut package_digest = None;
        let mut tirc_build = None;
        let mut module_hashes = None;
        let mut entry_function = None;
        let mut initializers = None;
        let mut finalizers = None;
        let mut gpu_capabilities = None;
        let mut minimum_driver_api = None;
        loop {
            if self.eat_punctuation('}') {
                break;
            }
            let (key, span) = self.atom_with_span()?;
            if !seen.insert(key.clone()) {
                return Err(error(span, "duplicate metadata image field"));
            }
            if IMAGE_FIELDS.get(field_index).copied() != Some(key.as_str()) {
                let expected = IMAGE_FIELDS
                    .get(field_index)
                    .copied()
                    .unwrap_or("no additional fields");
                return Err(error(
                    span,
                    &format!(
                        "metadata image fields must follow canonical order; expected {expected}"
                    ),
                ));
            }
            field_index += 1;
            self.expect_punctuation(':')?;
            match key.as_str() {
                "package_digest" => {
                    package_digest = Some(parse_digest(&self.string()?, "package_digest", span)?);
                }
                "tirc_build" => tirc_build = Some(self.string()?),
                "module_hashes" => {
                    let hashes = self
                        .string_array()?
                        .into_iter()
                        .map(|value| parse_digest(&value, "module_hashes", span))
                        .collect::<Result<Vec<_>, _>>()?;
                    module_hashes = Some(hashes);
                }
                "entry_function" => {
                    entry_function = Some(if self.eat_atom("none") {
                        None
                    } else {
                        Some(parse_symbol(&self.string()?, "entry_function", span)?)
                    });
                }
                "initializers" => initializers = Some(self.function_array("initializers")?),
                "finalizers" => finalizers = Some(self.function_array("finalizers")?),
                "gpu_capabilities" => gpu_capabilities = Some(self.atom_array()?),
                "minimum_driver_api" => {
                    minimum_driver_api = Some(if self.eat_atom("none") {
                        None
                    } else {
                        Some(parse_driver_api(&self.string()?, span)?)
                    });
                }
                _ => return Err(error(span, "unknown metadata image field")),
            }
            if self.eat_punctuation('}') {
                break;
            }
            self.expect_punctuation(',')?;
        }
        if self.index != self.tokens.len() {
            return Err(self.error("unexpected token after metadata image record"));
        }
        let missing = |field: &str| {
            error(
                self.end_span(),
                &format!("metadata image is missing {field}"),
            )
        };
        Ok(TextImageMetadata {
            package_digest: package_digest.ok_or_else(|| missing("package_digest"))?,
            tirc_build: tirc_build.ok_or_else(|| missing("tirc_build"))?,
            module_hashes: module_hashes.ok_or_else(|| missing("module_hashes"))?,
            entry_function: entry_function.ok_or_else(|| missing("entry_function"))?,
            initializers: initializers.ok_or_else(|| missing("initializers"))?,
            finalizers: finalizers.ok_or_else(|| missing("finalizers"))?,
            gpu_capabilities: gpu_capabilities.ok_or_else(|| missing("gpu_capabilities"))?,
            minimum_driver_api: minimum_driver_api.ok_or_else(|| missing("minimum_driver_api"))?,
        })
    }

    fn function_array(&mut self, label: &str) -> Result<Vec<TextImageFunction>, TextParseError> {
        self.expect_punctuation('[')?;
        let mut functions = Vec::new();
        if self.eat_punctuation(']') {
            return Ok(functions);
        }
        loop {
            self.expect_punctuation('[')?;
            let (function, span) = self.string_with_span()?;
            let function = parse_symbol(&function, label, span)?;
            self.expect_punctuation(',')?;
            let (priority, priority_span) = self.atom_with_span()?;
            let priority = priority
                .parse::<u16>()
                .map_err(|_| error(priority_span, "initializer priority must fit in u16"))?;
            self.expect_punctuation(']')?;
            functions.push(TextImageFunction { function, priority });
            if self.eat_punctuation(']') {
                break;
            }
            self.expect_punctuation(',')?;
        }
        Ok(functions)
    }

    fn string_array(&mut self) -> Result<Vec<String>, TextParseError> {
        self.expect_punctuation('[')?;
        let mut values = Vec::new();
        if self.eat_punctuation(']') {
            return Ok(values);
        }
        loop {
            values.push(self.string()?);
            if self.eat_punctuation(']') {
                break;
            }
            self.expect_punctuation(',')?;
        }
        Ok(values)
    }

    fn atom_array(&mut self) -> Result<Vec<String>, TextParseError> {
        self.expect_punctuation('[')?;
        let mut values = Vec::new();
        if self.eat_punctuation(']') {
            return Ok(values);
        }
        loop {
            values.push(self.atom()?);
            if self.eat_punctuation(']') {
                break;
            }
            self.expect_punctuation(',')?;
        }
        Ok(values)
    }

    fn string(&mut self) -> Result<String, TextParseError> {
        self.string_with_span().map(|(value, _)| value)
    }

    fn string_with_span(&mut self) -> Result<(String, Span), TextParseError> {
        let token = self.next()?;
        match token.kind {
            TokenKind::String(value) => Ok((value, token.span)),
            _ => Err(error(token.span, "expected string literal")),
        }
    }

    fn atom(&mut self) -> Result<String, TextParseError> {
        self.atom_with_span().map(|(value, _)| value)
    }

    fn atom_with_span(&mut self) -> Result<(String, Span), TextParseError> {
        let token = self.next()?;
        match token.kind {
            TokenKind::Atom(value) => Ok((value, token.span)),
            _ => Err(error(token.span, "expected identifier")),
        }
    }

    fn expect_atom(&mut self, expected: &str) -> Result<(), TextParseError> {
        let (value, span) = self.atom_with_span()?;
        if value == expected {
            Ok(())
        } else {
            Err(error(span, "unexpected identifier"))
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
        if matches!(self.tokens.get(self.index).map(|token| &token.kind), Some(TokenKind::Punctuation(value)) if *value == expected)
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
            .ok_or_else(|| self.error("unexpected end of metadata image"))?;
        self.index += 1;
        Ok(token)
    }

    fn error(&self, message: &str) -> TextParseError {
        let span = self
            .tokens
            .get(self.index)
            .map_or_else(|| self.end_span(), |token| token.span);
        error(span, message)
    }

    fn end_span(&self) -> Span {
        self.tokens.last().map_or(Span::new(0, 0), |token| {
            Span::new(token.span.end, token.span.end)
        })
    }
}

fn parse_digest(value: &str, field: &str, span: Span) -> Result<[u8; 32], TextParseError> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(error(
            span,
            &format!("{field} must be 64 lowercase hexadecimal digits"),
        ));
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).map_err(|_| {
            error(
                span,
                &format!("{field} must be 64 lowercase hexadecimal digits"),
            )
        })?;
    }
    Ok(digest)
}

fn parse_symbol(value: &str, field: &str, span: Span) -> Result<String, TextParseError> {
    if !value.starts_with('@') || value.len() == 1 || value.chars().any(char::is_whitespace) {
        return Err(error(
            span,
            &format!("{field} must name a TVISA function with an @-prefixed symbol"),
        ));
    }
    Ok(value.to_owned())
}

fn parse_driver_api(value: &str, span: Span) -> Result<(u8, u8), TextParseError> {
    let Some((year, month)) = value.split_once('.') else {
        return Err(error(span, "minimum_driver_api must use YY.M"));
    };
    let year = year
        .parse::<u8>()
        .ok()
        .filter(|year| (1..=99).contains(year))
        .ok_or_else(|| error(span, "minimum_driver_api year must be 1 through 99"))?;
    let month = month
        .parse::<u8>()
        .ok()
        .filter(|month| (1..=12).contains(month))
        .ok_or_else(|| error(span, "minimum_driver_api month must be 1 through 12"))?;
    Ok((year, month))
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}
