//! Strict textual forms for the first three ownership instructions.
use crate::text::TokenKind;
use crate::{TextInstruction, TextItem, TextParseError};
use tir_diag::Span;

pub(crate) struct OwnershipInstruction {
    pub(crate) opcode: u64,
    pub(crate) operand: String,
    pub(crate) result: Option<String>,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    item: TextItem,
) -> Result<OwnershipInstruction, TextParseError> {
    let (opcode, has_result) = match instruction.opcode.as_str() {
        "copy" => (0x180, true),
        "move" => (0x181, true),
        "destroy" => (0x182, false),
        "borrow.begin" => (0x183, true),
        "borrow.end" => (0x184, false),
        _ => {
            return Err(error(
                item,
                instruction.span,
                "not an ownership instruction",
            ));
        }
    };
    let tokens = &instruction.tokens;
    let (result, opcode_start) = if has_result {
        let [percent, name, equal, ..] = tokens.as_slice() else {
            return Err(error(
                item,
                instruction.span,
                "ownership result declaration is incomplete",
            ));
        };
        let (TokenKind::Punctuation('%'), TokenKind::Atom(name), TokenKind::Punctuation('=')) =
            (&percent.kind, &name.kind, &equal.kind)
        else {
            return Err(error(
                item,
                instruction.span,
                "invalid ownership result declaration",
            ));
        };
        if instruction.results.as_slice() != [name.clone()] {
            return Err(error(
                item,
                instruction.span,
                "ownership SSA result does not match declaration",
            ));
        }
        (Some(name.clone()), 3)
    } else {
        if !instruction.results.is_empty() {
            return Err(error(
                item,
                instruction.span,
                "ownership end/destroy cannot define a result",
            ));
        }
        (None, 0)
    };
    let dotted = instruction.opcode.starts_with("borrow.");
    let opcode_tokens = if dotted { 3 } else { 1 };
    let Some(opcode_slice) = tokens.get(opcode_start..opcode_start + opcode_tokens) else {
        return Err(error(item, instruction.span, "missing ownership opcode"));
    };
    let valid_opcode = match opcode_slice {
        [token] => matches!(&token.kind, TokenKind::Atom(value) if value == &instruction.opcode),
        [borrow, dot, tail] => {
            matches!(&borrow.kind, TokenKind::Atom(value) if value == "borrow")
                && matches!(&dot.kind, TokenKind::Punctuation('.'))
                && matches!(&tail.kind, TokenKind::Atom(value) if Some(value.as_str()) == instruction.opcode.strip_prefix("borrow."))
        }
        _ => false,
    };
    if !valid_opcode {
        return Err(error(
            item,
            instruction.span,
            "invalid ownership opcode spelling",
        ));
    }
    let operands = tokens
        .get(opcode_start + opcode_tokens..)
        .ok_or_else(|| error(item, instruction.span, "missing ownership operand"))?;
    let [percent, argument] = operands else {
        return Err(error(
            item,
            instruction.span,
            "ownership instruction expects exactly one SSA operand",
        ));
    };
    let (TokenKind::Punctuation('%'), TokenKind::Atom(argument)) = (&percent.kind, &argument.kind)
    else {
        return Err(error(
            item,
            instruction.span,
            "invalid ownership SSA operand",
        ));
    };
    Ok(OwnershipInstruction {
        opcode,
        operand: argument.clone(),
        result,
    })
}

fn error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

    fn source(operations: &str) -> String {
        format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.borrow(%a: i32) -> unit engine(native) {{\nbb0(%a: i32):\n{operations}\n}}\n"
        )
    }

    #[test]
    fn explicit_borrow_scope_round_trips() {
        let text = source("  %loan = borrow.begin %a\n  borrow.end %loan\n  return");
        let parsed = parse_text_module(&text).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("explicit borrow scope assembles");
        let binary = BinaryModule::decode(&bytes).expect("binary decodes");
        let canonical = disassemble_text_module(&binary, u64::MAX).expect("binary disassembles");
        assert!(canonical.contains("borrow.begin"));
        assert!(canonical.contains("borrow.end"));
        let rebuilt = assemble_text_module(&parse_text_module(&canonical).expect("reparse"))
            .expect("reassemble");
        assert_eq!(bytes, rebuilt);
    }

    #[test]
    fn malformed_borrow_instruction_has_no_binary_output() {
        for body in [
            "  %loan = borrow.begin %a, %a\n  return",
            "  borrow.end %a, %a\n  return",
            "  %loan = borrow.end %a\n  return",
        ] {
            let parsed = parse_text_module(&source(body)).expect("text parses");
            assert!(
                assemble_text_module(&parsed).is_err(),
                "malformed borrow assembled: {body}"
            );
        }
    }
}
