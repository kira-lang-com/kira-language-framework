//! Ownership transfer through a strictly linear sequence of explicit moves.
//! This deliberately accepts no aliases, implicit copies, or borrowed flows.
use tir_format::text::TokenKind;
use tir_format::{BasicBlock, TextInstruction};

use crate::{CanonicalizeError, error};

pub(super) fn remaining_owned_values(
    block: &BasicBlock,
    owned_parameters: &[&str],
) -> Result<Vec<String>, CanonicalizeError> {
    let mut live = owned_parameters
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    let mut borrows: Vec<(String, String)> = Vec::new();
    let Some((terminator, operations)) = block.instructions.split_last() else {
        return Err(error("owned unique block is empty"));
    };
    let returned = match terminator.tokens.as_slice() {
        [token] if matches!(&token.kind, TokenKind::Atom(opcode) if opcode == "return") => None,
        [op, percent, name]
            if matches!(&op.kind, TokenKind::Atom(opcode) if opcode == "return")
                && matches!(percent.kind, TokenKind::Punctuation('%')) =>
        {
            let TokenKind::Atom(name) = &name.kind else {
                return Err(error("owned unique return must use an SSA value"));
            };
            Some(name.clone())
        }
        _ => {
            return Err(error(
                "owned unique canonicalization requires a direct return",
            ));
        }
    };
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
                if borrows.iter().any(|(_, owner)| owner == operand) {
                    return Err(error(
                        "owned unique value cannot be moved during an active borrow",
                    ));
                }
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
                if borrows.iter().any(|(_, owner)| owner == operand) {
                    return Err(error(
                        "owned unique value cannot be destroyed during an active borrow",
                    ));
                }
                live.remove(index);
            }
            "borrow.begin" => {
                let [loan] = instruction.results.as_slice() else {
                    return Err(error("borrow.begin must define one borrowed SSA value"));
                };
                let owner = single_operand(instruction)?;
                if !live.contains(&owner) {
                    return Err(error("borrow.begin requires a live owned unique value"));
                }
                if borrows.iter().any(|(name, _)| name == loan) {
                    return Err(error("borrow.begin redefines a live borrowed value"));
                }
                borrows.push((loan.clone(), owner));
            }
            "borrow.end" => {
                if !instruction.results.is_empty() {
                    return Err(error("borrow.end must not define an SSA value"));
                }
                let loan = single_operand(instruction)?;
                let Some((current, _)) = borrows.pop() else {
                    return Err(error("borrow.end has no active borrow scope"));
                };
                if current != loan {
                    return Err(error("borrow.end must close the innermost active borrow"));
                }
            }
            _ => {
                return Err(error(
                    "linear owned unique canonicalization supports only explicit move and destroy operations",
                ));
            }
        }
    }
    if !borrows.is_empty() {
        return Err(error("owned unique borrow scopes must close before return"));
    }
    if let Some(returned) = returned {
        let Some(index) = live.iter().position(|name| name == &returned) else {
            return Err(error(format!(
                "returned owned unique value `%{returned}` is not live or was already consumed"
            )));
        };
        live.remove(index);
    }
    Ok(live)
}

fn single_operand(instruction: &TextInstruction) -> Result<String, CanonicalizeError> {
    let mut names = instruction
        .tokens
        .windows(2)
        .filter_map(|pair| match (&pair[0].kind, &pair[1].kind) {
            (TokenKind::Punctuation('%'), TokenKind::Atom(name)) => Some(name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some(result) = instruction.results.first()
        && let Some(index) = names.iter().position(|name| name == result)
    {
        names.remove(index);
    }
    let [name] = names.as_slice() else {
        return Err(error(
            "borrow scope instruction requires exactly one SSA operand",
        ));
    };
    Ok(name.clone())
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

    #[test]
    fn borrowed_owner_can_be_destroyed_after_loan_closes() {
        assert_eq!(
            remaining("  %loan = borrow.begin %value\n  borrow.end %loan\n  return")
                .expect("balanced borrow"),
            ["value"]
        );
    }

    #[test]
    fn live_borrow_prevents_consumption() {
        for body in [
            "  %loan = borrow.begin %value\n  %moved = move %value\n  borrow.end %loan\n  return",
            "  %loan = borrow.begin %value\n  destroy %value\n  borrow.end %loan\n  return",
            "  %loan = borrow.begin %value\n  return",
            "  borrow.end %value\n  return",
        ] {
            assert!(
                remaining(body).is_err(),
                "invalid borrow flow accepted: {body}"
            );
        }
    }

    #[test]
    fn nested_scopes_must_close_in_order() {
        let valid = "  %first = borrow.begin %value\n  %second = borrow.begin %value\n  borrow.end %second\n  borrow.end %first\n  return";
        assert_eq!(
            remaining(valid).expect("properly nested borrows"),
            ["value"]
        );
        let invalid = "  %first = borrow.begin %value\n  %second = borrow.begin %value\n  borrow.end %first\n  borrow.end %second\n  return";
        assert!(
            remaining(invalid).is_err(),
            "out-of-order borrow end was accepted"
        );
    }
}
