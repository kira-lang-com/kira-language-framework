//! Typed fixed-vector memory CODE (0x0C7–0x0C8).
use std::collections::HashMap;
use std::fmt::Write as _;
use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

const MASK: u64 = 1 << 32;

pub(crate) enum VectorMemory {
    Load {
        name: String,
        ty: Type,
        ptr: String,
        align: u64,
        mask: Option<(String, String)>,
    },
    Store {
        value: String,
        ptr: String,
        align: u64,
        mask: Option<String>,
    },
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(opcode, "vec.load" | "vec.store")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<VectorMemory, TextParseError> {
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid vector memory text span"))?
        .trim();
    let (main, clauses) = text
        .split_once(" [")
        .map_or((text, ""), |(head, rest)| (head, rest));
    let mut align = 0;
    let mut mask = None;
    for clause in if clauses.is_empty() {
        Vec::new()
    } else {
        clauses.split('[').collect()
    } {
        let clause = clause
            .trim()
            .strip_suffix(']')
            .ok_or_else(|| error(item, instruction.span, "unclosed vector memory clause"))?;
        if let Some(value) = clause.strip_prefix("align ") {
            if align != 0 {
                return Err(error(item, instruction.span, "duplicate vector alignment"));
            }
            align = value
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0 && value.is_power_of_two())
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "vector alignment must be a power of two",
                    )
                })?;
        } else if let Some(value) = clause.strip_prefix("mask ") {
            if mask.is_some() {
                return Err(error(item, instruction.span, "duplicate vector mask"));
            }
            mask = Some(value.to_owned());
        } else {
            return Err(error(
                item,
                instruction.span,
                "unknown vector memory clause",
            ));
        }
    }
    if instruction.opcode == "vec.load" {
        let [name] = instruction.results.as_slice() else {
            return Err(error(
                item,
                instruction.span,
                "vec.load must define one SSA result",
            ));
        };
        let prefix = format!("%{name} = vec.load ");
        let (ty, ptr) = main
            .strip_prefix(&prefix)
            .and_then(|raw| raw.split_once(", "))
            .ok_or_else(|| error(item, instruction.span, "vec.load requires T, %pointer"))?;
        let ty = crate::parse_type(ty)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
        let mask = mask
            .map(|mask| {
                let (mask, pass) = mask.split_once(", passthru ").ok_or_else(|| {
                    error(item, instruction.span, "masked vec.load requires passthru")
                })?;
                Ok((
                    parse_name(mask, item, instruction.span)?,
                    parse_name(pass, item, instruction.span)?,
                ))
            })
            .transpose()?;
        Ok(VectorMemory::Load {
            name: name.clone(),
            ty,
            ptr: parse_name(ptr, item, instruction.span)?,
            align,
            mask,
        })
    } else if instruction.opcode == "vec.store" {
        if !instruction.results.is_empty() {
            return Err(error(
                item,
                instruction.span,
                "vec.store must define zero results",
            ));
        }
        let (value, ptr) = main
            .strip_prefix("vec.store ")
            .and_then(|raw| raw.split_once(", "))
            .ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "vec.store requires %vector, %pointer",
                )
            })?;
        Ok(VectorMemory::Store {
            value: parse_name(value, item, instruction.span)?,
            ptr: parse_name(ptr, item, instruction.span)?,
            align,
            mask: mask
                .map(|value| parse_name(&value, item, instruction.span))
                .transpose()?,
        })
    } else {
        Err(error(
            item,
            instruction.span,
            "unsupported vector memory opcode",
        ))
    }
}

