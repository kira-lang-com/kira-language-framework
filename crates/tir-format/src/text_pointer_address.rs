use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{AddressSpace, Type};

use crate::types::{
    BinaryInstruction, BinaryOperand, BinarySignatureTable, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum PointerAddress {
    Cast {
        result: String,
        address: String,
        ty: Type,
    },
    ToPointer {
        result: String,
        address: String,
    },
    ToAddress {
        result: String,
        pointer: String,
        ty: Type,
    },
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(opcode, "addr.cast" | "addr.toptr" | "ptr.toaddr")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<PointerAddress, TextParseError> {
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "pointer/address cast must define one SSA result",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "pointer/address cast source span is invalid",
            )
        })?
        .trim();
    let prefix = format!("%{result} = {} ", instruction.opcode);
    let args = line.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "invalid pointer/address cast syntax",
        )
    })?;
    if instruction.opcode == "addr.cast" {
        let (address, ty) = args
            .split_once(" to ")
            .ok_or_else(|| error(item, instruction.span, "addr.cast requires %address to *T"))?;
        let ty = crate::parse_type(ty)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
        if !matches!(ty, Type::Address { .. }) {
            return Err(error(
                item,
                instruction.span,
                "addr.cast target must be an address type",
            ));
        }
        Ok(PointerAddress::Cast {
            result: result.clone(),
            address: ssa(address, item, instruction.span)?,
            ty,
        })
    } else if instruction.opcode == "addr.toptr" {
        Ok(PointerAddress::ToPointer {
            result: result.clone(),
            address: ssa(args, item, instruction.span)?,
        })
    } else if instruction.opcode == "ptr.toaddr" {
        let (pointer, ty) = args
            .split_once(" as ")
            .ok_or_else(|| error(item, instruction.span, "ptr.toaddr requires %pointer as *T"))?;
        let ty = crate::parse_type(ty)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
        if !matches!(ty, Type::Address { .. }) {
            return Err(error(
                item,
                instruction.span,
                "ptr.toaddr destination must be typed address",
            ));
        }
        Ok(PointerAddress::ToAddress {
            result: result.clone(),
            pointer: ssa(pointer, item, instruction.span)?,
            ty,
        })
    } else {
        Err(error(
            item,
            instruction.span,
            "unsupported pointer/address opcode",
        ))
    }
}

pub(crate) fn destination_type(operation: &PointerAddress) -> Option<&Type> {
    match operation {
        PointerAddress::Cast { ty, .. } | PointerAddress::ToAddress { ty, .. } => Some(ty),
        _ => None,
    }
}

pub(crate) fn validate(
    operation: &PointerAddress,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    let (name, result) = match operation {
        PointerAddress::Cast {
            result,
            address,
            ty,
        } => {
            let Some(Type::Address {
                pointee: from,
                space: from_space,
            }) = values.get(address)
            else {
                return Err(error(
                    item,
                    span,
                    "addr.cast requires a defined typed address",
                ));
            };
            let Type::Address {
                pointee: to,
                space: to_space,
            } = ty
            else {
                unreachable!()
            };
            if from_space != to_space {
                return Err(error(item, span, "addr.cast must preserve address space"));
            }
            let mut layouts = tir_layout::LayoutEngine::new();
            let compatible = match (layouts.layout(from), layouts.layout(to)) {
                (Ok(first), Ok(second)) => first == second,
                _ => false,
            };
            if !compatible {
                return Err(error(
                    item,
                    span,
                    "addr.cast pointee layouts are incompatible or unavailable",
                ));
            }
            (result.clone(), ty.clone())
        }
        PointerAddress::ToPointer { result, address } => {
            let Some(Type::Address { space, .. }) = values.get(address) else {
                return Err(error(
                    item,
                    span,
                    "addr.toptr requires a defined address SSA operand",
                ));
            };
            (
                result.clone(),
                Type::Pointer(space.unwrap_or(AddressSpace::Generic)),
            )
        }
        PointerAddress::ToAddress {
            result,
            pointer,
            ty,
        } => {
            let Some(Type::Pointer(pointer_space)) = values.get(pointer) else {
                return Err(error(
                    item,
                    span,
                    "ptr.toaddr requires a defined pointer SSA operand",
                ));
            };
            let Type::Address { space, .. } = ty else {
                unreachable!()
            };
            if space.unwrap_or(AddressSpace::Generic) != *pointer_space {
                return Err(error(
                    item,
                    span,
                    "ptr.toaddr cannot change the pointer address space",
                ));
            }
            (result.clone(), ty.clone())
        }
    };
    if values.contains_key(&name) {
        return Err(error(
            item,
            span,
            "pointer/address SSA result name is already defined",
        ));
    }
    Ok((name, result))
}

