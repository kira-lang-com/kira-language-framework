//! Close unescaped single-block borrow scopes before the return terminator.
//! Branch-spanning loans require path-sensitive lifetime analysis and are not rewritten.
use tir_format::text::TokenKind;
use tir_format::{TextInstruction, TextItemKind, parse_function_body, parse_text_module};

use crate::{CanonicalizeError, error};

pub(super) fn close_local_borrows(source: &str) -> Result<String, CanonicalizeError> {
    let module = parse_text_module(source).map_err(|issue| error(format!("TIR text: {issue}")))?;
    let mut edits = Vec::new();
    for item in &module.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let Some(body) = parse_function_body(module.item_text(*item))
            .map_err(|issue| error(format!("borrow cleanup function body: {issue}")))?
        else {
            continue;
        };
        if body.blocks.len() != 1 {
            continue;
        }
        let block = &body.blocks[0];
        let mut scopes = Vec::new();
        let mut earliest = None;
        for instruction in &block.instructions {
            match instruction.opcode.as_str() {
                "borrow.begin" | "load.borrow" => {
                    let [name] = instruction.results.as_slice() else {
                        return Err(error("borrow scope must define one SSA value"));
                    };
                    scopes.push(name.as_str());
                    earliest.get_or_insert(instruction.span.start);
                }
                "borrow.end" => {
                    let references = value_references(instruction);
                    let [name] = references.as_slice() else {
                        return Err(error("borrow.end expects exactly one borrowed SSA value"));
                    };
                    if scopes.pop() != Some(name.as_str()) {
                        return Err(error("borrow.end must close the most recent active borrow"));
                    }
                }
                _ => {}
            }
        }
        if scopes.is_empty() {
            continue;
        }
        let Some(return_instruction) = block.instructions.last() else {
            return Err(error("unclosed borrow has no terminator"));
        };
        if return_instruction.opcode != "return" {
            return Err(error(
                "unclosed borrow may not cross a non-return terminator",
            ));
        }
        if block.instructions.iter().any(|instruction| {
            instruction.opcode == "stack.dealloc"
                && earliest.is_some_and(|opened| instruction.span.start > opened)
        }) {
            return Err(error(
                "borrow scope must close before an existing stack.dealloc",
            ));
        }
        let offset = item
            .span
            .start
            .saturating_add(return_instruction.span.start);
        let before = source
            .get(..offset)
            .ok_or_else(|| error("invalid borrow return offset"))?;
        let line_start = before.rfind('\n').map_or(0, |index| index + 1);
        let indent = &before[line_start..];
        if !indent.chars().all(char::is_whitespace) {
            return Err(error("invalid borrow return indentation"));
        }
        let lines = scopes
            .iter()
            .rev()
            .map(|name| format!("{indent}borrow.end %{name}\n"))
            .collect::<String>();
        edits.push((line_start, lines));
    }
    let mut transformed = source.to_owned();
    edits.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    for (offset, contents) in edits {
        transformed.insert_str(offset, &contents);
    }
    Ok(transformed)
}

fn value_references(instruction: &TextInstruction) -> Vec<String> {
    let mut values = instruction
        .tokens
        .windows(2)
        .filter_map(|pair| match (&pair[0].kind, &pair[1].kind) {
            (TokenKind::Punctuation('%'), TokenKind::Atom(name)) => Some(name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some(result) = instruction.results.first()
        && let Some(index) = values.iter().position(|name| name == result)
    {
        values.remove(index);
    }
    values
}
