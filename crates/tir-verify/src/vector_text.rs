use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind};
use tir_format::{TextInstruction, parse_type};
use tir_types::{FloatType, Type};

use crate::model::ValueUse;
use crate::ssa::text_instruction_operands_start;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

mod memory;
mod operations;

pub(super) fn infer_result_types(
    source: &str,
    instruction: &TextInstruction,
    operands: &[ValueUse],
) -> Option<Vec<Type>> {
    let opcode = instruction.opcode.as_str();
    let first = || operands.first().map(|operand| operand.ty.clone());
    let result = match opcode {
        "vec.splat" => type_after_keyword(source, instruction, "to"),
        "vec.step" | "vec.load" | "vec.gather" => explicit_type(source, instruction),
        "vec.extract" | "vec.reduce" => first().and_then(|ty| vector_element(&ty).cloned()),
        "vec.insert" | "vec.compress" | "vec.expand" | "vec.dot" | "vec.zip" | "vec.unzip"
        | "vec.reverse" | "vec.slide" => first(),
        "vec.shuffle" => {
            let lanes = shuffle_indices(instruction)?.len();
            let element = vector_element(&first()?)?.clone();
            u32::try_from(lanes)
                .ok()
                .filter(|lanes| valid_lanes(*lanes))
                .map(|lanes| Type::FixedVector {
                    lanes,
                    element: Box::new(element),
                })
        }
        "vec.vscale" | "vec.mask.count" | "vec.mask.first" => Some(Type::Integer(64)),
        "vec.mask.active" => Some(Type::ScalableVector(Box::new(Type::Integer(1)))),
        "vec.concat" => operands
            .first()
            .zip(operands.get(1))
            .and_then(|(left, right)| concatenate_type(&left.ty, &right.ty)),
        "vec.split" => first().and_then(|ty| split_type(&ty)),
        "vec.table" => operands
            .get(1)
            .and_then(|operand| table_result_type(&operand.ty)),
        "vec.store" | "vec.scatter" => return Some(Vec::new()),
        _ => return None,
    };
    result.map(|ty| vec![ty])
}

pub(super) fn verify(
    source: &str,
    instruction: &TextInstruction,
    operands: &[ValueUse],
    inferred: Option<&[Type]>,
    result_count: usize,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let types = operands
        .iter()
        .map(|operand| &operand.ty)
        .collect::<Vec<_>>();
    let opcode = instruction.opcode.as_str();
    let expected_results = if matches!(opcode, "vec.store" | "vec.scatter") {
        0
    } else {
        1
    };
    if result_count != expected_results {
        report(
            &mut diagnostics,
            instruction,
            format!("{opcode} defines {result_count} results; expected {expected_results}"),
        );
    }
    if let Some(inferred) = inferred
        && inferred.len() != expected_results
    {
        report(
            &mut diagnostics,
            instruction,
            format!("{opcode} has an invalid inferred result shape"),
        );
    }

    let valid = match opcode {
        "vec.splat" => operations::verify_splat(source, instruction, &types, &mut diagnostics),
        "vec.extract" => operations::verify_extract(instruction, &types, &mut diagnostics),
        "vec.insert" => operations::verify_insert(instruction, &types, &mut diagnostics),
        "vec.shuffle" => operations::verify_shuffle(instruction, &types, &mut diagnostics),
        "vec.step" => operations::verify_step(source, instruction, &mut diagnostics),
        "vec.vscale" => types.is_empty(),
        "vec.reduce" => operations::verify_reduce(instruction, &types, &mut diagnostics),
        "vec.load" => memory::verify_load(source, instruction, &types, &mut diagnostics),
        "vec.store" => memory::verify_store(instruction, &types, &mut diagnostics),
        "vec.gather" => memory::verify_gather(source, instruction, &types, &mut diagnostics),
        "vec.scatter" => memory::verify_scatter(instruction, &types, &mut diagnostics),
        "vec.compress" | "vec.expand" => {
            operations::verify_vector_and_mask(instruction, &types, 3, &mut diagnostics)
        }
        "vec.dot" => operations::verify_dot(instruction, &types, &mut diagnostics),
        "vec.zip" => {
            operations::verify_two_vectors(instruction, &types, &mut diagnostics);
            operations::verify_optional_choice(instruction, &["lo", "hi"], &mut diagnostics)
        }
        "vec.unzip" => {
            operations::verify_two_vectors(instruction, &types, &mut diagnostics);
            operations::verify_optional_choice(instruction, &["even", "odd"], &mut diagnostics)
        }
        "vec.reverse" => operations::verify_reverse(instruction, &types, &mut diagnostics),
        "vec.slide" => {
            if types.len() >= 2 {
                operations::verify_two_vectors(instruction, &types[..2], &mut diagnostics);
            }
            operations::verify_optional_choice(instruction, &["up", "down"], &mut diagnostics);
            if types.get(2).is_some_and(|index| !is_integer_scalar(index)) {
                report(
                    &mut diagnostics,
                    instruction,
                    "vec.slide index must be an integer",
                );
            }
            types.len() == 3
        }
        "vec.concat" => {
            if types.len() != 2
                || types
                    .first()
                    .zip(types.get(1))
                    .and_then(|(left, right)| concatenate_type(left, right))
                    .is_none()
            {
                report(
                    &mut diagnostics,
                    instruction,
                    "vec.concat requires compatible fixed vectors with a valid lane count",
                );
                false
            } else {
                true
            }
        }
        "vec.split" => {
            if types.first().and_then(|ty| split_type(ty)).is_none() {
                report(
                    &mut diagnostics,
                    instruction,
                    "vec.split requires a fixed vector with an even lane count",
                );
            }
            operations::verify_optional_choice(instruction, &["lo", "hi"], &mut diagnostics);
            types.len() == 1
        }
        "vec.mask.active" => {
            if types.len() != 2 || types.iter().any(|ty| !is_integer_scalar(ty)) {
                report(
                    &mut diagnostics,
                    instruction,
                    "vec.mask.active requires two integer operands",
                );
                false
            } else {
                true
            }
        }
        "vec.mask.count" | "vec.mask.first" => {
            if types.len() != 1 || types.first().is_none_or(|ty| !is_mask(ty)) {
                report(
                    &mut diagnostics,
                    instruction,
                    "vector mask query requires one vector of i1",
                );
                false
            } else {
                true
            }
        }
        "vec.table" => operations::verify_table(instruction, &types, &mut diagnostics),
        _ => true,
    };
    if !valid_count(opcode, &types) {
        report(
            &mut diagnostics,
            instruction,
            "vector instruction has the wrong number of SSA operands",
        );
    }
    if !valid {
        return diagnostics;
    }
    diagnostics
}

