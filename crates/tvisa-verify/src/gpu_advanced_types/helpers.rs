use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::parse_tvisa_type;
use tir_format::text::{Token, TokenKind};
use tir_types::{AddressSpace, MatrixScope, Type};

use crate::calls::opcode_end;

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn require_pointer_space(
    instruction: &TextInstruction,
    operands: &[Type],
    index: usize,
    space: AddressSpace,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands
        .get(index)
        .is_some_and(|ty| ty != &Type::Pointer(space))
    {
        error(
            diagnostics,
            instruction,
            format!(
                "{} operand {} must have ptr<{}> type",
                instruction.opcode,
                index + 1,
                space.name()
            ),
        );
    }
}

pub(super) fn require_memory_pointer(
    instruction: &TextInstruction,
    operands: &[Type],
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands.get(index).is_some_and(|ty| {
        !matches!(
            ty,
            Type::Pointer(AddressSpace::Device | AddressSpace::Threadgroup)
        )
    }) {
        error(
            diagnostics,
            instruction,
            "cooperative memory operand must be a device or threadgroup pointer",
        );
    }
}

pub(super) fn tensor_type(ty: &Type) -> Option<(u32, &Type)> {
    if let Type::Tensor { count, element } = ty {
        Some((*count, element))
    } else {
        None
    }
}

pub(super) fn tensor_element(ty: &Type) -> Option<&Type> {
    tensor_type(ty).map(|(_, element)| element)
}

pub(super) fn cooperative_element(ty: &Type) -> Option<&Type> {
    if let Type::CooperativeMatrix { element, .. } = ty {
        Some(element)
    } else {
        None
    }
}

pub(super) fn matrix_type(ty: &Type) -> Option<(MatrixScope, u32, u32, &'static str)> {
    if let Type::CooperativeMatrix {
        scope,
        rows,
        columns,
        use_kind,
        ..
    } = ty
    {
        Some((*scope, *rows, *columns, use_kind.name()))
    } else {
        None
    }
}

pub(super) fn verify_coordinates(
    instruction: &TextInstruction,
    ty: &Type,
    rank: u32,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let valid = match ty {
        Type::Integer(32) => rank == 1,
        Type::FixedVector { lanes, element } => {
            *lanes == rank && matches!(element.as_ref(), Type::Integer(32))
        }
        _ => false,
    };
    if !valid {
        error(
            diagnostics,
            instruction,
            format!(
                "{} coordinates must contain {rank} integers",
                instruction.opcode
            ),
        );
    }
}

pub(super) fn integer_immediate_after_first_comma(instruction: &TextInstruction) -> Option<u32> {
    let start = opcode_end(instruction);
    let comma = instruction.tokens[start..]
        .iter()
        .position(|token| is_punctuation(token, ','))?
        + start;
    u32::try_from(tir_format::numeric_literals::parse_unsigned_integer(atom(
        instruction.tokens.get(comma + 1)?,
    )?)?)
    .ok()
}

pub(super) fn explicit_type_before_comma(
    source: &str,
    instruction: &TextInstruction,
) -> Option<Type> {
    let tokens = &instruction.tokens;
    let start = opcode_end(instruction);
    let end = top_level_commas(tokens, start)
        .first()
        .copied()
        .unwrap_or(tokens.len());
    type_from_tokens(source, tokens, start, end)
}

pub(super) fn type_after_atom(
    source: &str,
    instruction: &TextInstruction,
    marker: &str,
) -> Option<Type> {
    let tokens = &instruction.tokens;
    let start = tokens
        .iter()
        .enumerate()
        .find(|(index, token)| {
            atom(token) == Some(marker)
                && (*index == 0 || !is_punctuation(&tokens[*index - 1], '%'))
        })?
        .0
        + 1;
    type_from_tokens(source, tokens, start, tokens.len())
}

pub(super) fn top_level_commas(tokens: &[Token], start: usize) -> Vec<usize> {
    let mut commas = Vec::new();
    let mut depth = 0i32;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '<' | '[' | '(' => depth += 1,
                '>' | ']' | ')' => depth -= 1,
                ',' if depth == 0 => commas.push(index),
                _ => {}
            }
        }
    }
    commas
}

pub(super) fn type_from_tokens(
    source: &str,
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Option<Type> {
    let first = tokens.get(start)?;
    let last = tokens.get(end.checked_sub(1)?)?;
    parse_tvisa_type(source.get(first.span.start..last.span.end)?).ok()
}

pub(super) fn is_integer_type(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_))
}

pub(super) fn is_index_value(ty: &Type) -> bool {
    match ty {
        Type::Integer(32) => true,
        Type::FixedVector {
            lanes: 2 | 3,
            element,
        } => element.as_ref() == &Type::Integer(32),
        _ => false,
    }
}

pub(super) fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

pub(super) fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

pub(super) fn error(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(VFY3, message.into()).with_primary_span(instruction.span));
}

