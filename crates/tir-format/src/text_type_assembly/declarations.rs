use super::syntax::{
    atom_at, find_punctuation, item_error, matching, number_after_key, number_at, path_end,
    punctuation, span_at, split_ranges, token_text, token_text_for_path, trim_range,
};
use crate::text::{Token, lex};
use crate::types::{
    BinaryGenericParameter, BinaryLayout, BinaryNominalDeclaration, BinaryNominalKind,
};
use crate::{TextItem, TextModule, TextParseError};
use tir_diag::Span;

pub(super) struct ParsedDeclaration {
    pub(super) item: TextItem,
    pub(super) tokens: Vec<Token>,
    pub(super) declaration: BinaryNominalDeclaration,
    pub(super) layout: BinaryLayout,
    pub(super) body_start: usize,
    pub(super) body_end: usize,
    pub(super) policy_start: usize,
}

pub(super) fn parse_declaration(
    module: &TextModule,
    item: TextItem,
) -> Result<ParsedDeclaration, TextParseError> {
    let source = module.item_text(item);
    let tokens = lex(source).map_err(|error| item_error(item, error.span, error.message))?;
    let name_start = find_punctuation(&tokens, 0, '%')
        .map(|index| index + 1)
        .ok_or_else(|| item_error(item, Span::new(0, 0), "type declaration has no name"))?;
    let mut name_end = name_start;
    while name_end < tokens.len()
        && (atom_at(&tokens, name_end).is_some() || punctuation(&tokens[name_end], '.'))
    {
        name_end += 1;
    }
    let name = token_text(source, &tokens, name_start, name_end)
        .ok_or_else(|| item_error(item, span_at(&tokens, name_start), "invalid type name"))?;
    let (generic_parameters, after_generics) = parse_generic_parameters(item, &tokens, name_end)?;
    let equals = find_punctuation(&tokens, after_generics, '=').ok_or_else(|| {
        item_error(
            item,
            span_at(&tokens, after_generics),
            "type declaration is missing '='",
        )
    })?;
    let (kind, kind_end) = match atom_at(&tokens, equals + 1) {
        Some("struct") => (BinaryNominalKind::Struct, equals + 2),
        Some("enum") => (BinaryNominalKind::Enum, equals + 2),
        Some("explicit") => (BinaryNominalKind::Explicit, equals + 2),
        _ => {
            return Err(item_error(
                item,
                span_at(&tokens, equals + 1),
                "unknown nominal type kind",
            ));
        }
    };
    let (layout, layout_end) = parse_layout(item, &tokens, kind_end)?;
    let body_start = find_punctuation(&tokens, layout_end, '{').ok_or_else(|| {
        item_error(
            item,
            span_at(&tokens, layout_end),
            "type declaration has no body",
        )
    })?;
    let body_end = matching(&tokens, body_start, '{', '}')
        .ok_or_else(|| item_error(item, span_at(&tokens, body_start), "unterminated type body"))?;
    let policy_start = (body_end + 1..tokens.len())
        .find(|index| atom_at(&tokens, *index) == Some("policy"))
        .ok_or_else(|| {
            item_error(
                item,
                span_at(&tokens, body_end),
                "type declaration is missing its policy",
            )
        })?;
    if kind == BinaryNominalKind::Enum && matches!(layout, BinaryLayout::Explicit { .. }) {
        return Err(item_error(
            item,
            span_at(&tokens, kind_end),
            "enum cannot use explicit layout",
        ));
    }
    Ok(ParsedDeclaration {
        item,
        tokens,
        declaration: BinaryNominalDeclaration {
            name,
            generic_parameters,
            kind,
        },
        layout,
        body_start,
        body_end,
        policy_start,
    })
}