fn shuffle_indices(instruction: &TextInstruction) -> Option<Vec<u32>> {
    let start = text_instruction_operands_start(&instruction.tokens);
    let open = instruction.tokens[start..]
        .iter()
        .position(|token| is_punctuation(token, '['))?
        + start;
    let close = instruction.tokens[open + 1..]
        .iter()
        .position(|token| is_punctuation(token, ']'))?
        + open
        + 1;
    let mut indices = Vec::new();
    let mut expect_value = true;
    for token in &instruction.tokens[open + 1..close] {
        if is_punctuation(token, ',') {
            if expect_value {
                return None;
            }
            expect_value = true;
        } else if expect_value {
            let index = atom(token)
                .and_then(tir_format::numeric_literals::parse_unsigned_integer)
                .and_then(|value| u32::try_from(value).ok())?;
            indices.push(index);
            expect_value = false;
        } else {
            return None;
        }
    }
    if expect_value && !indices.is_empty() {
        return None;
    }
    Some(indices)
}

fn operation_atom(instruction: &TextInstruction) -> Option<&str> {
    let start = opcode_end(instruction);
    let value_start = instruction.tokens[start..]
        .iter()
        .position(|token| is_punctuation(token, '%'))
        .map_or(instruction.tokens.len(), |index| start + index);
    instruction.tokens[start..value_start]
        .iter()
        .filter_map(atom)
        .last()
}

fn leading_modifier_atoms(instruction: &TextInstruction) -> Vec<&str> {
    let tokens = &instruction.tokens;
    let mut cursor = opcode_end(instruction);
    let mut modifiers = Vec::new();
    while tokens
        .get(cursor)
        .is_some_and(|token| is_punctuation(token, '['))
    {
        cursor += 1;
        while cursor < tokens.len() && !is_punctuation(&tokens[cursor], ']') {
            if let Some(value) = atom(&tokens[cursor]) {
                modifiers.push(value);
            }
            cursor += 1;
        }
        cursor += 1;
    }
    modifiers
}

fn type_after_keyword(source: &str, instruction: &TextInstruction, keyword: &str) -> Option<Type> {
    let start = text_instruction_operands_start(&instruction.tokens);
    let index = instruction.tokens[start..]
        .iter()
        .position(|token| atom(token) == Some(keyword))?
        + start
        + 1;
    let first = instruction.tokens.get(index)?;
    let end = instruction.tokens[index..]
        .iter()
        .find(|token| {
            matches!(
                token.kind,
                TokenKind::Punctuation('!') | TokenKind::Newline | TokenKind::End
            )
        })
        .map_or(instruction.span.end, |token| token.span.start);
    parse_type(source.get(first.span.start..end)?.trim()).ok()
}

fn explicit_type(source: &str, instruction: &TextInstruction) -> Option<Type> {
    let start = text_instruction_operands_start(&instruction.tokens);
    let first = instruction.tokens.get(start)?;
    let mut depth = 0i32;
    for token in instruction.tokens.iter().skip(start) {
        match token.kind {
            TokenKind::Punctuation('<' | '[' | '(' | '{') => depth += 1,
            TokenKind::Punctuation('>' | ']' | ')' | '}') => depth -= 1,
            TokenKind::Punctuation(',') if depth == 0 => {
                return parse_type(source.get(first.span.start..token.span.start)?.trim()).ok();
            }
            TokenKind::Punctuation('!') if depth == 0 => {
                return parse_type(source.get(first.span.start..token.span.start)?.trim()).ok();
            }
            _ => {}
        }
    }
    parse_type(source.get(first.span.start..instruction.span.end)?.trim()).ok()
}

