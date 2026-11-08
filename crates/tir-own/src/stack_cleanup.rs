//! Insert LIFO stack deallocations on returns when the allocation lives
//! in the entry block and no exit may bypass the cleanup.
use std::collections::HashSet;

use tir_format::text::TokenKind;
use tir_format::{
    BasicBlock, TextInstruction, TextItem, TextItemKind, parse_function_body, parse_text_module,
};

use crate::{CanonicalizeError, error};

pub(super) fn insert_missing_deallocs(source: &str) -> Result<String, CanonicalizeError> {
    let module = parse_text_module(source).map_err(|issue| error(format!("TIR text: {issue}")))?;
    let mut edits = Vec::new();
    for item in &module.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let Some(body) = parse_function_body(module.item_text(*item))
            .map_err(|issue| error(format!("stack cleanup function body: {issue}")))?
        else {
            continue;
        };
        let Some(entry) = body.blocks.first() else {
            continue;
        };
        let allocations = entry
            .instructions
            .iter()
            .filter(|instruction| instruction.opcode == "stack.alloc")
            .collect::<Vec<_>>();
        let any_allocation = body.blocks.iter().skip(1).any(|block| {
            block
                .instructions
                .iter()
                .any(|op| op.opcode == "stack.alloc")
        });
        if any_allocation {
            return Err(error(
                "automatic stack cleanup requires all allocations in the entry block",
            ));
        }
        if allocations.is_empty() {
            continue;
        }
        let mut slots = Vec::with_capacity(allocations.len());
        let mut defined = HashSet::new();
        for allocation in allocations {
            let [name] = allocation.results.as_slice() else {
                return Err(error("stack.alloc must define one address SSA value"));
            };
            if !defined.insert(name.as_str()) {
                return Err(error("duplicate stack.alloc SSA name"));
            }
            slots.push(name.as_str());
        }
        let order = slots.iter().rev().copied().collect::<Vec<_>>();
        let mut exits = 0;
        for block in &body.blocks {
            let Some(return_instruction) = block.instructions.last() else {
                return Err(error(
                    "stack cleanup requires a terminator in every basic block",
                ));
            };
            if return_instruction.opcode != "return" {
                if block
                    .instructions
                    .iter()
                    .any(|op| op.opcode == "stack.dealloc")
                {
                    return Err(error(
                        "stack cleanup cannot deallocate on a non-return block",
                    ));
                }
                if !matches!(return_instruction.opcode.as_str(), "br" | "cond.br") {
                    return Err(error(
                        "automatic stack cleanup cannot prove non-return exit behavior",
                    ));
                }
                continue;
            }
            exits += 1;
            let present = existing_deallocations(block, body.blocks.len() == 1)?;
            if present.len() > order.len()
                || present
                    .iter()
                    .zip(&order)
                    .any(|(actual, expected)| actual != expected)
            {
                return Err(error(
                    "existing stack.dealloc instructions do not follow LIFO allocation order",
                ));
            }
            let missing = &order[present.len()..];
            if missing.is_empty() {
                continue;
            }
            let (offset, indent) = before_instruction(source, *item, return_instruction)?;
            let text = missing
                .iter()
                .map(|name| format!("{indent}stack.dealloc %{name}\n"))
                .collect::<String>();
            edits.push((offset, text));
        }
        if exits == 0 {
            return Err(error("stack cleanup cannot prove an exit return"));
        }
    }
    let mut transformed = source.to_owned();
    edits.sort_unstable_by(|left, right| right.0.cmp(&left.0));
    for (offset, instructions) in edits {
        transformed.insert_str(offset, &instructions);
    }
    Ok(transformed)
}

fn existing_deallocations(
    block: &BasicBlock,
    single_block: bool,
) -> Result<Vec<String>, CanonicalizeError> {
    let operations = block
        .instructions
        .get(..block.instructions.len().saturating_sub(1))
        .ok_or_else(|| error("return block is malformed"))?;
    let count = operations
        .iter()
        .rev()
        .take_while(|op| op.opcode == "stack.dealloc")
        .count();
    let (earlier, suffix) = operations.split_at(operations.len() - count);
    if !single_block && earlier.iter().any(|op| op.opcode == "stack.dealloc") {
        return Err(error(
            "existing stack.dealloc must be a contiguous suffix before return",
        ));
    }
    let deallocations = if single_block {
        operations
            .iter()
            .filter(|op| op.opcode == "stack.dealloc")
            .collect::<Vec<_>>()
    } else {
        suffix.iter().collect::<Vec<_>>()
    };
    deallocations
        .iter()
        .map(|op| {
            let references = value_references(op);
            let [name] = references.as_slice() else {
                return Err(error("stack.dealloc must take exactly one SSA address"));
            };
            Ok(name.clone())
        })
        .collect()
}

fn before_instruction(
    source: &str,
    item: TextItem,
    instruction: &TextInstruction,
) -> Result<(usize, String), CanonicalizeError> {
    let offset = item.span.start.saturating_add(instruction.span.start);
    let before = source
        .get(..offset)
        .ok_or_else(|| error("stack cleanup source offset is invalid"))?;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let indentation = &before[line_start..];
    if !indentation.chars().all(char::is_whitespace) {
        return Err(error("stack cleanup return indentation is invalid"));
    }
    Ok((line_start, indentation.to_owned()))
}

fn value_references(instruction: &TextInstruction) -> Vec<String> {
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
    names
}
