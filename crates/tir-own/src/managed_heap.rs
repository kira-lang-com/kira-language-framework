//! Conservative destroy insertion for counted/unique heap values.
//! Only scalar-pointee allocations without aliases, and disjoint exits, are proven.
use std::collections::BTreeMap;

use tir_format::text::TokenKind;
use tir_format::{
    BasicBlock, FunctionBody, TextInstruction, TextItem, TextItemKind, parse_function_body,
    parse_text_module, parse_type,
};
use tir_types::{Convention, Type};

use crate::{CanonicalizeError, error};

pub(super) fn handles(opcode: &str) -> bool {
    matches!(
        opcode,
        "heap.alloc.counted" | "heap.alloc.unique" | "weak.new"
    )
}

pub(super) fn insert_unused_destroys(source: &str) -> Result<String, CanonicalizeError> {
    let module = parse_text_module(source).map_err(|issue| error(format!("TIR text: {issue}")))?;
    let mut edits: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for item in &module.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let function = module.item_text(*item);
        let Some(body) = parse_function_body(function)
            .map_err(|issue| error(format!("managed heap function body: {issue}")))?
        else {
            continue;
        };
        if !body
            .blocks
            .iter()
            .any(|block| block.instructions.iter().any(|op| handles(&op.opcode)))
        {
            continue;
        }
        let (entry, exits) = checked_exits(&body)?;
        if body
            .blocks
            .iter()
            .skip(1)
            .any(|block| block.instructions.iter().any(|op| handles(&op.opcode)))
        {
            return Err(error(
                "managed heap allocations must occur in the entry block",
            ));
        }
        for allocation in entry.instructions.iter().filter(|op| handles(&op.opcode)) {
            let [name] = allocation.results.as_slice() else {
                return Err(error(
                    "managed heap allocation must define exactly one SSA result",
                ));
            };
            validate_source(entry, allocation, function)?;
            let counts = destroy_counts(&body, entry, &exits, allocation, name)?;
            for (exit, count) in exits.iter().zip(counts) {
                if count != 0 {
                    continue;
                }
                let (offset, indent) = if body.blocks.len() == 1 {
                    let (start, indent) = location(source, *item, allocation)?;
                    let offset = source[start..]
                        .find('\n')
                        .map(|end| start + end + 1)
                        .ok_or_else(|| error("heap allocation has no following line"))?;
                    (offset, indent)
                } else {
                    let return_instruction = exit
                        .instructions
                        .last()
                        .ok_or_else(|| error("heap exit has no return"))?;
                    let (start, indent) = location(source, *item, return_instruction)?;
                    let offset = source[..start].rfind('\n').map_or(0, |index| index + 1);
                    (offset, indent)
                };
                edits
                    .entry(offset)
                    .or_default()
                    .push(format!("{indent}destroy %{name}\n"));
            }
        }
    }
    let mut transformed = source.to_owned();
    for (offset, instructions) in edits.into_iter().rev() {
        transformed.insert_str(offset, &instructions.concat());
    }
    Ok(transformed)
}

pub(super) fn validate_closed(body: &FunctionBody) -> Result<(), CanonicalizeError> {
    let (entry, exits) = checked_exits(body)?;
    if body
        .blocks
        .iter()
        .skip(1)
        .any(|block| block.instructions.iter().any(|op| handles(&op.opcode)))
    {
        return Err(error(
            "managed heap allocation outside entry block is unsupported",
        ));
    }
    for allocation in entry.instructions.iter().filter(|op| handles(&op.opcode)) {
        let [name] = allocation.results.as_slice() else {
            return Err(error("managed heap allocation must define one result"));
        };
        let counts = destroy_counts(body, entry, &exits, allocation, name)?;
        if counts.iter().any(|count| *count != 1) {
            return Err(error(format!(
                "managed heap value %{name} must be destroyed once on each exit"
            )));
        }
    }
    Ok(())
}