fn concatenate_type(left: &Type, right: &Type) -> Option<Type> {
    let (
        Type::FixedVector {
            lanes: left_lanes,
            element,
        },
        Type::FixedVector {
            lanes: right_lanes,
            element: right_element,
        },
    ) = (left, right)
    else {
        return None;
    };
    if element != right_element {
        return None;
    }
    let lanes = left_lanes.checked_add(*right_lanes)?;
    valid_lanes(lanes).then(|| Type::FixedVector {
        lanes,
        element: element.clone(),
    })
}

fn split_type(vector: &Type) -> Option<Type> {
    let Type::FixedVector { lanes, element } = vector else {
        return None;
    };
    if lanes % 2 != 0 {
        return None;
    }
    let lanes = lanes / 2;
    valid_lanes(lanes).then(|| Type::FixedVector {
        lanes,
        element: element.clone(),
    })
}

fn table_result_type(indices: &Type) -> Option<Type> {
    Some(match indices {
        Type::FixedVector { lanes, .. } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(Type::Integer(8)),
        },
        Type::ScalableVector(_) => Type::ScalableVector(Box::new(Type::Integer(8))),
        _ => return None,
    })
}

fn vector_element(ty: &Type) -> Option<&Type> {
    match ty {
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => Some(element),
        _ => None,
    }
}

fn vector_shape(ty: &Type) -> Option<VectorShape> {
    Some(match ty {
        Type::FixedVector { lanes, .. } => VectorShape::Fixed(*lanes),
        Type::ScalableVector(_) => VectorShape::Scalable,
        _ => return None,
    })
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum VectorShape {
    Fixed(u32),
    Scalable,
}

fn fixed_vector(ty: &Type) -> Option<(u32, &Type)> {
    match ty {
        Type::FixedVector { lanes, element } => Some((*lanes, element)),
        _ => None,
    }
}

fn same_lanes(left: &Type, right: &Type) -> bool {
    vector_shape(left).is_some() && vector_shape(left) == vector_shape(right)
}

fn mask_matches(mask: &Type, vector: &Type) -> bool {
    same_lanes(mask, vector) && vector_element(mask) == Some(&Type::Integer(1))
}

fn is_mask(ty: &Type) -> bool {
    vector_element(ty) == Some(&Type::Integer(1))
}

fn is_integer_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_))
}

fn is_integer_vector(ty: &Type) -> bool {
    matches!(vector_element(ty), Some(Type::Integer(_)))
}

fn is_pointer(ty: &Type) -> bool {
    matches!(ty, Type::Pointer(_) | Type::Address { .. })
}

fn is_pointer_vector(ty: &Type) -> bool {
    matches!(vector_element(ty), Some(Type::Pointer(_)))
}

fn valid_lanes(lanes: u32) -> bool {
    matches!(lanes, 2 | 3 | 4 | 8 | 16 | 32 | 64)
}

fn valid_count(opcode: &str, operands: &[&Type]) -> bool {
    match opcode {
        "vec.splat" | "vec.reverse" | "vec.split" | "vec.mask.count" | "vec.mask.first" => {
            operands.len() == 1
        }
        "vec.extract" => operands.len() == 2,
        "vec.insert" | "vec.compress" | "vec.expand" | "vec.dot" | "vec.slide" => {
            operands.len() == 3
        }
        "vec.shuffle" | "vec.zip" | "vec.unzip" | "vec.concat" | "vec.mask.active"
        | "vec.table" => operands.len() == 2,
        "vec.step" | "vec.vscale" => operands.is_empty(),
        "vec.reduce" => matches!(operands.len(), 1 | 2),
        "vec.load" => matches!(operands.len(), 1 | 3),
        "vec.store" => matches!(operands.len(), 2 | 3),
        "vec.gather" => matches!(operands.len(), 1 | 2 | 3 | 4),
        "vec.scatter" => matches!(operands.len(), 2..=4),
        _ => true,
    }
}

fn clause_count(instruction: &TextInstruction, clause: &str) -> usize {
    instruction
        .tokens
        .windows(2)
        .filter(|pair| is_punctuation(&pair[0], '[') && atom(&pair[1]) == Some(clause))
        .count()
}

fn has_mask_clause(instruction: &TextInstruction) -> bool {
    clause_count(instruction, "mask") != 0
}

fn has_passthru_clause(instruction: &TextInstruction) -> bool {
    instruction
        .tokens
        .windows(2)
        .any(|pair| is_punctuation(&pair[0], ',') && atom(&pair[1]) == Some("passthru"))
}

fn opcode_end(instruction: &TextInstruction) -> usize {
    text_instruction_operands_start(&instruction.tokens)
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn is_punctuation(token: &Token, punctuation: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(actual) if actual == punctuation)
}

fn report(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(V2, message.into()).with_primary_span(instruction.span));
}
