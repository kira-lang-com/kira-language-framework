use tir_diag::{Diagnostic, Span};
use tir_format::text::{Token, TokenKind, decode_kira_string_literal_contents, lex};

use crate::symbols::read_symbol_refs;

use super::{DataKind, DataObject, Relocation, RelocationKind, VFY9};

pub(super) fn parse_data(source: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<DataObject> {
    let tokens = significant_tokens(source)?;
    if atom(tokens.first()?)? != "data" {
        return None;
    }
    let references: Vec<_> = tokens.iter().collect();
    let Some(symbol_index) = tokens.iter().position(|token| punctuation(token, '@')) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "data record is missing its symbol")
                .with_primary_span(tokens[0].span),
        );
        return None;
    };
    let Some((_, after_symbol, _)) = read_symbol_refs(&references, symbol_index) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "data record has an invalid symbol")
                .with_primary_span(tokens[symbol_index].span),
        );
        return None;
    };
    if tokens
        .get(after_symbol)
        .is_some_and(|token| atom(token) == Some("import"))
    {
        diagnostics.push(
            Diagnostic::error(VFY9, "data imports must use an import data record")
                .with_primary_span(tokens[after_symbol].span),
        );
        return None;
    }
    let kind_index = if tokens
        .get(after_symbol)
        .is_some_and(|token| matches!(atom(token), Some("local" | "image" | "exported")))
    {
        after_symbol + 1
    } else {
        after_symbol
    };
    let Some(kind_token) = tokens.get(kind_index) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "data record is missing its data kind")
                .with_primary_span(tokens.last().map_or(tokens[0].span, |token| token.span)),
        );
        return None;
    };
    let Some(kind_name) = atom(kind_token) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "data kind must be an identifier")
                .with_primary_span(kind_token.span),
        );
        return None;
    };
    let kind_name = kind_name.to_owned();
    let kind = DataKind::parse(&kind_name);
    if kind == DataKind::Unknown {
        diagnostics.push(
            Diagnostic::error(VFY9, format!("unknown data kind '{kind_name}'"))
                .with_primary_span(kind_token.span),
        );
        return None;
    }
    let per_cpu = tokens
        .get(kind_index + 1)
        .is_some_and(|token| atom(token) == Some("percpu"));

    let Some(align) = tokens.iter().position(|token| atom(token) == Some("align")) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "data object is missing its alignment")
                .with_primary_span(tokens[0].span),
        );
        return None;
    };
    let Some(align_token) = tokens.get(align + 1) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "data alignment is missing its value")
                .with_primary_span(tokens[align].span),
        );
        return None;
    };
    let Some(alignment) = number(align_token) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "data alignment must be a positive power of two")
                .with_primary_span(align_token.span),
        );
        return None;
    };
    if alignment == 0 || !alignment.is_power_of_two() {
        diagnostics.push(
            Diagnostic::error(VFY9, "data alignment must be a positive power of two")
                .with_primary_span(align_token.span),
        );
        return None;
    }
    let initializer = align + 2;
    let (size, zero_initialized, cursor) = parse_initializer(&tokens, initializer, diagnostics)?;
    if kind.is_zero_initialized() != zero_initialized {
        diagnostics.push(
            Diagnostic::error(
                VFY9,
                if kind.is_zero_initialized() {
                    "zero-initialized data kinds require a zero initializer"
                } else {
                    "zero initializers require a zero-initialized data kind"
                },
            )
            .with_primary_span(tokens[initializer].span),
        );
    }
    let relocations = parse_relocations(&tokens, cursor, diagnostics);
    Some(DataObject {
        kind,
        per_cpu,
        size,
        relocations,
    })
}

