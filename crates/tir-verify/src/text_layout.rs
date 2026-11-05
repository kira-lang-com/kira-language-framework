use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{TextItemKind, TextModule, parse_type};
use tir_layout::{
    DefinitionKind, EnumVariant, LayoutEngine, LayoutField, LayoutRule, NominalDefinition,
};
use tir_types::Type;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn from_module(module: &TextModule) -> (LayoutEngine, Vec<Diagnostic>) {
    let mut engine = LayoutEngine::new();
    let mut diagnostics = Vec::new();
    for item in module
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Type)
    {
        let source = module.item_text(*item);
        match parse_definition(source) {
            Ok(definition) => {
                if let Err(error) = engine.add_definition(definition) {
                    diagnostics.push(
                        Diagnostic::error(V2, error.to_string()).with_primary_span(item.span),
                    );
                }
            }
            Err((span, message)) => diagnostics.push(
                Diagnostic::error(V2, message).with_primary_span(shift(span, item.span.start)),
            ),
        }
    }
    for item in module
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Import)
    {
        let source = module.item_text(*item);
        let Ok(tokens) = lex(source) else {
            continue;
        };
        for index in 0..tokens.len() {
            if atom_at(&tokens, index) != Some("type") {
                continue;
            }
            let Some(symbol_start) = (index + 1..tokens.len()).find(|cursor| {
                punctuation(&tokens[*cursor], '@')
                    || matches!(tokens[*cursor].kind, TokenKind::Punctuation('}' | '\n'))
            }) else {
                continue;
            };
            let Some((_, symbol_end, _)) = crate::variant_text::read_symbol(&tokens, symbol_start)
            else {
                continue;
            };
            let Some(colon) = (symbol_end..tokens.len()).find(|cursor| {
                punctuation(&tokens[*cursor], ':')
                    || matches!(tokens[*cursor].kind, TokenKind::Punctuation(',' | '}'))
            }) else {
                continue;
            };
            if !punctuation(&tokens[colon], ':') {
                continue;
            }
            let end = crate::signature_types::type_item_end(&tokens, colon + 1);
            let Some(first) = tokens.get(colon + 1) else {
                continue;
            };
            let Some(last) = tokens.get(end.saturating_sub(1)) else {
                continue;
            };
            let Some(text) = source.get(first.span.start..last.span.end) else {
                continue;
            };
            if let Ok(Type::Nominal { path, .. }) = parse_type(text) {
                engine.register_imported_nominal(path);
            }
        }
    }
    (engine, diagnostics)
}

fn parse_definition(source: &str) -> Result<NominalDefinition, (Span, String)> {
    let tokens = lex(source).map_err(|error| (error.span, error.message.to_owned()))?;
    let name_start = tokens
        .iter()
        .position(|token| punctuation(token, '%'))
        .map(|index| index + 1)
        .ok_or_else(|| parse_error(&tokens, 0, "type declaration has no name"))?;
    let mut name_end = name_start;
    while name_end < tokens.len()
        && (matches!(tokens[name_end].kind, TokenKind::Atom(_))
            || punctuation(&tokens[name_end], '.'))
    {
        name_end += 1;
    }
    let name = source
        .get(
            tokens.get(name_start).map_or(0, |token| token.span.start)
                ..tokens
                    .get(name_end.saturating_sub(1))
                    .map_or(0, |token| token.span.end),
        )
        .ok_or_else(|| parse_error(&tokens, name_start, "invalid type name"))?
        .to_owned();
    let parameters = generic_parameters(&tokens, name_end);
    let equals = tokens
        .iter()
        .enumerate()
        .skip(name_end)
        .find_map(|(index, token)| punctuation(token, '=').then_some(index))
        .ok_or_else(|| parse_error(&tokens, name_end, "type declaration is missing '='"))?;
    let kind_index = equals + 1;
    let kind_name = atom_at(&tokens, kind_index)
        .ok_or_else(|| parse_error(&tokens, kind_index, "type declaration has no kind"))?;
    let kind = match kind_name {
        "struct" => DefinitionKind::Struct,
        "enum" => DefinitionKind::Enum,
        "explicit" => DefinitionKind::Union,
        _ => {
            return Err(parse_error(
                &tokens,
                kind_index,
                "unknown nominal type kind",
            ));
        }
    };
    let (rule, explicit_size, explicit_align, rule_end) =
        parse_layout_rule(&tokens, kind_index + 1)?;
    let body_start = tokens
        .iter()
        .enumerate()
        .skip(rule_end)
        .find_map(|(index, token)| punctuation(token, '{').then_some(index))
        .ok_or_else(|| parse_error(&tokens, rule_end, "type declaration has no body"))?;
    let body_end = matching(tokens.as_slice(), body_start, '{', '}')
        .ok_or_else(|| parse_error(&tokens, body_start, "unterminated type body"))?;
    let members = split_ranges(&tokens, body_start + 1, body_end);
    let (fields, variants) = if kind == DefinitionKind::Enum {
        (Vec::new(), parse_variants(source, &tokens, &members)?)
    } else {
        (parse_fields(source, &tokens, &members)?, Vec::new())
    };
    Ok(NominalDefinition {
        path: name,
        parameters,
        opaque: false,
        rule,
        kind,
        fields,
        variants,
        tag_type: (kind == DefinitionKind::Enum && rule == LayoutRule::C)
            .then_some(Type::Integer(32)),
        explicit_size,
        explicit_align,
    })
}

