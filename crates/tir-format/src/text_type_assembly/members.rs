use super::declarations::ParsedDeclaration;
use super::lower::TypeTableBuilder;
use super::policies::parse_policy;
use super::syntax::{
    atom, atom_at, find_punctuation, item_error, matching, number_at, parse_u64, punctuation,
    span_at, split_ranges, token_text, trim_range,
};
use crate::text::TokenKind;
use crate::types::{
    BinaryField, BinaryLayout, BinaryNominalDefinition, BinaryNominalKind, BinaryVariant,
};
use crate::{TextModule, TextParseError, parse_type};
use tir_diag::Span;
use tir_types::Type;

pub(super) fn parse_definition(
    module: &TextModule,
    parsed: &ParsedDeclaration,
    declaration_index: u64,
    builder: &mut TypeTableBuilder<'_>,
) -> Result<BinaryNominalDefinition, TextParseError> {
    let ranges = split_ranges(&parsed.tokens, parsed.body_start + 1, parsed.body_end);
    let mut fields = Vec::new();
    let mut variants = Vec::new();
    for (start, end) in ranges {
        let (start, end) = trim_range(&parsed.tokens, start, end);
        if start == end {
            continue;
        }
        if parsed.declaration.kind == BinaryNominalKind::Enum {
            variants.push(parse_variant(
                module,
                parsed,
                start,
                end,
                declaration_index,
                builder,
            )?);
        } else {
            fields.push(parse_field(
                module,
                parsed,
                start,
                end,
                declaration_index,
                builder,
            )?);
        }
    }
    if parsed.declaration.kind == BinaryNominalKind::Enum
        && parsed.layout == BinaryLayout::C
        && variants.iter().any(|variant| !variant.payload.is_empty())
    {
        return Err(item_error(
            parsed.item,
            Span::new(0, 0),
            "C enumerations cannot carry variant payloads",
        ));
    }
    let policy = parse_policy(parsed, builder.function_indices())?;
    Ok(BinaryNominalDefinition {
        declaration: declaration_index,
        layout: parsed.layout,
        fields,
        variants,
        policy,
    })
}

