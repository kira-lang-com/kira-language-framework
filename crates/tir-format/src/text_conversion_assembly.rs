use tir_diag::Span;
use tir_types::Type;

use crate::{BinaryType, BinaryTypeTable, TextInstruction, TextItem, TextParseError};

pub(crate) struct Conversion {
    pub result: String,
    pub source: String,
    pub target: Type,
    pub opcode: u64,
    pub modifiers: u64,
}

pub(crate) fn opcode(name: &str) -> Option<u64> {
    Some(
        0x0a0
            + match name {
                "trunc" => 0,
                "zext" => 1,
                "sext" => 2,
                "fptrunc" => 3,
                "fpext" => 4,
                "sitofp" => 5,
                "uitofp" => 6,
                "fptosi" => 7,
                "fptoui" => 8,
                "ptrtoint" => 9,
                "inttoptr" => 10,
                "bitcast" => 11,
                "fcvt" => 12,
                _ => return None,
            },
    )
}

pub(crate) fn name(opcode: u64) -> Option<&'static str> {
    Some(match opcode {
        0x0a0 => "trunc",
        0x0a1 => "zext",
        0x0a2 => "sext",
        0x0a3 => "fptrunc",
        0x0a4 => "fpext",
        0x0a5 => "sitofp",
        0x0a6 => "uitofp",
        0x0a7 => "fptosi",
        0x0a8 => "fptoui",
        0x0a9 => "ptrtoint",
        0x0aa => "inttoptr",
        0x0ab => "bitcast",
        0x0ac => "fcvt",
        _ => return None,
    })
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<Conversion, TextParseError> {
    let opcode = opcode(&instruction.opcode)
        .ok_or_else(|| error(item, instruction.span, "unknown conversion opcode"))?;
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "conversion must define exactly one SSA result",
        ));
    };
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "conversion source span is invalid"))?
        .trim();
    let prefix = format!("%{result} = {} ", instruction.opcode);
    let rest = text.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "conversion syntax requires `%result = opcode %value to T`",
        )
    })?;
    let (left, right) = rest.split_once(" to ").ok_or_else(|| {
        error(
            item,
            instruction.span,
            "conversion must state destination type",
        )
    })?;
    let mut left = left.trim();
    let mut modifiers = 0u64;
    if matches!(opcode, 0x0a7 | 0x0a8) {
        let (selector, remainder) = if let Some(rest) = left.strip_prefix("[sat] ") {
            (0, rest)
        } else if let Some(rest) = left.strip_prefix("[trap] ") {
            (1, rest)
        } else if let Some(rest) = left.strip_prefix("[poison] ") {
            (2, rest)
        } else {
            return Err(error(
                item,
                instruction.span,
                "fptosi/fptoui requires [sat|trap|poison]",
            ));
        };
        left = remainder;
        modifiers = match selector {
            0 => 2,
            1 => 1,
            _ => (1 << 47) | 3,
        };
    }
    let source_name = left
        .strip_prefix('%')
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "conversion operand must be one SSA value",
            )
        })?;
    let (type_text, options) = match right.find(" [") {
        Some(index) => (&right[..index], right[index..].trim()),
        None => (right.trim(), ""),
    };
    let target = crate::parse_type(type_text).map_err(|issue| {
        error(
            item,
            instruction.span,
            format!("invalid conversion destination type: {issue}"),
        )
    })?;
    let mut options = options;
    if matches!(opcode, 0x0a3 | 0x0a5 | 0x0a6 | 0x0ac) {
        for (mode, code) in [("rne", 1), ("rtz", 2), ("rup", 3), ("rdn", 4), ("rna", 5)] {
            let prefix = format!("[{mode}]");
            if let Some(tail) = options.strip_prefix(&prefix) {
                modifiers |= code << 13;
                options = tail.trim();
                break;
            }
        }
    }
    if opcode == 0x0ac {
        for (mode, code) in [("sat", 0), ("satfinite", 1), ("nonsat", 2)] {
            let prefix = format!("[{mode}]");
            if let Some(tail) = options.strip_prefix(&prefix) {
                modifiers |= code << 48;
                options = tail.trim();
                break;
            }
        }
    }
    if !options.is_empty() {
        return Err(error(
            item,
            instruction.span,
            "unsupported conversion modifier",
        ));
    }
    Ok(Conversion {
        result: result.clone(),
        source: source_name.to_owned(),
        target,
        opcode,
        modifiers,
    })
}

pub(crate) fn target_index(types: &BinaryTypeTable, ty: &Type) -> Option<usize> {
    crate::text_memory_assembly::supported_type_index(types, ty).or_else(|| match ty {
        Type::Pointer(space) => types.entries.iter().position(
            |entry| matches!(entry, BinaryType::Pointer(encoded) if *encoded == *space as u8),
        ),
        _ => None,
    })
}

pub(crate) fn validate(from: &Type, to: &Type, opcode: u64) -> bool {
    let (from_scalar, to_scalar) = match (from, to) {
        (
            Type::FixedVector {
                lanes: a,
                element: x,
            },
            Type::FixedVector {
                lanes: b,
                element: y,
            },
        ) if a == b => (x.as_ref(), y.as_ref()),
        (Type::FixedVector { .. }, _) | (_, Type::FixedVector { .. }) => return false,
        (from, to) => (from, to),
    };
    match opcode {
        0x0a0 => matches!((from_scalar, to_scalar), (Type::Integer(a), Type::Integer(b)) if a > b),
        0x0a1 | 0x0a2 => {
            matches!((from_scalar, to_scalar), (Type::Integer(a), Type::Integer(b)) if a < b)
        }
        0x0a3 => {
            matches!((from_scalar, to_scalar), (Type::Float(a), Type::Float(b)) if a.bits() > b.bits())
        }
        0x0a4 => {
            matches!((from_scalar, to_scalar), (Type::Float(a), Type::Float(b)) if a.bits() < b.bits())
        }
        0x0a5 | 0x0a6 => matches!((from_scalar, to_scalar), (Type::Integer(_), Type::Float(_))),
        0x0a7 | 0x0a8 => matches!((from_scalar, to_scalar), (Type::Float(_), Type::Integer(_))),
        0x0a9 => matches!((from, to), (Type::Pointer(_), Type::Integer(64))),
        0x0aa => matches!((from, to), (Type::Integer(64), Type::Pointer(_))),
        0x0ab => bit_size(from) == bit_size(to) && bit_size(to).is_some(),
        0x0ac => matches!((from_scalar, to_scalar), (Type::Float(_), Type::Float(_))),
        _ => false,
    }
}

fn bit_size(ty: &Type) -> Option<u64> {
    match ty {
        Type::Integer(bits) => Some(u64::from(*bits)),
        Type::Float(float) => Some(u64::from(float.bits())),
        Type::Pointer(_) => Some(64),
        Type::FixedVector { lanes, element } => {
            bit_size(element).and_then(|bits| bits.checked_mul(u64::from(*lanes)))
        }
        _ => None,
    }
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
