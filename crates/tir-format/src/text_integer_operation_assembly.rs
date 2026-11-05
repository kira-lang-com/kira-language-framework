use crate::text::TokenKind;
use crate::{TextInstruction, TextItem, TextParseError};
use tir_diag::Span;

pub(crate) struct ParsedIntegerOperation {
    pub(crate) opcode: String,
    pub(crate) name: String,
    pub(crate) operands: Vec<String>,
    pub(crate) modifiers: u64,
}

pub(crate) fn parse_integer_operation(
    instruction: &TextInstruction,
    item: TextItem,
) -> Result<ParsedIntegerOperation, TextParseError> {
    let Some((_, operand_count)) = integer_operation_info(&instruction.opcode) else {
        return Err(item_error(
            item,
            instruction.span,
            "unsupported integer operation",
        ));
    };
    let tokens = &instruction.tokens;
    let valid_header = instruction.results.len() == 1
        && matches!(
            tokens.first().map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
        && matches!(
            tokens.get(2).map(|token| &token.kind),
            Some(TokenKind::Punctuation('='))
        )
        && matches!(tokens.get(3).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == &instruction.opcode);
    if !valid_header {
        return Err(item_error(
            item,
            instruction.span,
            "expected `%name = integer-op operands`",
        ));
    }
    let name = match &tokens[1].kind {
        TokenKind::Atom(name) if name == &instruction.results[0] => name.clone(),
        _ => {
            return Err(item_error(
                item,
                instruction.span,
                "invalid integer operation result name",
            ));
        }
    };
    let (operand_indices, modifiers) = if instruction.opcode == "icmp" {
        let predicate = match tokens.get(4).map(|token| &token.kind) {
            Some(TokenKind::Atom(predicate)) => predicate.as_str(),
            _ => {
                return Err(item_error(
                    item,
                    instruction.span,
                    "icmp predicate is missing",
                ));
            }
        };
        let selector: u64 = match predicate {
            "eq" => 0,
            "ne" => 1,
            "slt" => 2,
            "sle" => 3,
            "sgt" => 4,
            "sge" => 5,
            "ult" => 6,
            "ule" => 7,
            "ugt" => 8,
            "uge" => 9,
            _ => return Err(item_error(item, tokens[4].span, "unknown icmp predicate")),
        };
        if tokens.len() != 10
            || !matches!(
                tokens.get(5).map(|token| &token.kind),
                Some(TokenKind::Punctuation('%'))
            )
            || !matches!(
                tokens.get(7).map(|token| &token.kind),
                Some(TokenKind::Punctuation(','))
            )
            || !matches!(
                tokens.get(8).map(|token| &token.kind),
                Some(TokenKind::Punctuation('%'))
            )
        {
            return Err(item_error(
                item,
                instruction.span,
                "icmp requires two value operands",
            ));
        }
        (vec![6, 9], selector << 48)
    } else {
        let indices = match operand_count {
            1 if tokens.len() == 6
                && matches!(
                    tokens.get(4).map(|token| &token.kind),
                    Some(TokenKind::Punctuation('%'))
                ) =>
            {
                vec![5]
            }
            2 if tokens.len() == 9
                && matches!(
                    tokens.get(4).map(|token| &token.kind),
                    Some(TokenKind::Punctuation('%'))
                )
                && matches!(
                    tokens.get(6).map(|token| &token.kind),
                    Some(TokenKind::Punctuation(','))
                )
                && matches!(
                    tokens.get(7).map(|token| &token.kind),
                    Some(TokenKind::Punctuation('%'))
                ) =>
            {
                vec![5, 8]
            }
            _ => {
                return Err(item_error(
                    item,
                    instruction.span,
                    "integer operation has the wrong operand form",
                ));
            }
        };
        (indices, 0)
    };
    let mut operands = Vec::with_capacity(operand_count);
    for index in operand_indices {
        let token = &tokens[index];
        match &token.kind {
            TokenKind::Atom(value) => operands.push(value.clone()),
            _ => {
                return Err(item_error(
                    item,
                    token.span,
                    "invalid integer operation operand",
                ));
            }
        }
    }
    Ok(ParsedIntegerOperation {
        opcode: instruction.opcode.clone(),
        name,
        operands,
        modifiers,
    })
}

pub(crate) fn integer_operation_info(name: &str) -> Option<(u64, usize)> {
    match name {
        "add" => Some((0x020, 2)),
        "sub" => Some((0x021, 2)),
        "mul" => Some((0x022, 2)),
        "and" => Some((0x02b, 2)),
        "or" => Some((0x02c, 2)),
        "xor" => Some((0x02d, 2)),
        "not" => Some((0x02e, 1)),
        "neg" => Some((0x02f, 1)),
        "abs" => Some((0x030, 1)),
        "icmp" => Some((0x03c, 2)),
        _ => None,
    }
}

fn item_error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