fn parse_field(
    module: &TextModule,
    parsed: &ParsedDeclaration,
    start: usize,
    end: usize,
    declaration_index: u64,
    builder: &mut TypeTableBuilder<'_>,
) -> Result<BinaryField, TextParseError> {
    let name = atom_at(&parsed.tokens, start)
        .ok_or_else(|| {
            item_error(
                parsed.item,
                span_at(&parsed.tokens, start),
                "field has no name",
            )
        })?
        .to_owned();
    let colon = find_punctuation(&parsed.tokens, start + 1, ':')
        .filter(|index| *index < end)
        .ok_or_else(|| {
            item_error(
                parsed.item,
                span_at(&parsed.tokens, start),
                "field has no type",
            )
        })?;
    let mut type_end = end;
    let mut nesting = 0_i32;
    let mut frame_symbol_seen = false;
    for index in colon + 1..end {
        if let TokenKind::Punctuation(character) = parsed.tokens[index].kind {
            match character {
                '<' | '(' | '[' | '{' => nesting += 1,
                '>' | ')' | ']' | '}' => nesting -= 1,
                '@' if nesting == 0 => {
                    // `frame @function` is part of a type; all other `@` tokens
                    // begin an offset or field annotation.
                    if atom_at(&parsed.tokens, colon + 1) == Some("frame") && !frame_symbol_seen {
                        frame_symbol_seen = true;
                        continue;
                    }
                    type_end = index;
                    break;
                }
                _ => {}
            }
        }
    }
    let ty = parse_type_at(module, parsed, colon + 1, type_end)?;
    let type_index = builder.intern_type(&ty, declaration_index, parsed.item)?;
    let mut cursor = type_end;
    let mut offset = None;
    let mut bit_offset = None;
    let mut bit_width = None;
    let mut annotations = Vec::new();
    while cursor < end {
        if !punctuation(&parsed.tokens[cursor], '@') {
            cursor += 1;
            continue;
        }
        let next = cursor + 1;
        if let Some(value) = parsed
            .tokens
            .get(next)
            .and_then(|token| atom(token))
            .and_then(parse_u64)
        {
            if offset.is_some() {
                return Err(item_error(
                    parsed.item,
                    span_at(&parsed.tokens, cursor),
                    "field has multiple byte offsets",
                ));
            }
            offset = Some(value);
            cursor = next + 1;
            if cursor < end && punctuation(&parsed.tokens[cursor], ':') {
                bit_offset = Some(number_at(&parsed.tokens, cursor + 1).ok_or_else(|| {
                    item_error(
                        parsed.item,
                        span_at(&parsed.tokens, cursor + 1),
                        "invalid bit offset",
                    )
                })?);
                if !parsed
                    .tokens
                    .get(cursor + 2)
                    .is_some_and(|token| punctuation(token, ':'))
                {
                    return Err(item_error(
                        parsed.item,
                        span_at(&parsed.tokens, cursor + 2),
                        "bitfield offset requires a width",
                    ));
                }
                bit_width = Some(number_at(&parsed.tokens, cursor + 3).ok_or_else(|| {
                    item_error(
                        parsed.item,
                        span_at(&parsed.tokens, cursor + 3),
                        "invalid bit width",
                    )
                })?);
                cursor += 4;
            }
            continue;
        }
        let annotation_name = atom_at(&parsed.tokens, next)
            .ok_or_else(|| {
                item_error(
                    parsed.item,
                    span_at(&parsed.tokens, next),
                    "invalid field annotation",
                )
            })?
            .to_owned();
        cursor = next + 1;
        let mut arguments = Vec::new();
        if cursor < end && punctuation(&parsed.tokens[cursor], '(') {
            let close = matching(&parsed.tokens, cursor, '(', ')')
                .filter(|close| *close < end)
                .ok_or_else(|| {
                    item_error(
                        parsed.item,
                        span_at(&parsed.tokens, cursor),
                        "unterminated annotation",
                    )
                })?;
            for (arg_start, arg_end) in split_ranges(&parsed.tokens, cursor + 1, close) {
                let (arg_start, arg_end) = trim_range(&parsed.tokens, arg_start, arg_end);
                if arg_start < arg_end {
                    arguments.push(
                        token_text(
                            module.item_text(parsed.item),
                            &parsed.tokens,
                            arg_start,
                            arg_end,
                        )
                        .unwrap_or_default(),
                    );
                }
            }
            cursor = close + 1;
        }
        annotations.push(crate::BinaryAnnotation {
            name: annotation_name,
            arguments,
        });
    }
    Ok(BinaryField {
        name,
        ty: type_index,
        offset,
        bit_offset,
        bit_width,
        annotations,
    })
}

fn parse_variant(
    module: &TextModule,
    parsed: &ParsedDeclaration,
    start: usize,
    end: usize,
    declaration_index: u64,
    builder: &mut TypeTableBuilder<'_>,
) -> Result<BinaryVariant, TextParseError> {
    let name = atom_at(&parsed.tokens, start)
        .ok_or_else(|| {
            item_error(
                parsed.item,
                span_at(&parsed.tokens, start),
                "enum variant has no name",
            )
        })?
        .to_owned();
    let mut payload = Vec::new();
    if let Some(open) =
        find_punctuation(&parsed.tokens, start + 1, '(').filter(|index| *index < end)
    {
        let close = matching(&parsed.tokens, open, '(', ')')
            .filter(|close| *close < end)
            .ok_or_else(|| {
                item_error(
                    parsed.item,
                    span_at(&parsed.tokens, open),
                    "unterminated enum payload",
                )
            })?;
        for (part_start, part_end) in split_ranges(&parsed.tokens, open + 1, close) {
            let (part_start, part_end) = trim_range(&parsed.tokens, part_start, part_end);
            if part_start < part_end {
                let ty = parse_type_at(module, parsed, part_start, part_end)?;
                payload.push(builder.intern_type(&ty, declaration_index, parsed.item)?);
            }
        }
    }
    Ok(BinaryVariant { name, payload })
}

fn parse_type_at(
    module: &TextModule,
    parsed: &ParsedDeclaration,
    start: usize,
    end: usize,
) -> Result<Type, TextParseError> {
    let source = module.item_text(parsed.item);
    let text = token_text(source, &parsed.tokens, start, end)
        .ok_or_else(|| item_error(parsed.item, span_at(&parsed.tokens, start), "missing type"))?;
    let offset = parsed.tokens.get(start).map_or(0, |token| token.span.start);
    parse_type(&text).map_err(|error| {
        item_error(
            parsed.item,
            Span::new(
                offset.saturating_add(error.span.start),
                offset.saturating_add(error.span.end),
            ),
            error.message,
        )
    })
}