pub(crate) fn validate(
    operation: &VectorMemory,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    let source = |name: &str| {
        values
            .get(name)
            .ok_or_else(|| error(item, span, "vector memory SSA operand is undefined"))
    };
    let (vector, ptr, mask, result) = match operation {
        VectorMemory::Load {
            name,
            ty,
            ptr,
            mask,
            ..
        } => {
            if values.contains_key(name) {
                return Err(error(item, span, "duplicate vec.load SSA result"));
            }
            (
                ty,
                ptr,
                mask.as_ref().map(|(mask, _)| mask.as_str()),
                Some((name.clone(), ty.clone())),
            )
        }
        VectorMemory::Store {
            value, ptr, mask, ..
        } => (source(value)?, ptr, mask.as_deref(), None),
    };
    let Type::FixedVector { lanes, element } = vector else {
        return Err(error(
            item,
            span,
            "vector memory requires a fixed-vector value/type",
        ));
    };
    if !matches!(element.as_ref(), Type::Integer(_) | Type::Float(_)) {
        return Err(error(item, span, "vector memory element must be numeric"));
    }
    match source(ptr)? {
        Type::Pointer(_) => {}
        Type::Address { pointee, .. } if pointee == element => {}
        _ => {
            return Err(error(
                item,
                span,
                "vector memory address must be ptr or *element",
            ));
        }
    }
    if let Some(mask) = mask {
        if source(mask)?
            != &(Type::FixedVector {
                lanes: *lanes,
                element: Box::new(Type::Integer(1)),
            })
        {
            return Err(error(
                item,
                span,
                "vector memory mask has incorrect lanes/type",
            ));
        }
    }
    if let VectorMemory::Load {
        mask: Some((_, pass)),
        ..
    } = operation
        && source(pass)? != vector
    {
        return Err(error(
            item,
            span,
            "vec.load passthru has incorrect vector type",
        ));
    }
    Ok(result)
}

pub(crate) fn load_type(operation: &VectorMemory) -> Option<&Type> {
    match operation {
        VectorMemory::Load { ty, .. } => Some(ty),
        _ => None,
    }
}