fn parse_generic_parameters(
    item: TextItem,
    tokens: &[Token],
    start: usize,
) -> Result<(Vec<BinaryGenericParameter>, usize), TextParseError> {
    if !tokens
        .get(start)
        .is_some_and(|token| punctuation(token, '<'))
    {
        return Ok((Vec::new(), start));
    }
    let end = matching(tokens, start, '<', '>').ok_or_else(|| {
        item_error(
            item,
            span_at(tokens, start),
            "unterminated generic parameter list",
        )
    })?;
    let mut parameters = Vec::new();
    let mut names = std::collections::HashSet::new();
    for (part_start, part_end) in split_ranges(tokens, start + 1, end) {
        let (part_start, part_end) = trim_range(tokens, part_start, part_end);
        if part_start == part_end {
            continue;
        }
        if !punctuation(&tokens[part_start], '$') {
            return Err(item_error(
                item,
                span_at(tokens, part_start),
                "generic parameter must start with '$'",
            ));
        }
        let name = atom_at(tokens, part_start + 1)
            .ok_or_else(|| {
                item_error(
                    item,
                    span_at(tokens, part_start + 1),
                    "generic parameter has no name",
                )
            })?
            .to_owned();
        if !names.insert(name.clone()) {
            return Err(item_error(
                item,
                span_at(tokens, part_start + 1),
                "duplicate generic parameter name",
            ));
        }
        let mut bounds = Vec::new();
        if part_start + 2 < part_end {
            if !punctuation(&tokens[part_start + 2], ':') {
                return Err(item_error(
                    item,
                    span_at(tokens, part_start + 2),
                    "expected ':' before generic bounds",
                ));
            }
            let mut cursor = part_start + 3;
            while cursor < part_end {
                if !punctuation(&tokens[cursor], '%') {
                    return Err(item_error(
                        item,
                        span_at(tokens, cursor),
                        "generic bound must be a trait path",
                    ));
                }
                let path_start = cursor + 1;
                cursor = path_end(tokens, path_start);
                if cursor == path_start {
                    return Err(item_error(
                        item,
                        span_at(tokens, path_start),
                        "generic bound has no path",
                    ));
                }
                bounds.push(token_text_for_path(tokens, path_start, cursor));
                if cursor < part_end {
                    if !punctuation(&tokens[cursor], '&') {
                        return Err(item_error(
                            item,
                            span_at(tokens, cursor),
                            "expected '&' between generic bounds",
                        ));
                    }
                    cursor += 1;
                }
            }
        }
        parameters.push(BinaryGenericParameter { name, bounds });
    }
    Ok((parameters, end + 1))
}

fn parse_layout(
    item: TextItem,
    tokens: &[Token],
    start: usize,
) -> Result<(BinaryLayout, usize), TextParseError> {
    match atom_at(tokens, start) {
        Some("ridl") => Ok((BinaryLayout::Ridl, start + 1)),
        Some("c") => Ok((BinaryLayout::C, start + 1)),
        Some("packed") => {
            let value = number_at(tokens, start + 2)
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(|| {
                    item_error(
                        item,
                        span_at(tokens, start + 2),
                        "packed layout requires an alignment",
                    )
                })?;
            if !matches!(value, 1 | 2 | 4 | 8) {
                return Err(item_error(
                    item,
                    span_at(tokens, start + 2),
                    "packed alignment must be 1, 2, 4, or 8",
                ));
            }
            Ok((BinaryLayout::Packed(u64::from(value)), start + 4))
        }
        Some("explicit") => {
            let size = number_after_key(tokens, start + 2, "size").ok_or_else(|| {
                item_error(
                    item,
                    span_at(tokens, start + 2),
                    "explicit layout requires a size",
                )
            })?;
            let align = number_after_key(tokens, start + 5, "align").ok_or_else(|| {
                item_error(
                    item,
                    span_at(tokens, start + 5),
                    "explicit layout requires an alignment",
                )
            })?;
            if align == 0 {
                return Err(item_error(
                    item,
                    span_at(tokens, start + 5),
                    "explicit alignment must be nonzero",
                ));
            }
            Ok((BinaryLayout::Explicit { size, align }, start + 8))
        }
        _ => Err(item_error(
            item,
            span_at(tokens, start),
            "unknown type layout rule",
        )),
    }
}