fn parse_layout_rule(
    tokens: &[Token],
    index: usize,
) -> Result<(LayoutRule, Option<u64>, Option<u64>, usize), (Span, String)> {
    match atom_at(tokens, index) {
        Some("ridl") => Ok((LayoutRule::Ridl, None, None, index + 1)),
        Some("c") => Ok((LayoutRule::C, None, None, index + 1)),
        Some("packed") => {
            let value = atom_at(tokens, index + 2)
                .and_then(parse_integer)
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(|| parse_error(tokens, index, "packed layout requires an alignment"))?;
            Ok((LayoutRule::Packed(value), None, None, index + 4))
        }
        Some("explicit") => {
            let size = field_integer(tokens, index + 3, "size")
                .ok_or_else(|| parse_error(tokens, index, "explicit layout requires a size"))?;
            let align = field_integer(tokens, index + 6, "align").ok_or_else(|| {
                parse_error(tokens, index, "explicit layout requires an alignment")
            })?;
            Ok((LayoutRule::Explicit, Some(size), Some(align), index + 8))
        }
        _ => Err(parse_error(tokens, index, "unknown type layout rule")),
    }
}

fn parse_fields(
    source: &str,
    tokens: &[Token],
    ranges: &[(usize, usize)],
) -> Result<Vec<LayoutField>, (Span, String)> {
    ranges
        .iter()
        .filter_map(|(start, end)| {
            let (start, end) = trim_range(tokens, *start, *end);
            (start < end).then_some((start, end))
        })
        .map(|(start, end)| {
            let colon = (start..end)
                .find(|index| punctuation(&tokens[*index], ':'))
                .ok_or_else(|| parse_error(tokens, start, "struct field has no type"))?;
            let name = atom_at(tokens, start)
                .ok_or_else(|| parse_error(tokens, start, "struct field has no name"))?
                .to_owned();
            let type_start = colon + 1;
            let type_end = (type_start..end)
                .find(|index| punctuation(&tokens[*index], '@'))
                .unwrap_or(end);
            let ty = parse_type_range(source, tokens, type_start, type_end)?;
            let (explicit_offset, explicit_bit_offset, bit_width) =
                field_location(tokens, type_end, end)?;
            Ok(LayoutField {
                name: Some(name),
                ty,
                explicit_offset,
                explicit_bit_offset,
                bit_width,
                flexible: (type_end..end).any(|index| atom_at(tokens, index) == Some("flexible")),
            })
        })
        .collect()
}