pub(crate) fn encode(
    operation: &PointerAddress,
    names: &[&str],
    types: &BinaryTypeTable,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let (name, ty, opcode) = match operation {
        PointerAddress::Cast { address, ty, .. } => (address, Some(ty), 0x126),
        PointerAddress::ToPointer { address, .. } => (address, None, 0x127),
        PointerAddress::ToAddress { pointer, ty, .. } => (pointer, Some(ty), 0x128),
    };
    let delta = names
        .iter()
        .position(|candidate| *candidate == name)
        .map(|index| (names.len() - index) as u64)
        .ok_or_else(|| error(item, span, "pointer/address input SSA value is undefined"))?;
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(opcode)?;
    write(0)?;
    write(1)?;
    write(delta)?;
    if let Some(ty) = ty {
        let index = crate::text_descriptor_reference::type_index(types, ty)
            .ok_or_else(|| error(item, span, "ptr.toaddr destination type was not interned"))?;
        write(index as u64)?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    operation: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if operation.modifiers != 0 || operation.result_count != 1 {
        return Err(malformed(
            "pointer/address conversion requires one result, zero modifiers",
        ));
    }
    let (id, target) = match (operation.opcode, operation.operands.as_slice()) {
        (0x126, [BinaryOperand::Value(value), BinaryOperand::Type(index)]) => {
            (*value, Some(*index))
        }
        (0x127, [BinaryOperand::Value(value)]) => (*value, None),
        (0x128, [BinaryOperand::Value(value), BinaryOperand::Type(index)]) => {
            (*value, Some(*index))
        }
        _ => return Err(malformed("pointer/address CODE operands are malformed")),
    };
    let (source, source_ty) = usize::try_from(id)
        .ok()
        .and_then(|index| values.get(index))
        .map(|(name, ty)| (name.as_str(), *ty))
        .ok_or_else(|| malformed("pointer/address operand has invalid SSA value index"))?;
    let (code, result_ty) = if operation.opcode == 0x126 {
        let target = target.ok_or_else(|| malformed("addr.cast lacks target address type"))?;
        let Some(BinaryType::Address {
            pointee: from,
            space: from_space,
        }) = types.entries.get(source_ty as usize)
        else {
            return Err(malformed("addr.cast source must be a typed address"));
        };
        let Some(BinaryType::Address {
            pointee: to,
            space: to_space,
        }) = types.entries.get(target as usize)
        else {
            return Err(malformed("addr.cast target must be a typed address"));
        };
        if from_space != to_space {
            return Err(malformed("addr.cast must preserve address space"));
        }
        if from != to {
            let first = crate::text_type_disassembly::format_type_reference(
                types,
                *from,
                &BinarySignatureTable::default(),
            )?;
            let second = crate::text_type_disassembly::format_type_reference(
                types,
                *to,
                &BinarySignatureTable::default(),
            )?;
            let (first, second) = (
                crate::parse_type(&first)
                    .map_err(|_| malformed("addr.cast source type is invalid"))?,
                crate::parse_type(&second)
                    .map_err(|_| malformed("addr.cast target type is invalid"))?,
            );
            let mut layouts = tir_layout::LayoutEngine::new();
            if !matches!((layouts.layout(&first), layouts.layout(&second)),
                (Ok(first), Ok(second)) if first == second)
            {
                return Err(malformed(
                    "addr.cast source and target layout are incompatible",
                ));
            }
        }
        let ty = crate::text_type_disassembly::format_type_reference(
            types,
            target,
            &BinarySignatureTable::default(),
        )?;
        (format!("addr.cast %{source} to {ty}"), target)
    } else if let Some(target) = target {
        let Some(BinaryType::Pointer(space)) = types.entries.get(source_ty as usize) else {
            return Err(malformed("ptr.toaddr input must be pointer"));
        };
        let Some(BinaryType::Address { space: to, .. }) = types.entries.get(target as usize) else {
            return Err(malformed("ptr.toaddr target must be typed address"));
        };
        if to != space {
            return Err(malformed("ptr.toaddr must preserve pointer space"));
        }
        let ty = crate::text_type_disassembly::format_type_reference(
            types,
            target,
            &BinarySignatureTable::default(),
        )?;
        (format!("ptr.toaddr %{source} as {ty}"), target)
    } else {
        let Some(BinaryType::Address { space, .. }) = types.entries.get(source_ty as usize) else {
            return Err(malformed("addr.toptr input must be typed address"));
        };
        let result = types
            .entries
            .iter()
            .position(|ty| {
                matches!(ty,
            BinaryType::Pointer(ptr_space) if ptr_space == space)
            })
            .ok_or_else(|| malformed("addr.toptr pointer result type is not interned"))?;
        (format!("addr.toptr %{source}"), result as u64)
    };
    writeln!(text, "    %v{} = {code}", values.len())
        .map_err(|issue| malformed(issue.to_string()))?;
    values.push((format!("v{}", values.len()), result_ty));
    Ok(())
}

fn ssa(value: &str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    value
        .strip_prefix('%')
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .map(str::to_owned)
        .ok_or_else(|| {
            error(
                item,
                span,
                "pointer/address conversion requires SSA operand",
            )
        })
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