pub(super) fn verify_matrix_layout(
    instruction: &TextInstruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let words = words_after_opcode(instruction);
    let row_count = words
        .iter()
        .filter(|word| word.as_str() == "row_major")
        .count();
    let column_count = words
        .iter()
        .filter(|word| word.as_str() == "col_major")
        .count();
    if (row_count != 0 && column_count != 0) || row_count > 1 || column_count > 1 {
        error(
            diagnostics,
            instruction,
            "cooperative matrix memory operation has conflicting or repeated layouts",
        );
    }
}

pub(super) fn verify_commas(
    instruction: &TextInstruction,
    expected: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let actual = top_level_commas(&instruction.tokens, opcode_end(instruction)).len();
    if actual != expected {
        error(
            diagnostics,
            instruction,
            format!(
                "{} expects {expected} top-level commas, found {actual}",
                instruction.opcode
            ),
        );
    }
}

pub(super) fn has_query_field(instruction: &TextInstruction, field: &str) -> bool {
    has_modifier(instruction, field)
}

pub(super) fn tensor_query_field(instruction: &TextInstruction) -> Option<&str> {
    let start = opcode_end(instruction);
    let comma = instruction.tokens[start..]
        .iter()
        .position(|token| is_punctuation(token, ','))?
        + start;
    let mut index = comma + 2;
    if instruction
        .tokens
        .get(comma + 1)
        .is_some_and(|token| is_punctuation(token, '-'))
    {
        index += 1;
    }
    instruction.tokens.get(index).and_then(atom)
}

pub(super) fn has_modifier(instruction: &TextInstruction, name: &str) -> bool {
    instruction.tokens.iter().enumerate().any(|(index, token)| {
        (atom(token) == Some(name)
            || name.split_once('.').is_some_and(|(base, suffix)| {
                atom(token) == Some(base)
                    && instruction
                        .tokens
                        .get(index + 1)
                        .is_some_and(|token| is_punctuation(token, '.'))
                    && instruction.tokens.get(index + 2).and_then(atom) == Some(suffix)
            }))
            && (index == 0 || !is_punctuation(&instruction.tokens[index - 1], '%'))
    })
}

pub(super) fn is_float_type(ty: &Type) -> bool {
    matches!(ty, Type::Float(_))
}

pub(super) fn words_after_opcode(instruction: &TextInstruction) -> Vec<String> {
    words_in_range(
        instruction,
        opcode_end(instruction),
        instruction.tokens.len(),
        false,
    )
}

pub(super) fn words_for_value_operand(
    instruction: &TextInstruction,
    operand_index: usize,
    has_type_prefix: bool,
    allow_address_offset: bool,
) -> Option<Vec<String>> {
    let opcode_start = opcode_end(instruction);
    let commas = top_level_commas(&instruction.tokens, opcode_start);
    let (start, end) = if has_type_prefix {
        (
            commas.get(operand_index).copied()?.checked_add(1)?,
            commas
                .get(operand_index + 1)
                .copied()
                .unwrap_or(instruction.tokens.len()),
        )
    } else {
        (
            if operand_index == 0 {
                opcode_start
            } else {
                commas.get(operand_index - 1).copied()?.checked_add(1)?
            },
            commas
                .get(operand_index)
                .copied()
                .unwrap_or(instruction.tokens.len()),
        )
    };
    Some(words_in_range(
        instruction,
        start,
        end,
        allow_address_offset,
    ))
}

pub(super) fn verify_words(
    instruction: &TextInstruction,
    words: &[String],
    allowed: &[&str],
    context: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if words.iter().any(|word| !allowed.contains(&word.as_str())) {
        error(
            diagnostics,
            instruction,
            format!("{context} contains an unsupported field or modifier"),
        );
    }
}

fn words_in_range(
    instruction: &TextInstruction,
    mut index: usize,
    end: usize,
    allow_address_offset: bool,
) -> Vec<String> {
    let mut words = Vec::new();
    while index < end {
        let token = &instruction.tokens[index];
        if is_punctuation(token, '%') {
            index += 2;
            continue;
        }
        if let Some(word) = atom(token) {
            if allow_address_offset
                && is_address_offset_literal(&instruction.tokens, index)
                && tir_format::numeric_literals::parse_unsigned_integer(word).is_some()
            {
                index += 1;
                continue;
            }
            if index + 2 < end
                && is_punctuation(&instruction.tokens[index + 1], '.')
                && let Some(suffix) = instruction.tokens.get(index + 2).and_then(atom)
            {
                words.push(format!("{word}.{suffix}"));
                index += 3;
                continue;
            }
            words.push(word.to_owned());
        }
        index += 1;
    }
    words
}

fn is_address_offset_literal(tokens: &[Token], index: usize) -> bool {
    (index > 0 && is_punctuation(&tokens[index - 1], '+'))
        || (index >= 2
            && is_punctuation(&tokens[index - 1], '-')
            && is_punctuation(&tokens[index - 2], '+'))
}
