//! GPU tile and work-graph CODE 0x5a0, 0x5c0–0x5c5: typed SSA and node mode.
use std::collections::HashMap;
use tir_diag::Span;
use tir_types::{AddressSpace, Type};

use crate::{TextInstruction, TextItem, TextParseError};

pub(crate) const NAMES: [(&str, u64); 7] = [
    ("tile.imageblock.addr", 0x5a0),
    ("node.input.addr", 0x5c0),
    ("node.input.count", 0x5c1),
    ("node.output.allocate", 0x5c2),
    ("node.output.addr", 0x5c3),
    ("node.output.commit", 0x5c4),
    ("node.finished_cross_group", 0x5c5),
];

pub(crate) struct TileNodeOp {
    pub(crate) opcode: u64,
    pub(crate) result: Option<String>,
    pub(crate) operands: Vec<String>,
    pub(crate) modifiers: u64,
}

pub(crate) fn opcode(name: &str) -> Option<u64> {
    NAMES
        .iter()
        .find_map(|(key, value)| (*key == name).then_some(*value))
}

pub(crate) fn handles(name: &str) -> bool {
    opcode(name).is_some()
}

pub(crate) fn input_count(opcode: u64) -> usize {
    match opcode {
        0x5c1 | 0x5c5 => 0,
        0x5c2 | 0x5c3 => 2,
        _ => 1,
    }
}

pub(crate) fn result_type(opcode: u64) -> Option<Type> {
    match opcode {
        0x5a0 => Some(Type::Pointer(AddressSpace::Tile)),
        0x5c0 | 0x5c2 | 0x5c3 => Some(Type::Pointer(AddressSpace::NodeRecord)),
        0x5c1 => Some(Type::Integer(32)),
        0x5c5 => Some(Type::Integer(1)),
        _ => None,
    }
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<TileNodeOp, TextParseError> {
    let opcode = opcode(&instruction.opcode)
        .ok_or_else(|| error(item, instruction.span, "not a tile/node opcode"))?;
    let result = if result_type(opcode).is_some() {
        let [name] = instruction.results.as_slice() else {
            return Err(error(
                item,
                instruction.span,
                "tile/node operation requires one SSA result",
            ));
        };
        Some(name.clone())
    } else {
        if !instruction.results.is_empty() {
            return Err(error(
                item,
                instruction.span,
                "node.output.commit has no result",
            ));
        }
        None
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid tile/node source span"))?
        .trim();
    let prefix = result.as_ref().map_or_else(
        || instruction.opcode.clone(),
        |name| format!("%{name} = {}", instruction.opcode),
    );
    let tail = line.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "tile/node result/opcode syntax is invalid",
        )
    })?;
    let (arguments, modifiers) = if let Some(index) = tail.find(" [") {
        let mode = tail[index..].trim();
        if opcode != 0x5c2 {
            return Err(error(
                item,
                instruction.span,
                "only node.output.allocate has mode selectors",
            ));
        }
        let flag = match mode {
            "[thread]" => 0,
            "[group]" => 1 << 48,
            _ => {
                return Err(error(
                    item,
                    instruction.span,
                    "node allocation mode must be thread or group",
                ));
            }
        };
        (&tail[..index], flag)
    } else {
        (tail, 0)
    };
    let arguments = arguments.trim();
    let operands = if arguments.is_empty() {
        Vec::new()
    } else {
        arguments
            .split(',')
            .map(|value| {
                value
                    .trim()
                    .strip_prefix('%')
                    .filter(|name| valid_name(name))
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "tile/node operands must be SSA names",
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    if operands.len() != input_count(opcode) {
        return Err(error(
            item,
            instruction.span,
            "tile/node operand count is incorrect",
        ));
    }
    Ok(TileNodeOp {
        opcode,
        result,
        operands,
        modifiers,
    })
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
}

pub(crate) fn validate(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &mut HashMap<String, Type>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    if op
        .result
        .as_ref()
        .is_some_and(|name| values.contains_key(name))
    {
        return Err(error(
            item,
            instruction.span,
            "tile/node SSA result is duplicated",
        ));
    }
    let input_types = op
        .operands
        .iter()
        .map(|name| {
            values
                .get(name)
                .ok_or_else(|| error(item, instruction.span, "tile/node SSA operand is undefined"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let integer = |ty: &Type| matches!(ty, Type::Integer(1..=64));
    let node_ptr = |ty: &Type| *ty == Type::Pointer(AddressSpace::NodeRecord);
    let valid = match op.opcode {
        0x5a0 => matches!(input_types[0], Type::FixedVector { lanes: 2, element }
            if matches!(element.as_ref(), Type::Integer(32))),
        0x5c0 => integer(input_types[0]),
        0x5c1 | 0x5c5 => true,
        0x5c2 => integer(input_types[0]) && integer(input_types[1]),
        0x5c3 => node_ptr(input_types[0]) && integer(input_types[1]),
        0x5c4 => node_ptr(input_types[0]),
        _ => false,
    };
    if !valid {
        return Err(error(
            item,
            instruction.span,
            "tile/node operands have incompatible types",
        ));
    }
    if let (Some(name), Some(ty)) = (op.result, result_type(op.opcode)) {
        values.insert(name, ty);
    }
    Ok(())
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &[&str],
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    let count = u64::from(op.result.is_some());
    for word in [op.opcode, op.modifiers, count] {
        crate::leb::write_uleb(word, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    for name in &op.operands {
        let position = values
            .iter()
            .position(|value| *value == name)
            .ok_or_else(|| error(item, instruction.span, "undefined tile/node SSA operand"))?;
        crate::leb::write_uleb((values.len() - position) as u64, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    Ok(())
}

pub(super) fn error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
