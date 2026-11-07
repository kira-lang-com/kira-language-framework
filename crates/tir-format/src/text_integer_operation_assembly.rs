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
        let mut cursor = 4;
        let mut modifiers = 0u64;
        if matches!(
            tokens.get(cursor).map(|token| &token.kind),
            Some(TokenKind::Punctuation('['))
        ) {
            let Some([_, flag, close]) = tokens.get(cursor..cursor + 3) else {
                return Err(item_error(
                    item,
                    instruction.span,
                    "integer modifier is incomplete",
                ));
            };
            if !matches!(close.kind, TokenKind::Punctuation(']')) {
                return Err(item_error(
                    item,
                    instruction.span,
                    "integer modifier is not closed",
                ));
            }
            let TokenKind::Atom(flag) = &flag.kind else {
                return Err(item_error(
                    item,
                    instruction.span,
                    "integer modifier must be a keyword",
                ));
            };
            modifiers = match (instruction.opcode.as_str(), flag.as_str()) {
                ("udiv", "exact") => 1 << 3,
                ("or", "disjoint") => 1 << 4,
                ("ctlz" | "cttz", "poison") => 1 << 47,
                _ => {
                    return Err(item_error(
                        item,
                        instruction.span,
                        "unsupported integer modifier for this operation",
                    ));
                }
            };
            cursor += 3;
        }
        let mut indices = Vec::with_capacity(operand_count);
        for position in 0..operand_count {
            if position != 0 {
                if !matches!(
                    tokens.get(cursor).map(|token| &token.kind),
                    Some(TokenKind::Punctuation(','))
                ) {
                    return Err(item_error(
                        item,
                        instruction.span,
                        "integer operands must be comma-separated",
                    ));
                }
                cursor += 1;
            }
            if !matches!(
                tokens.get(cursor).map(|token| &token.kind),
                Some(TokenKind::Punctuation('%'))
            ) {
                return Err(item_error(
                    item,
                    instruction.span,
                    "integer operands must be SSA values",
                ));
            }
            let Some(TokenKind::Atom(_)) = tokens.get(cursor + 1).map(|token| &token.kind) else {
                return Err(item_error(
                    item,
                    instruction.span,
                    "integer operand has no SSA name",
                ));
            };
            indices.push(cursor + 1);
            cursor += 2;
        }
        if cursor != tokens.len() {
            return Err(item_error(
                item,
                instruction.span,
                "integer operation has extra operands or modifiers",
            ));
        }
        (indices, modifiers)
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
        "sdiv" => Some((0x027, 2)),
        "udiv" => Some((0x028, 2)),
        "srem" => Some((0x029, 2)),
        "urem" => Some((0x02a, 2)),
        "and" => Some((0x02b, 2)),
        "or" => Some((0x02c, 2)),
        "xor" => Some((0x02d, 2)),
        "not" => Some((0x02e, 1)),
        "neg" => Some((0x02f, 1)),
        "abs" => Some((0x030, 1)),
        "shl" => Some((0x031, 2)),
        "lshr" => Some((0x032, 2)),
        "ashr" => Some((0x033, 2)),
        "rotl" => Some((0x034, 2)),
        "rotr" => Some((0x035, 2)),
        "fshl" => Some((0x036, 3)),
        "fshr" => Some((0x037, 3)),
        "smin" => Some((0x038, 2)),
        "smax" => Some((0x039, 2)),
        "umin" => Some((0x03a, 2)),
        "umax" => Some((0x03b, 2)),
        "icmp" => Some((0x03c, 2)),
        "ctlz" => Some((0x03e, 1)),
        "cttz" => Some((0x03f, 1)),
        "ctpop" => Some((0x040, 1)),
        "bswap" => Some((0x041, 1)),
        "bitreverse" => Some((0x042, 1)),
        "clmul" => Some((0x043, 2)),
        "clmulh" => Some((0x044, 2)),
        "freeze" => Some((0x045, 1)),
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