fn parse_initializer(
    tokens: &[Token],
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(Option<u64>, bool, usize)> {
    let Some(initializer) = tokens.get(index) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "data object is missing its initializer")
                .with_primary_span(tokens.last().map_or(Span::new(0, 0), |token| token.span)),
        );
        return None;
    };
    let Some(kind) = atom(initializer) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "data initializer must begin with bytes, zero, or '{'")
                .with_primary_span(initializer.span),
        );
        return None;
    };
    match kind {
        "bytes" => {
            let Some(token) = tokens.get(index + 1) else {
                diagnostics.push(
                    Diagnostic::error(VFY9, "bytes initializer is missing its string")
                        .with_primary_span(initializer.span),
                );
                return None;
            };
            let TokenKind::String(raw) = &token.kind else {
                diagnostics.push(
                    Diagnostic::error(VFY9, "bytes initializer requires a string")
                        .with_primary_span(token.span),
                );
                return None;
            };
            let decoded = match decode_kira_string_literal_contents(raw) {
                Ok(decoded) => decoded,
                Err(error) => {
                    diagnostics.push(
                        Diagnostic::error(VFY9, format!("invalid string escape: {error}"))
                            .with_primary_span(token.span),
                    );
                    return None;
                }
            };
            let size = decoded.len() as u64;
            Some((Some(size), false, index + 2))
        }
        "zero" => {
            let Some(token) = tokens.get(index + 1) else {
                diagnostics.push(
                    Diagnostic::error(VFY9, "zero initializer is missing its byte count")
                        .with_primary_span(initializer.span),
                );
                return None;
            };
            let Some(size) = number(token) else {
                diagnostics.push(
                    Diagnostic::error(VFY9, "zero initializer requires a byte count")
                        .with_primary_span(token.span),
                );
                return None;
            };
            Some((Some(size), true, index + 2))
        }
        _ if punctuation(initializer, '{') => parse_aggregate(tokens, index, diagnostics),
        _ => {
            diagnostics.push(
                Diagnostic::error(
                    VFY9,
                    "data object requires a bytes, zero, or aggregate initializer",
                )
                .with_primary_span(initializer.span),
            );
            None
        }
    }
}

fn parse_aggregate(
    tokens: &[Token],
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(Option<u64>, bool, usize)> {
    let Some(size_keyword) = tokens.get(index + 1) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "aggregate initializer is missing its size")
                .with_primary_span(tokens[index].span),
        );
        return None;
    };
    if atom(size_keyword) != Some("size") {
        diagnostics.push(
            Diagnostic::error(VFY9, "aggregate initializer must begin with its size")
                .with_primary_span(size_keyword.span),
        );
        return None;
    }
    let Some(size_token) = tokens.get(index + 2) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "aggregate size is missing its value")
                .with_primary_span(size_keyword.span),
        );
        return None;
    };
    let Some(size) = number(size_token) else {
        diagnostics.push(
            Diagnostic::error(VFY9, "aggregate size must be a nonnegative integer")
                .with_primary_span(size_token.span),
        );
        return None;
    };

    let mut cursor = index + 3;
    let mut previous_end = 0;
    while let Some(token) = tokens.get(cursor) {
        if punctuation(token, '}') {
            return Some((Some(size), false, cursor + 1));
        }
        let Some(offset) = number(token) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "aggregate data element requires a byte offset")
                    .with_primary_span(token.span),
            );
            return None;
        };
        let Some(colon) = tokens.get(cursor + 1) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "aggregate data element is missing ':'")
                    .with_primary_span(token.span),
            );
            return None;
        };
        if !punctuation(colon, ':') {
            diagnostics.push(
                Diagnostic::error(VFY9, "aggregate data element is missing ':'")
                    .with_primary_span(colon.span),
            );
            return None;
        }
        let Some(hex_keyword) = tokens.get(cursor + 2) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "aggregate data element requires 'hex'")
                    .with_primary_span(colon.span),
            );
            return None;
        };
        if atom(hex_keyword) != Some("hex") {
            diagnostics.push(
                Diagnostic::error(VFY9, "aggregate data element requires 'hex'")
                    .with_primary_span(hex_keyword.span),
            );
            return None;
        }
        let Some(bytes) = tokens.get(cursor + 3) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "aggregate data element is missing its hex string")
                    .with_primary_span(hex_keyword.span),
            );
            return None;
        };
        let TokenKind::String(hex) = &bytes.kind else {
            diagnostics.push(
                Diagnostic::error(VFY9, "aggregate data element requires a quoted hex string")
                    .with_primary_span(bytes.span),
            );
            return None;
        };
        if hex.is_empty() || hex.len() % 2 != 0 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            diagnostics.push(
                Diagnostic::error(
                    VFY9,
                    "aggregate hex string must contain nonempty byte pairs",
                )
                .with_primary_span(bytes.span),
            );
            return None;
        }
        let byte_count = (hex.len() / 2) as u64;
        let Some(end) = offset.checked_add(byte_count) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "aggregate data element range overflows")
                    .with_primary_span(token.span),
            );
            return None;
        };
        if offset < previous_end {
            diagnostics.push(
                Diagnostic::error(
                    VFY9,
                    "aggregate data elements must be ordered and must not overlap",
                )
                .with_primary_span(token.span),
            );
        }
        if end > size {
            diagnostics.push(
                Diagnostic::error(
                    VFY9,
                    "aggregate data element extends beyond its declared size",
                )
                .with_primary_span(token.span),
            );
        }
        previous_end = previous_end.max(end);
        cursor += 4;
    }
    diagnostics.push(
        Diagnostic::error(VFY9, "unterminated data initializer")
            .with_primary_span(tokens[index].span),
    );
    None
}

