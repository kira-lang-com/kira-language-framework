//! GPU subgroup collectives (CODE 0x440–0x452), including typed reduce/scan.
use std::collections::HashMap;

use tir_diag::Span;
use tir_types::{FloatType, Type};

use crate::{TextInstruction, TextItem, TextParseError};

pub(crate) const NAMES: [&str; 19] = [
    "subgroup.elect",
    "subgroup.all",
    "subgroup.any",
    "subgroup.all_equal",
    "subgroup.broadcast",
    "subgroup.broadcast_first",
    "subgroup.ballot",
    "subgroup.inverse_ballot",
    "subgroup.ballot_bit_count",
    "subgroup.ballot_find_lsb",
    "subgroup.ballot_find_msb",
    "subgroup.shuffle",
    "subgroup.shuffle_xor",
    "subgroup.shuffle_up",
    "subgroup.shuffle_down",
    "subgroup.rotate",
    "subgroup.reduce",
    "subgroup.scan_inclusive",
    "subgroup.scan_exclusive",
];

pub(crate) const OPERATORS: [&str; 15] = [
    "add.s", "add.u", "add.f", "mul.s", "mul.u", "mul.f", "min.s", "min.u", "min.f", "max.s",
    "max.u", "max.f", "and", "or", "xor",
];

pub(crate) struct SubgroupOp {
    pub(crate) name: String,
    pub(crate) opcode: u64,
    pub(crate) inputs: Vec<String>,
    pub(crate) operator: Option<u64>,
    pub(crate) cluster: u64,
}

pub(crate) fn opcode(name: &str) -> Option<u64> {
    NAMES
        .iter()
        .position(|candidate| *candidate == name)
        .map(|index| 0x440 + index as u64)
}

pub(crate) fn handles(name: &str) -> bool {
    opcode(name).is_some()
}

pub(crate) fn result_type(name: &str) -> Option<Type> {
    match name {
        "subgroup.elect"
        | "subgroup.all"
        | "subgroup.any"
        | "subgroup.all_equal"
        | "subgroup.inverse_ballot" => Some(Type::Integer(1)),
        "subgroup.ballot" => Some(ballot()),
        "subgroup.ballot_bit_count" | "subgroup.ballot_find_lsb" | "subgroup.ballot_find_msb" => {
            Some(Type::Integer(32))
        }
        _ => None,
    }
}

pub(crate) fn ballot() -> Type {
    Type::FixedVector {
        lanes: 4,
        element: Box::new(Type::Integer(32)),
    }
}

pub(crate) fn input_count(opcode: u64) -> usize {
    match opcode {
        0x440 => 0,
        0x444 | 0x44b..=0x44f => 2,
        _ => 1,
    }
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<SubgroupOp, TextParseError> {
    let opcode = opcode(&instruction.opcode)
        .ok_or_else(|| error(item, instruction.span, "expected subgroup opcode"))?;
    let [name] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "subgroup operation requires one SSA result",
        ));
    };
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "subgroup source span is invalid"))?
        .trim();
    let prefix = format!("%{name} = {}", instruction.opcode);
    let rest = text.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "subgroup result/opcode syntax is invalid",
        )
    })?;
    let rest = rest.trim();
    let end = rest
        .find(" [")
        .or_else(|| rest.starts_with('[').then_some(0))
        .unwrap_or(rest.len());
    let arguments = rest[..end].trim();
    let mut inputs = Vec::new();
    if !arguments.is_empty() {
        for part in arguments.split(',') {
            let name = part
                .trim()
                .strip_prefix('%')
                .filter(|name| valid_name(name))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "subgroup arguments must be SSA names",
                    )
                })?;
            inputs.push(name.to_owned());
        }
    }
    if inputs.len() != input_count(opcode) {
        return Err(error(
            item,
            instruction.span,
            "subgroup SSA input count is invalid",
        ));
    }
    let mut remaining = rest[end..].trim();
    let mut operator = None;
    let mut cluster = None;
    while !remaining.is_empty() {
        let (clause, tail) = remaining
            .strip_prefix('[')
            .and_then(|text| text.split_once(']'))
            .ok_or_else(|| error(item, instruction.span, "subgroup option requires brackets"))?;
        let clause = clause.trim();
        if let Some(size) = clause.strip_prefix("cluster ") {
            if cluster.is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate subgroup cluster option",
                ));
            }
            let size = crate::numeric_literals::parse_unsigned_integer(size)
                .and_then(|size| u64::try_from(size).ok())
                .filter(|size| (1..=128).contains(size) && size.is_power_of_two())
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "subgroup cluster must be power of two in 1..=128",
                    )
                })?;
            cluster = Some(size);
        } else {
            if operator.is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate subgroup reduction operator",
                ));
            }
            let index = OPERATORS
                .iter()
                .position(|operator| *operator == clause)
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "unsupported subgroup operator or option",
                    )
                })?;
            operator = Some(index as u64);
        }
        remaining = tail.trim();
    }
    if matches!(opcode, 0x450..=0x452) != operator.is_some()
        || (!matches!(opcode, 0x450..=0x452) && cluster.is_some())
    {
        return Err(error(
            item,
            instruction.span,
            "subgroup reduce/scan requires one operator; other operations forbid operators and clusters",
        ));
    }
    Ok(SubgroupOp {
        name: name.clone(),
        opcode,
        inputs,
        operator,
        cluster: cluster.unwrap_or(0),
    })
}

fn integer(ty: &Type) -> bool {
    matches!(ty, Type::Integer(1..=64))
}
fn floating(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Float(FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64)
    )
}
fn scalar(ty: &Type) -> bool {
    integer(ty) || floating(ty)
}
fn numeric(ty: &Type) -> bool {
    scalar(ty) || matches!(ty, Type::FixedVector { element, .. } if scalar(element))
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '.'))
}

pub(crate) fn validate(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &mut HashMap<String, Type>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    if values.contains_key(&op.name) {
        return Err(error(
            item,
            instruction.span,
            "subgroup SSA result is duplicated",
        ));
    }
    let inputs = op
        .inputs
        .iter()
        .map(|name| {
            values
                .get(name)
                .ok_or_else(|| error(item, instruction.span, "subgroup operand SSA is undefined"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let valid = match op.opcode {
        0x440 => true,
        0x441 | 0x442 | 0x446 => inputs[0] == &Type::Integer(1),
        0x443 => scalar(inputs[0]),
        0x444 | 0x44b..=0x44f => numeric(inputs[0]) && inputs[1] == &Type::Integer(32),
        0x445 => numeric(inputs[0]),
        0x447..=0x44a => inputs[0] == &ballot(),
        0x450..=0x452 => {
            let op_code = op.operator.expect("required operator checked by parser");
            match op_code {
                2 | 5 | 8 | 11 => floating(inputs[0]),
                _ => integer(inputs[0]),
            }
        }
        _ => false,
    };
    if !valid {
        return Err(error(
            item,
            instruction.span,
            "subgroup input or operator has incorrect type",
        ));
    }
    let result = result_type(&instruction.opcode).unwrap_or_else(|| inputs[0].clone());
    values.insert(op.name, result);
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
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))
    };
    for word in [op.opcode, 0, 1] {
        write(word)?;
    }
    for name in &op.inputs {
        let index = values
            .iter()
            .position(|value| *value == name)
            .ok_or_else(|| error(item, instruction.span, "subgroup operand SSA is undefined"))?;
        write((values.len() - index) as u64)?;
    }
    if let Some(operator) = op.operator {
        write(operator)?;
        write(op.cluster)?;
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
