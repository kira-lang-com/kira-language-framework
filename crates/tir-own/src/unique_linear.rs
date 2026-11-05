//! Ownership transfer through a strictly linear sequence of explicit moves.
//! This deliberately accepts no aliases, implicit copies, or borrowed flows.
use tir_format::BasicBlock;
use tir_format::text::TokenKind;

use crate::{CanonicalizeError, error};

pub(super) fn remaining_owned_values(
    block: &BasicBlock,
    owned_parameters: &[&str],
) -> Result<Vec<String>, CanonicalizeError> {
    let mut live = owned_parameters
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    let Some((terminator, operations)) = block.instructions.split_last() else {
        return Err(error("owned unique block is empty"));
    };
    if terminator.opcode != "return" || terminator.tokens.len() != 1 {
        return Err(error(
            "linear owned unique canonicalization requires a unit return",
        ));
    }
    for instruction in operations {
        match instruction.opcode.as_str() {
            "move" => {
                let [p1, result, equal, mnemonic, p2, operand] = instruction.tokens.as_slice()
                else {
                    return Err(error("ownership move expects `%result = move %source`"));
                };
                let (
                    TokenKind::Punctuation('%'),
                    TokenKind::Atom(result),
                    TokenKind::Punctuation('='),
                    TokenKind::Atom(mnemonic),
                    TokenKind::Punctuation('%'),
                    TokenKind::Atom(operand),
                ) = (
                    &p1.kind,
                    &result.kind,
                    &equal.kind,
                    &mnemonic.kind,
                    &p2.kind,
                    &operand.kind,
                )
                else {
                    return Err(error("invalid ownership move operands"));
                };
                if mnemonic != "move" || instruction.results.as_slice() != [result.clone()] {
                    return Err(error("ownership move result is malformed"));
                }
                let Some(index) = live.iter().position(|name| name == operand) else {
                    return Err(error(format!(
                        "owned unique value `%{operand}` was already consumed or never owned"
                    )));
                };
                if live.iter().any(|name| name == result) {
                    return Err(error("ownership move cannot redefine a live owned value"));
                }
                live.remove(index);
                live.insert(index, result.clone());
            }
            "destroy" => {
                let [mnemonic, percent, operand] = instruction.tokens.as_slice() else {
                    return Err(error("destroy expects one SSA operand"));
                };
                let (
                    TokenKind::Atom(mnemonic),
                    TokenKind::Punctuation('%'),
                    TokenKind::Atom(operand),
                ) = (&mnemonic.kind, &percent.kind, &operand.kind)
                else {
                    return Err(error("invalid ownership destroy operand"));
                };
                if mnemonic != "destroy" || !instruction.results.is_empty() {
                    return Err(error("destroy must not define a result"));
                }
                let Some(index) = live.iter().position(|name| name == operand) else {
                    return Err(error(format!(
                        "owned unique value `%{operand}` was already consumed or never owned"
                    )));
                };
                live.remove(index);
            }
            _ => {
                return Err(error(
                    "linear owned unique canonicalization supports only explicit move and destroy operations",
                ));
            }
        }
    }
    Ok(live)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tir_format::parse_function_body;

    fn remaining(body: &str) -> Result<Vec<String>, CanonicalizeError> {
        let source = format!(
            "func @Smoke.drop(%value: owned uniq i32) -> unit {{\nbb0(%value: owned uniq i32):\n{body}\n}}"
        );
        let parsed = parse_function_body(&source)
            .expect("body parses")
            .expect("has body");
        remaining_owned_values(&parsed.blocks[0], &["value"])
    }

    #[test]
    fn moves_track_the_new_live_owner() {
        assert_eq!(
            remaining("  %m1 = move %value\n  %m2 = move %m1\n  return").expect("moves are valid"),
            ["m2"],
        );
        assert!(
            remaining("  %m1 = move %value\n  destroy %m1\n  return")
                .expect("destruction valid")
                .is_empty()
        );
    }

    #[test]
    fn move_or_destroy_after_consumption_is_rejected() {
        for body in [
            "  %m1 = move %value\n  %m2 = move %value\n  return",
            "  destroy %value\n  destroy %value\n  return",
            "  %m1 = copy %value\n  return",
        ] {
            assert!(
                remaining(body).is_err(),
                "invalid ownership operation accepted: {body}"
            );
        }
    }
}
