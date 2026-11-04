use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::text::{ByteStringEscapeError, Token, TokenKind, decode_kira_byte_string};
use tir_types::{FloatType, Type};

use crate::calls::opcode_end;

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify(source: &str, instruction: &TextInstruction) -> Vec<Diagnostic> {
    let opcode = instruction.opcode.as_str();
    let mut diagnostics = Vec::new();
    let ty = crate::instruction_types::explicit_type(source, instruction);
    match opcode {
        "const.bool" => {
            if !matches!(
                atom_at(instruction, opcode_end(instruction)),
                Some("true" | "false")
            ) {
                push(
                    &mut diagnostics,
                    instruction,
                    "const.bool requires true or false",
                );
            }
        }
        "const.int" => match ty {
            Some(Type::Integer(bits)) if integer_literal_fits(source, instruction, bits) => {}
            Some(Type::Integer(bits)) => push(
                &mut diagnostics,
                instruction,
                format!("const.int literal is outside the range of i{bits}"),
            ),
            _ => push(
                &mut diagnostics,
                instruction,
                "const.int requires an integer type",
            ),
        },
        "const.float" => match ty {
            Some(Type::Float(float_type)) => {
                if !float_literal_is_valid(source, instruction, float_type) {
                    push(
                        &mut diagnostics,
                        instruction,
                        format!("const.float literal is not exactly representable as {float_type}"),
                    );
                }
            }
            _ => push(
                &mut diagnostics,
                instruction,
                "const.float requires a float type",
            ),
        },
        "const.vec" => match ty {
            Some(Type::FixedVector { lanes, .. }) => {
                if vector_literal_count(instruction) != Some(lanes as usize) {
                    push(
                        &mut diagnostics,
                        instruction,
                        "const.vec literal count must match its vector lane count",
                    );
                }
            }
            _ => push(
                &mut diagnostics,
                instruction,
                "const.vec requires a fixed vector type",
            ),
        },
        "const.bytes" => match ty {
            Some(vector @ Type::FixedVector { .. }) => match byte_string(instruction) {
                Ok(Some(bytes)) => match tir_layout::LayoutEngine::new().layout(&vector) {
                    Ok(layout) if layout.size != bytes.len() as u64 => push(
                        &mut diagnostics,
                        instruction,
                        format!(
                            "const.bytes contains {} bytes; vector layout requires {}",
                            bytes.len(),
                            layout.size
                        ),
                    ),
                    Ok(_) => {}
                    Err(error) => push(
                        &mut diagnostics,
                        instruction,
                        format!("const.bytes vector type has no computable layout: {error}"),
                    ),
                },
                Ok(None) => push(
                    &mut diagnostics,
                    instruction,
                    "const.bytes requires a b-prefixed string literal",
                ),
                Err(error) => push(
                    &mut diagnostics,
                    instruction,
                    format!("const.bytes has an invalid byte-string escape: {error}"),
                ),
            },
            _ => {
                push(
                    &mut diagnostics,
                    instruction,
                    "const.bytes requires a fixed vector type in TVISA",
                );
            }
        },
        "const.null" => {
            if atom_at(instruction, opcode_end(instruction))
                .is_some_and(|space| !is_address_space(space))
            {
                push(
                    &mut diagnostics,
                    instruction,
                    "const.null has an unknown pointer address space",
                );
            }
        }
        "const.sampler" => diagnostics.extend(crate::sampler::verify(source, instruction)),
        _ => {}
    }
    diagnostics
}

fn integer_literal_fits(source: &str, instruction: &TextInstruction, bits: u16) -> bool {
    literal_text_after_comma(source, instruction)
        .is_some_and(|literal| tir_format::numeric_literals::integer_literal_fits(literal, bits))
}

fn float_literal_is_valid(
    source: &str,
    instruction: &TextInstruction,
    float_type: FloatType,
) -> bool {
    let Some(literal) = literal_text_after_comma(source, instruction) else {
        return false;
    };
    tir_format::numeric_literals::float_literal_is_exactly_representable(literal, float_type)
}

fn literal_text_after_comma<'a>(source: &'a str, instruction: &TextInstruction) -> Option<&'a str> {
    let tokens = &instruction.tokens;
    let comma = tokens
        .iter()
        .enumerate()
        .skip(opcode_end(instruction))
        .find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation(',')).then_some(index)
        })?;
    let first = tokens.get(comma + 1)?;
    let last = tokens
        .iter()
        .rev()
        .find(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))?;
    source.get(first.span.start..last.span.end).map(str::trim)
}

fn vector_literal_count(instruction: &TextInstruction) -> Option<usize> {
    let tokens = &instruction.tokens;
    let comma = tokens
        .iter()
        .enumerate()
        .skip(opcode_end(instruction))
        .find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation(',')).then_some(index)
        })?;
    let open = tokens
        .iter()
        .enumerate()
        .skip(comma + 1)
        .find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation('[')).then_some(index)
        })?;
    let mut depth = 1i32;
    let mut commas = 0usize;
    let mut has_value = false;
    let mut ended_with_comma = false;
    for token in &tokens[open + 1..] {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return if !has_value && commas == 0 {
                            Some(0)
                        } else if ended_with_comma {
                            None
                        } else {
                            Some(commas + 1)
                        };
                    }
                }
                ',' if depth == 1 => {
                    if !has_value {
                        return None;
                    }
                    commas += 1;
                    has_value = false;
                    ended_with_comma = true;
                }
                _ => {
                    has_value = true;
                    ended_with_comma = false;
                }
            }
        } else {
            has_value = true;
            ended_with_comma = false;
        }
    }
    None
}

fn byte_string(instruction: &TextInstruction) -> Result<Option<Vec<u8>>, ByteStringEscapeError> {
    let tokens = &instruction.tokens;
    let Some(comma) = tokens
        .iter()
        .enumerate()
        .skip(opcode_end(instruction))
        .find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation(',')).then_some(index)
        })
    else {
        return Ok(None);
    };
    if !matches!(tokens.get(comma + 1).map(|token| &token.kind), Some(TokenKind::Atom(prefix)) if prefix == "b")
    {
        return Ok(None);
    }
    let Some(token) = tokens.get(comma + 2) else {
        return Ok(None);
    };
    let TokenKind::String(raw) = &token.kind else {
        return Ok(None);
    };
    decode_kira_byte_string(raw.as_str()).map(Some)
}

fn atom_at(instruction: &TextInstruction, index: usize) -> Option<&str> {
    instruction.tokens.get(index).and_then(atom)
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn is_address_space(space: &str) -> bool {
    matches!(
        space,
        "generic"
            | "device"
            | "constant"
            | "threadgroup"
            | "thread"
            | "object_payload"
            | "ray_payload"
            | "hit_attribute"
            | "tile"
            | "node_record"
    )
}

fn push(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(VFY3, message.into()).with_primary_span(instruction.span));
}