fn parse_variants(
    source: &str,
    tokens: &[Token],
    ranges: &[(usize, usize)],
) -> Result<Vec<EnumVariant>, (Span, String)> {
    ranges
        .iter()
        .filter_map(|(start, end)| {
            let (start, end) = trim_range(tokens, *start, *end);
            (start < end).then_some((start, end))
        })
        .map(|(start, end)| {
            let Some(open) = (start..end).find(|index| punctuation(&tokens[*index], '(')) else {
                return Ok(EnumVariant { fields: Vec::new() });
            };
            let close = matching(tokens, open, '(', ')')
                .filter(|close| *close < end)
                .ok_or_else(|| parse_error(tokens, open, "unterminated enum variant"))?;
            let fields = split_ranges(tokens, open + 1, close)
                .into_iter()
                .filter_map(|(field_start, field_end)| {
                    let (field_start, field_end) = trim_range(tokens, field_start, field_end);
                    (field_start < field_end).then_some((field_start, field_end))
                })
                .map(|(field_start, field_end)| {
                    parse_type_range(source, tokens, field_start, field_end)
                        .map(LayoutField::positional)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(EnumVariant { fields })
        })
        .collect()
}

fn parse_type_range(
    source: &str,
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Result<Type, (Span, String)> {
    let (start, end) = trim_range(tokens, start, end);
    let first = tokens
        .get(start)
        .ok_or_else(|| parse_error(tokens, start, "missing field type"))?;
    let last = tokens
        .get(end.saturating_sub(1))
        .ok_or_else(|| parse_error(tokens, start, "missing field type"))?;
    let text = source
        .get(first.span.start..last.span.end)
        .ok_or_else(|| parse_error(tokens, start, "invalid field type"))?;
    parse_type(text).map_err(|error| (error.span, error.message))
}

fn generic_parameters(tokens: &[Token], name_end: usize) -> Vec<String> {
    if !tokens
        .get(name_end)
        .is_some_and(|token| punctuation(token, '<'))
    {
        return Vec::new();
    }
    let end = matching(tokens, name_end, '<', '>').unwrap_or(tokens.len());
    split_ranges(tokens, name_end + 1, end)
        .into_iter()
        .filter_map(|(start, finish)| {
            (start..finish).find_map(|index| {
                punctuation(&tokens[index], '$')
                    .then(|| atom_at(tokens, index + 1).map(str::to_owned))
                    .flatten()
            })
        })
        .collect()
}

fn field_location(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Result<(Option<u64>, Option<u16>, Option<u16>), (Span, String)> {
    let Some(at) = (start..end).find(|index| {
        punctuation(&tokens[*index], '@')
            && atom_at(tokens, index + 1).and_then(parse_integer).is_some()
    }) else {
        return Ok((None, None, None));
    };
    let offset = atom_at(tokens, at + 1)
        .and_then(parse_integer)
        .ok_or_else(|| parse_error(tokens, at + 1, "explicit field offset must be an integer"))?;
    if at + 2 >= end || !punctuation(&tokens[at + 2], ':') {
        return Ok((Some(offset), None, None));
    }
    let bit_offset = atom_at(tokens, at + 3)
        .and_then(parse_integer)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| parse_error(tokens, at + 3, "bit offset must fit in 16 bits"))?;
    if at + 4 >= end || !punctuation(&tokens[at + 4], ':') {
        return Err(parse_error(
            tokens,
            at + 2,
            "explicit bit-fields use @byte_offset:bit_offset:width",
        ));
    }
    let width = atom_at(tokens, at + 5)
        .and_then(parse_integer)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| parse_error(tokens, at + 5, "bit width must fit in 16 bits"))?;
    Ok((Some(offset), Some(bit_offset), Some(width)))
}

fn field_integer(tokens: &[Token], index: usize, expected: &str) -> Option<u64> {
    (atom_at(tokens, index) == Some(expected))
        .then(|| atom_at(tokens, index + 1).and_then(parse_integer))
        .flatten()
}

fn parse_integer(value: &str) -> Option<u64> {
    u64::try_from(tir_format::numeric_literals::parse_unsigned_integer(value)?).ok()
}

fn split_ranges(tokens: &[Token], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut part_start = start;
    let mut stack = Vec::new();
    for index in start..end {
        match tokens[index].kind {
            TokenKind::Punctuation('(' | '[' | '{' | '<') => {
                if let TokenKind::Punctuation(character) = tokens[index].kind {
                    stack.push(character);
                }
            }
            TokenKind::Punctuation(')' | ']' | '}' | '>') => {
                if let TokenKind::Punctuation(character) = tokens[index].kind {
                    let expected = match character {
                        ')' => '(',
                        ']' => '[',
                        '}' => '{',
                        '>' => '<',
                        _ => unreachable!(),
                    };
                    if stack.last() == Some(&expected) {
                        stack.pop();
                    }
                }
            }
            TokenKind::Punctuation(',') if stack.is_empty() => {
                ranges.push((part_start, index));
                part_start = index + 1;
            }
            _ => {}
        }
    }
    ranges.push((part_start, end));
    ranges
}

fn matching(tokens: &[Token], open: usize, left: char, right: char) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if punctuation(token, left) {
            depth += 1;
        } else if punctuation(token, right) {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn trim_range(tokens: &[Token], mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end && matches!(tokens[start].kind, TokenKind::Newline | TokenKind::End) {
        start += 1;
    }
    while end > start && matches!(tokens[end - 1].kind, TokenKind::Newline | TokenKind::End) {
        end -= 1;
    }
    (start, end)
}

fn atom_at(tokens: &[Token], index: usize) -> Option<&str> {
    tokens.get(index).and_then(atom)
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn punctuation(token: &Token, character: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(found) if found == character)
}

fn parse_error(tokens: &[Token], index: usize, message: &str) -> (Span, String) {
    let span = tokens
        .get(index)
        .map_or(Span::new(0, 0), |token| token.span);
    (span, message.to_owned())
}

fn shift(span: Span, offset: usize) -> Span {
    Span::new(
        span.start.saturating_add(offset),
        span.end.saturating_add(offset),
    )
}