pub(crate) fn encode(
    operation: &VectorMemory,
    values: &[&str],
    types: &BinaryTypeTable,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let delta = |name: &str| {
        values
            .iter()
            .position(|value| *value == name)
            .map(|position| (values.len() - position) as u64)
            .ok_or_else(|| error(item, span, "undefined vector memory SSA operand"))
    };
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match operation {
        VectorMemory::Load {
            ty,
            ptr,
            align,
            mask,
            ..
        } => {
            let index = crate::text_descriptor_reference::type_index(types, ty)
                .ok_or_else(|| error(item, span, "vec.load target type is not interned"))?;
            write(0x0c7)?;
            write(if mask.is_some() { MASK } else { 0 })?;
            write(1)?;
            write(index as u64)?;
            write(delta(ptr)?)?;
            write(*align)?;
            if let Some((mask, pass)) = mask {
                write(delta(mask)?)?;
                write(delta(pass)?)?;
            }
        }
        VectorMemory::Store {
            value,
            ptr,
            align,
            mask,
        } => {
            write(0x0c8)?;
            write(if mask.is_some() { MASK } else { 0 })?;
            write(0)?;
            write(delta(value)?)?;
            write(delta(ptr)?)?;
            write(*align)?;
            if let Some(mask) = mask {
                write(delta(mask)?)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.modifiers & !MASK != 0 {
        return Err(malformed("vector memory has unsupported modifiers"));
    }
    let masked = instruction.modifiers & MASK != 0;
    let name = |index: u64| {
        values
            .get(index as usize)
            .map(|(name, ty)| (name.as_str(), *ty))
            .ok_or_else(|| malformed("vector memory SSA operand index is invalid"))
    };
    let (op, result) = match instruction.opcode {
        0x0c7 => {
            if instruction.result_count != 1 {
                return Err(malformed("vec.load must return one value"));
            }
            let (ty, ptr, align, mask) = match instruction.operands.as_slice() {
                [
                    BinaryOperand::Type(ty),
                    BinaryOperand::Value(ptr),
                    BinaryOperand::Index(align),
                ] if !masked => (*ty, *ptr, *align, None),
                [
                    BinaryOperand::Type(ty),
                    BinaryOperand::Value(ptr),
                    BinaryOperand::Index(align),
                    BinaryOperand::Values(mask),
                ] if masked && mask.len() == 2 => (*ty, *ptr, *align, Some((mask[0], mask[1]))),
                _ => return Err(malformed("vec.load operands do not match binary grammar")),
            };
            let ty_name = crate::text_memory_disassembly::format_storage_type(types, ty)?;
            let (ptr_name, ptr_type) = name(ptr)?;
            let (lanes, element) = numeric_vector(types, ty)?;
            pointer_ok(types, ptr_type, element)?;
            let mut suffix = align_clause(align)?;
            if let Some((mask, passthru)) = mask {
                let (mask_name, mask_type) = name(mask)?;
                mask_ok(types, mask_type, lanes)?;
                let (pass_name, pass_ty) = name(passthru)?;
                if pass_ty != ty {
                    return Err(malformed("vec.load passthru vector type differs"));
                }
                suffix.push_str(&format!(" [mask %{mask_name}, passthru %{pass_name}]"));
            }
            (
                format!(
                    "    %v{} = vec.load {ty_name}, %{ptr_name}{suffix}",
                    values.len()
                ),
                Some(ty),
            )
        }
        0x0c8 => {
            if instruction.result_count != 0 {
                return Err(malformed("vec.store cannot define a result"));
            }
            let (vector, ptr, align, mask) = match instruction.operands.as_slice() {
                [BinaryOperand::Values(args), BinaryOperand::Index(align)]
                    if !masked && args.len() == 2 =>
                {
                    (args[0], args[1], *align, None)
                }
                [
                    BinaryOperand::Values(args),
                    BinaryOperand::Index(align),
                    BinaryOperand::Value(mask),
                ] if masked && args.len() == 2 => (args[0], args[1], *align, Some(*mask)),
                _ => return Err(malformed("vec.store operands do not match binary grammar")),
            };
            let (vec_name, vec_type) = name(vector)?;
            let (ptr_name, ptr_type) = name(ptr)?;
            let (lanes, element) = numeric_vector(types, vec_type)?;
            pointer_ok(types, ptr_type, element)?;
            let mut suffix = align_clause(align)?;
            if let Some(mask) = mask {
                let (mask_name, mask_type) = name(mask)?;
                mask_ok(types, mask_type, lanes)?;
                suffix.push_str(&format!(" [mask %{mask_name}]"));
            }
            (
                format!("    vec.store %{vec_name}, %{ptr_name}{suffix}"),
                None,
            )
        }
        _ => return Err(malformed("invalid vector memory opcode")),
    };
    writeln!(text, "{op}").map_err(|issue| malformed(issue.to_string()))?;
    if let Some(ty) = result {
        values.push((format!("v{}", values.len()), ty));
    }
    Ok(())
}

fn numeric_vector(types: &BinaryTypeTable, index: u64) -> Result<(u64, u64), FormatError> {
    let Some(BinaryType::FixedVector { lanes, element }) = types.entries.get(index as usize) else {
        return Err(malformed("vector memory needs a fixed-vector type"));
    };
    if !matches!(
        types.entries.get(*element as usize),
        Some(BinaryType::Integer(_) | BinaryType::Float(_))
    ) {
        return Err(malformed("vector memory requires numeric elements"));
    }
    Ok((*lanes, *element))
}
fn pointer_ok(types: &BinaryTypeTable, ptr: u64, element: u64) -> Result<(), FormatError> {
    match types.entries.get(ptr as usize) {
        Some(BinaryType::Pointer(_)) => Ok(()),
        Some(BinaryType::Address { pointee, .. }) if *pointee == element => Ok(()),
        _ => Err(malformed("vector memory pointer type is incompatible")),
    }
}
fn mask_ok(types: &BinaryTypeTable, mask: u64, lanes: u64) -> Result<(), FormatError> {
    let Some(BinaryType::FixedVector {
        lanes: count,
        element,
    }) = types.entries.get(mask as usize)
    else {
        return Err(malformed("vector memory mask must be fixed vector"));
    };
    if *count != lanes
        || !matches!(
            types.entries.get(*element as usize),
            Some(BinaryType::Integer(1))
        )
    {
        return Err(malformed("vector memory mask shape disagrees with vector"));
    }
    Ok(())
}
fn align_clause(align: u64) -> Result<String, FormatError> {
    if align == 0 {
        Ok(String::new())
    } else if align.is_power_of_two() {
        Ok(format!(" [align {align}]"))
    } else {
        Err(malformed("vector memory alignment is invalid"))
    }
}
fn parse_name(name: &str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    name.strip_prefix('%')
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .map(ToOwned::to_owned)
        .ok_or_else(|| error(item, span, "vector memory requires an SSA operand"))
}
fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
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