fn checked_exits(
    body: &FunctionBody,
) -> Result<(&BasicBlock, Vec<&BasicBlock>), CanonicalizeError> {
    match body.blocks.as_slice() {
        [block]
            if block
                .instructions
                .last()
                .is_some_and(|op| op.opcode == "return") =>
        {
            Ok((block, vec![block]))
        }
        [entry, left, right]
            if entry
                .instructions
                .last()
                .is_some_and(|op| op.opcode == "cond.br")
                && left.parameters.is_empty()
                && right.parameters.is_empty()
                && unit_return(left)
                && unit_return(right) =>
        {
            let terminator = entry
                .instructions
                .last()
                .expect("checked conditional branch");
            let targets = terminator
                .tokens
                .iter()
                .filter_map(|token| match &token.kind {
                    TokenKind::Atom(label)
                        if label.starts_with("bb")
                            && label[2..].chars().all(|ch| ch.is_ascii_digit()) =>
                    {
                        Some(label.as_str())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            if targets.len() != 2
                || !targets.contains(&left.label.as_str())
                || !targets.contains(&right.label.as_str())
            {
                return Err(error(
                    "managed heap conditional must branch directly to both return blocks",
                ));
            }
            Ok((entry, vec![left, right]))
        }
        _ => Err(error(
            "managed heap canonicalization requires one unit-return block or a two-exit conditional",
        )),
    }
}

fn unit_return(block: &BasicBlock) -> bool {
    block
        .instructions
        .last()
        .is_some_and(|op| op.opcode == "return" && op.tokens.len() == 1)
}

fn validate_source(
    entry: &BasicBlock,
    instruction: &TextInstruction,
    function: &str,
) -> Result<(), CanonicalizeError> {
    if instruction.opcode == "weak.new" {
        let sources = value_references(instruction);
        let [name] = sources.as_slice() else {
            return Err(error(
                "weak.new requires exactly one borrowed reference input",
            ));
        };
        let allowed = entry.parameters.iter().any(|parameter| {
            parameter.name == *name
                && matches!(parameter.ty, Type::Reference(_))
                && parameter.convention == Some(Convention::Borrow)
        });
        if !allowed {
            return Err(error(
                "weak.new canonicalization requires a borrowed ref entry parameter",
            ));
        }
        return Ok(());
    }
    let [name] = instruction.results.as_slice() else {
        return Err(error("managed heap allocation must define one SSA result"));
    };
    let source = function
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error("managed heap allocation has an invalid source span"))?;
    let prefix = format!("%{name} = {} ", instruction.opcode);
    let pointee = source
        .trim()
        .strip_prefix(&prefix)
        .and_then(|text| parse_type(text).ok())
        .ok_or_else(|| error("managed heap allocation has an unsupported pointee type"))?;
    if !matches!(pointee, Type::Integer(_) | Type::Float(_)) {
        return Err(error(
            "automatic heap destroys require a scalar integer/float pointee",
        ));
    }
    Ok(())
}

fn destroy_counts(
    body: &FunctionBody,
    entry: &BasicBlock,
    exits: &[&BasicBlock],
    allocation: &TextInstruction,
    name: &str,
) -> Result<Vec<usize>, CanonicalizeError> {
    let mut counts = vec![0; exits.len()];
    for block in &body.blocks {
        for instruction in &block.instructions {
            let references = value_references(instruction);
            if !references.iter().any(|reference| reference == name) {
                continue;
            }
            let exit = exits
                .iter()
                .position(|exit| exit.label == block.label)
                .ok_or_else(|| error(format!("heap value %{name} is used before the exit")))?;
            let transferred = body.blocks.len() == 1
                && instruction.opcode == "return"
                && references.len() == 1
                && references[0] == name;
            if (!transferred && instruction.opcode != "destroy")
                || references.len() != 1
                || references[0] != name
                || (block.label == entry.label && instruction.span.start <= allocation.span.start)
            {
                return Err(error(format!(
                    "heap value %{name} escapes, aliases, or is used after destruction"
                )));
            }
            counts[exit] += 1;
            if counts[exit] > 1 {
                return Err(error(format!(
                    "heap value %{name} is destroyed multiple times on an exit"
                )));
            }
        }
    }
    Ok(counts)
}

pub(super) fn returned_owner_type(body: &FunctionBody, source: &str) -> Option<Type> {
    let [entry] = body.blocks.as_slice() else {
        return None;
    };
    let return_instruction = entry.instructions.last()?;
    let [
        TokenKind::Atom(opcode),
        TokenKind::Punctuation('%'),
        TokenKind::Atom(returned),
    ] = return_instruction
        .tokens
        .iter()
        .map(|token| &token.kind)
        .collect::<Vec<_>>()
        .as_slice()
    else {
        return None;
    };
    if opcode != "return" {
        return None;
    }
    let owner = entry.instructions.iter().find(|instruction| {
        handles(&instruction.opcode)
            && instruction.results.len() == 1
            && instruction.results[0] == *returned
    })?;
    if validate_source(entry, owner, source).is_err() {
        return None;
    }
    if owner.opcode == "weak.new" {
        let references = value_references(owner);
        let [strong] = references.as_slice() else {
            return None;
        };
        let parameter = entry
            .parameters
            .iter()
            .find(|parameter| parameter.name == *strong)?;
        let Type::Reference(pointee) = &parameter.ty else {
            return None;
        };
        return Some(Type::Weak(pointee.clone()));
    }
    let text = source.get(owner.span.start..owner.span.end)?.trim();
    let prefix = format!("%{returned} = {} ", owner.opcode);
    let pointee = parse_type(text.strip_prefix(&prefix)?).ok()?;
    Some(if owner.opcode == "heap.alloc.counted" {
        Type::Reference(Box::new(pointee))
    } else {
        Type::Unique(Box::new(pointee))
    })
}

fn location(
    source: &str,
    item: TextItem,
    instruction: &TextInstruction,
) -> Result<(usize, String), CanonicalizeError> {
    let start = item.span.start.saturating_add(instruction.span.start);
    let before = source
        .get(..start)
        .ok_or_else(|| error("managed heap source offset is invalid"))?;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let indentation = &before[line_start..];
    if !indentation.chars().all(char::is_whitespace) {
        return Err(error("managed heap instruction indentation is invalid"));
    }
    Ok((start, indentation.to_owned()))
}

fn value_references(instruction: &TextInstruction) -> Vec<String> {
    let mut values = instruction
        .tokens
        .windows(2)
        .filter_map(|tokens| match (&tokens[0].kind, &tokens[1].kind) {
            (TokenKind::Punctuation('%'), TokenKind::Atom(value)) => Some(value.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some(result) = instruction.results.first()
        && let Some(index) = values.iter().position(|value| value == result)
    {
        values.remove(index);
    }
    values
}
