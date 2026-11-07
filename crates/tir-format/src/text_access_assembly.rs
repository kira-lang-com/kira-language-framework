//! Text encoding for access.begin/end scopes (TIR 9.4, CODE 0x187/0x188).
use tir_diag::Span;

use crate::text::TokenKind;
use crate::{TextInstruction, TextItem, TextParseError};

pub(crate) struct AccessInstruction {
    pub(crate) opcode: u64,
    pub(crate) modifiers: u64,
    pub(crate) operand: String,
    pub(crate) result: Option<String>,
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(opcode, "access.begin" | "access.end")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    item: TextItem,
) -> Result<AccessInstruction, TextParseError> {
    let begin = instruction.opcode == "access.begin";
    if !handles(&instruction.opcode) {
        return Err(error(item, instruction.span, "not an access instruction"));
    }
    let tokens = &instruction.tokens;
    let (result, offset) = if begin {
        let [first, name, equal, ..] = tokens.as_slice() else {
            return Err(error(
                item,
                instruction.span,
                "access.begin requires a result",
            ));
        };
        let (TokenKind::Punctuation('%'), TokenKind::Atom(name), TokenKind::Punctuation('=')) =
            (&first.kind, &name.kind, &equal.kind)
        else {
            return Err(error(
                item,
                instruction.span,
                "malformed access.begin result",
            ));
        };
        if instruction.results.as_slice() != [name.clone()] {
            return Err(error(
                item,
                instruction.span,
                "access.begin SSA result mismatch",
            ));
        }
        (Some(name.clone()), 3)
    } else {
        if !instruction.results.is_empty() {
            return Err(error(
                item,
                instruction.span,
                "access.end must not define a result",
            ));
        }
        (None, 0)
    };
    let Some([access, dot, mnemonic]) = tokens.get(offset..offset + 3) else {
        return Err(error(item, instruction.span, "access opcode is incomplete"));
    };
    if !matches!(&access.kind, TokenKind::Atom(value) if value == "access")
        || !matches!(&dot.kind, TokenKind::Punctuation('.'))
        || !matches!(&mnemonic.kind, TokenKind::Atom(value)
            if value == if begin { "begin" } else { "end" })
    {
        return Err(error(item, instruction.span, "invalid access opcode"));
    }
    let mut cursor = offset + 3;
    let mut access_mode = None;
    let mut checking_mode = None;
    while tokens
        .get(cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('[')))
    {
        let Some([open, value, close]) = tokens.get(cursor..cursor + 3) else {
            return Err(error(item, instruction.span, "incomplete access mode"));
        };
        if !matches!(&open.kind, TokenKind::Punctuation('['))
            || !matches!(&close.kind, TokenKind::Punctuation(']'))
        {
            return Err(error(item, instruction.span, "malformed access mode"));
        }
        let TokenKind::Atom(value) = &value.kind else {
            return Err(error(item, instruction.span, "access mode must be a name"));
        };
        match value.as_str() {
            "read" if begin && access_mode.is_none() => access_mode = Some(0u64),
            "modify" if begin && access_mode.is_none() => access_mode = Some(1u64),
            "static" if begin && checking_mode.is_none() => checking_mode = Some(0u64),
            "dynamic" if begin && checking_mode.is_none() => checking_mode = Some(1u64),
            "unchecked" if begin && checking_mode.is_none() => checking_mode = Some(2u64),
            _ => {
                return Err(error(
                    item,
                    instruction.span,
                    "invalid or repeated access mode",
                ));
            }
        }
        cursor += 3;
    }
    let Some([percent, operand]) = tokens.get(cursor..) else {
        return Err(error(
            item,
            instruction.span,
            "access operation requires one SSA operand",
        ));
    };
    let (TokenKind::Punctuation('%'), TokenKind::Atom(operand)) = (&percent.kind, &operand.kind)
    else {
        return Err(error(item, instruction.span, "invalid access SSA operand"));
    };
    let selector = access_mode.unwrap_or(0) + checking_mode.unwrap_or(0) * 2;
    Ok(AccessInstruction {
        opcode: if begin { 0x187 } else { 0x188 },
        modifiers: selector << 48,
        operand: operand.clone(),
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

    fn module(body: &str) -> String {
        format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.access(%v: i32) -> i32 engine(native) {{\nbb0(%v: i32):\n{body}\n}}\n"
        )
    }

    #[test]
    fn access_modes_and_tokens_round_trip() {
        for (mode, checking) in [
            ("read", "static"),
            ("modify", "static"),
            ("read", "dynamic"),
            ("modify", "dynamic"),
        ] {
            let source = module(&format!(
                "  %slot = stack.alloc i32\n  %scope = access.begin [{mode}] [{checking}] %slot\n  access.end %scope\n  stack.dealloc %slot\n  return %v"
            ));
            let parsed = parse_text_module(&source).expect("text parses");
            let bytes = assemble_text_module(&parsed).expect("access scope assembles");
            let binary = BinaryModule::decode(&bytes).expect("CODE decodes");
            let canonical = disassemble_text_module(&binary, u64::MAX).expect("CODE disassembles");
            assert!(canonical.contains(&format!("access.begin [{mode}] [{checking}]")));
            let rebuilt =
                assemble_text_module(&parse_text_module(&canonical).expect("text reparses"))
                    .expect("canonical CODE reassembles");
            assert_eq!(bytes, rebuilt);
        }
    }

    #[test]
    fn duplicate_or_unknown_access_modes_fail() {
        for instruction in [
            "%scope = access.begin [read] [modify] %slot",
            "%scope = access.begin [static] [dynamic] %slot",
            "%scope = access.begin [nonsense] %slot",
            "access.end %slot, %slot",
        ] {
            let source = module(&format!(
                "  %slot = stack.alloc i32\n  {instruction}\n  stack.dealloc %slot\n  return %v"
            ));
            let parsed = parse_text_module(&source).expect("text parses");
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid access assembled: {instruction}"
            );
        }
    }
}
