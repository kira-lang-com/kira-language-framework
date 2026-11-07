//! Restricted same-function `br.indirect` CODE with block.addr provenance.
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use tir_diag::Span;
use tir_types::{AddressSpace, Type};

use crate::text::TokenKind;
use crate::text_control_flow_assembly::item_error;
use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, FunctionBody, TextInstruction, TextItem, TextParseError};

#[derive(Clone, Debug)]
pub(crate) struct IndirectBranch {
    pub(crate) address: String,
    pub(crate) targets: Vec<String>,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<IndirectBranch, TextParseError> {
    if instruction.opcode != "br.indirect" || !instruction.results.is_empty() {
        return Err(item_error(
            item,
            instruction.span,
            "br.indirect must be a zero-result terminator",
        ));
    }
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| item_error(item, instruction.span, "br.indirect source span is invalid"))?
        .trim();
    let rest = line.strip_prefix("br.indirect ").ok_or_else(|| {
        item_error(
            item,
            instruction.span,
            "br.indirect requires an SSA address",
        )
    })?;
    let (address, targets) = rest.split_once(", [").ok_or_else(|| {
        item_error(
            item,
            instruction.span,
            "br.indirect expects a bracketed target list",
        )
    })?;
    let address = address
        .trim()
        .strip_prefix('%')
        .filter(|name| valid_name(name))
        .ok_or_else(|| {
            item_error(
                item,
                instruction.span,
                "br.indirect address must be an SSA name",
            )
        })?;
    let targets = targets.strip_suffix(']').ok_or_else(|| {
        item_error(
            item,
            instruction.span,
            "br.indirect target list is unclosed",
        )
    })?;
    let targets = targets
        .split(',')
        .map(|label| {
            let label = label.trim();
            if !label.starts_with("bb")
                || label.len() == 2
                || !label[2..].chars().all(|ch| ch.is_ascii_digit())
            {
                return Err(item_error(
                    item,
                    instruction.span,
                    "br.indirect target must be canonical bbN",
                ));
            }
            Ok(label.to_owned())
        })
        .collect::<Result<Vec<_>, TextParseError>>()?;
    if targets.is_empty()
        || targets.len() > (1 << 24)
        || targets.iter().collect::<HashSet<_>>().len() != targets.len()
    {
        return Err(item_error(
            item,
            instruction.span,
            "br.indirect target list must be nonempty and unique",
        ));
    }
    Ok(IndirectBranch {
        address: address.to_owned(),
        targets,
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
}

pub(crate) fn validate(
    branch: &IndirectBranch,
    body: &FunctionBody,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    if values.get(&branch.address) != Some(&Type::Pointer(AddressSpace::Generic)) {
        return Err(item_error(
            item,
            span,
            "br.indirect requires a generic pointer address",
        ));
    }
    for label in &branch.targets {
        let Some((index, block)) = body
            .blocks
            .iter()
            .enumerate()
            .find(|(_, block)| block.label == *label)
        else {
            return Err(item_error(item, span, "br.indirect target does not exist"));
        };
        if label[2..].parse::<usize>().ok() != Some(index) || !block.parameters.is_empty() {
            return Err(item_error(
                item,
                span,
                "br.indirect target must be a numbered zero-parameter block",
            ));
        }
    }
    Ok(())
}

/// Restrict the initial text CODE subset to direct, same-function block-address
/// provenance. Forwarding through an owned aggregate or an SSA block parameter
/// requires a separate flow-sensitive escape proof before this subset expands.
pub(crate) fn verify_provenance(
    body: &FunctionBody,
    source: &str,
    item: TextItem,
) -> Result<(), TextParseError> {
    let mut addresses = HashMap::new();
    for block in &body.blocks {
        for instruction in &block.instructions {
            if instruction.opcode != "block.addr" {
                continue;
            }
            let address = crate::text_block_address::parse(instruction, source, item)?;
            let target = body
                .blocks
                .iter()
                .enumerate()
                .find(|(_, block)| block.label == address.label)
                .ok_or_else(|| {
                    item_error(
                        item,
                        instruction.span,
                        "block.addr target is not in this function",
                    )
                })?;
            if address.label[2..].parse::<usize>().ok() != Some(target.0) {
                return Err(item_error(
                    item,
                    instruction.span,
                    "block.addr target must match the canonical block index",
                ));
            }
            addresses.insert(address.result, address.label);
        }
    }
    for block in &body.blocks {
        for instruction in &block.instructions {
            if instruction.opcode == "block.addr" {
                continue;
            }
            for name in addresses.keys() {
                let uses = instruction.tokens.windows(2).any(|pair| {
                    matches!(pair[0].kind, TokenKind::Punctuation('%'))
                        && matches!(&pair[1].kind, TokenKind::Atom(operand) if operand == name)
                });
                if uses && instruction.opcode != "br.indirect" {
                    return Err(item_error(
                        item,
                        instruction.span,
                        "block.addr values can only flow to br.indirect in this CODE subset",
                    ));
                }
            }
            if instruction.opcode == "br.indirect" {
                let branch = parse(instruction, source, item)?;
                let Some(addressed_block) = addresses.get(&branch.address) else {
                    return Err(item_error(
                        item,
                        instruction.span,
                        "br.indirect requires a same-function block.addr SSA value",
                    ));
                };
                if !branch.targets.contains(addressed_block) {
                    return Err(item_error(
                        item,
                        instruction.span,
                        "br.indirect target set omits the block.addr destination",
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn encode(
    branch: &IndirectBranch,
    values: &[&str],
    labels: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let index = values
        .iter()
        .position(|name| *name == branch.address)
        .ok_or_else(|| item_error(item, span, "br.indirect address is undefined"))?;
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes)
            .map_err(|error| item_error(item, span, error.to_string()))
    };
    write(0x1e4)?;
    write(0)?;
    write(0)?;
    write((values.len() - index) as u64)?;
    write(branch.targets.len() as u64)?;
    for label in &branch.targets {
        let index = labels
            .iter()
            .position(|candidate| candidate == label)
            .ok_or_else(|| item_error(item, span, "indirect branch target is undeclared"))?;
        write(index as u64)?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &[(String, u64)],
    block_count: usize,
    block_parameters: &[usize],
) -> Result<(), FormatError> {
    if instruction.opcode != 0x1e4
        || !instruction.terminator
        || instruction.modifiers != 0
        || instruction.result_count != 0
    {
        return Err(malformed("br.indirect requires zero result and modifiers"));
    }
    let [
        BinaryOperand::Value(pointer),
        BinaryOperand::Indices(targets),
    ] = instruction.operands.as_slice()
    else {
        return Err(malformed(
            "br.indirect must carry one address and counted target indexes",
        ));
    };
    let (name, ty) = usize::try_from(*pointer)
        .ok()
        .and_then(|index| values.get(index))
        .ok_or_else(|| malformed("br.indirect address is undefined"))?;
    if !matches!(
        types.entries.get(*ty as usize),
        Some(BinaryType::Pointer(0))
    ) {
        return Err(malformed("br.indirect address must be a generic pointer"));
    }
    if targets.is_empty() || targets.iter().collect::<HashSet<_>>().len() != targets.len() {
        return Err(malformed("br.indirect targets must be nonempty and unique"));
    }
    let mut target_names = Vec::new();
    for target in targets {
        let index = usize::try_from(*target)
            .map_err(|_| malformed("br.indirect target index overflows"))?;
        if index >= block_count || block_parameters.get(index) != Some(&0) {
            return Err(malformed(
                "br.indirect target must name a parameterless block",
            ));
        }
        target_names.push(format!("bb{target}"));
    }
    writeln!(text, "br.indirect %{name}, [{}]", target_names.join(", "))
        .map_err(|issue| malformed(issue.to_string()))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