fn parse_relocations(
    tokens: &[Token],
    mut cursor: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Relocation> {
    let mut relocations = Vec::new();
    let references: Vec<_> = tokens.iter().collect();
    while cursor < tokens.len() {
        let token = &tokens[cursor];
        if atom(token) != Some("reloc") {
            diagnostics.push(
                Diagnostic::error(VFY9, "unexpected token after data initializer")
                    .with_primary_span(token.span),
            );
            break;
        }
        let start = cursor;
        cursor += 1;
        let Some(offset_token) = tokens.get(cursor) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "relocation is missing its offset")
                    .with_primary_span(token.span),
            );
            break;
        };
        let Some(offset) = number(offset_token) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "relocation offset must be a nonnegative integer")
                    .with_primary_span(offset_token.span),
            );
            break;
        };
        cursor += 1;
        let Some(kind_token) = tokens.get(cursor) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "relocation is missing its kind")
                    .with_primary_span(offset_token.span),
            );
            break;
        };
        let Some(kind) = atom(kind_token).and_then(RelocationKind::parse) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "unknown relocation kind")
                    .with_primary_span(kind_token.span),
            );
            break;
        };
        cursor += 1;
        let Some((target, next, target_span)) = read_symbol_refs(&references, cursor) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "relocation is missing a valid symbol").with_primary_span(
                    tokens
                        .get(cursor)
                        .map_or(kind_token.span, |token| token.span),
                ),
            );
            break;
        };
        cursor = next;
        if tokens
            .get(cursor)
            .is_some_and(|token| punctuation(token, '+') || punctuation(token, '-'))
        {
            let negative = punctuation(&tokens[cursor], '-');
            if let Some(magnitude) = tokens.get(cursor + 1).and_then(number) {
                let addend = match (negative, magnitude) {
                    (false, value) => i64::try_from(value).ok(),
                    (true, value) if value == (1u64 << 63) => Some(i64::MIN),
                    (true, value) => i64::try_from(value).ok().map(i64::wrapping_neg),
                };
                if addend.is_none() {
                    diagnostics.push(
                        Diagnostic::error(
                            VFY9,
                            "relocation addend is outside the signed 64-bit range",
                        )
                        .with_primary_span(tokens[cursor + 1].span),
                    );
                }
            } else {
                diagnostics.push(
                    Diagnostic::error(VFY9, "relocation addend must be a signed integer")
                        .with_primary_span(tokens[cursor].span),
                );
                break;
            }
            cursor += 2;
        }
        let mut relocation_tag = None;
        if tokens.get(cursor).and_then(|token| atom(token)) == Some("tag") {
            let Some(tag) = tokens.get(cursor + 1).and_then(hex_u32) else {
                diagnostics.push(
                    Diagnostic::error(VFY9, "relocation tag must be a 32-bit hexadecimal value")
                        .with_primary_span(tokens[cursor].span),
                );
                break;
            };
            relocation_tag = Some(tag);
            cursor += 2;
        }
        if cursor < tokens.len() && atom(&tokens[cursor]) != Some("reloc") {
            diagnostics.push(
                Diagnostic::error(VFY9, "unexpected token after relocation")
                    .with_primary_span(tokens[cursor].span),
            );
            break;
        }
        let end = tokens
            .get(cursor.saturating_sub(1))
            .map_or(token.span.end, |token| token.span.end);
        relocations.push(Relocation {
            kind,
            offset,
            target,
            tag: relocation_tag,
            target_span,
            span: Span::new(tokens[start].span.start, end),
        });
    }
    relocations
}

fn significant_tokens(source: &str) -> Option<Vec<Token>> {
    Some(
        lex(source)
            .ok()?
            .into_iter()
            .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
            .collect(),
    )
}

pub(super) fn function_address_info(source: &str) -> (bool, Option<u32>) {
    let Some(tokens) = significant_tokens(source) else {
        return (false, None);
    };
    let header_end = tokens
        .iter()
        .position(|token| punctuation(token, '{'))
        .unwrap_or(tokens.len());
    let header = &tokens[..header_end];
    let address_taken = header
        .iter()
        .any(|token| atom(token) == Some("address_taken"));
    let tag = header.windows(2).find_map(|pair| {
        (atom(&pair[0]) == Some("tag"))
            .then(|| hex_u32(&pair[1]))
            .flatten()
    });
    (address_taken, tag)
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn number(token: &Token) -> Option<u64> {
    u64::try_from(tir_format::numeric_literals::parse_unsigned_integer(atom(
        token,
    )?)?)
    .ok()
}

fn hex_u32(token: &Token) -> Option<u32> {
    let value = atom(token)?
        .strip_prefix("0x")
        .or_else(|| atom(token)?.strip_prefix("0X"))?;
    u32::from_str_radix(value, 16).ok()
}
