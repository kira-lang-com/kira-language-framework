//! Fixed-vector gather/scatter CODE, in both pointer-vector and base/offset forms.
use std::collections::HashMap;
use std::fmt::Write as _;
use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

const MASK: u64 = 1 << 32;
const BASE: u64 = 1 << 48;

pub(crate) enum VectorGather {
    Gather {
        result: String,
        ty: Type,
        address: Address,
        mask: Option<(String, String)>,
    },
    Scatter {
        value: String,
        address: Address,
        mask: Option<String>,
    },
}

pub(crate) enum Address {
    PointerVector(String),
    Base {
        base: String,
        offsets: String,
        scale: u64,
    },
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(opcode, "vec.gather" | "vec.scatter")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<VectorGather, TextParseError> {
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid vector gather/scatter span"))?
        .trim();
    let (main, options) = text
        .split_once(" [")
        .map_or((text, None), |(text, options)| (text, Some(options)));
    let mask = options
        .map(|options| {
            options
                .strip_suffix(']')
                .and_then(|options| options.strip_prefix("mask "))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "unsupported gather/scatter mask clause",
                    )
                })
        })
        .transpose()?;
    if instruction.opcode == "vec.gather" {
        let [result] = instruction.results.as_slice() else {
            return Err(error(
                item,
                instruction.span,
                "vec.gather defines one SSA result",
            ));
        };
        let prefix = format!("%{result} = vec.gather ");
        let (ty, args) = main
            .strip_prefix(&prefix)
            .and_then(|args| args.split_once(", "))
            .ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "vec.gather requires a vector type and address",
                )
            })?;
        let ty = crate::parse_type(ty)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
        let mask = mask
            .map(|mask| {
                let (mask, pass) = mask.split_once(", passthru ").ok_or_else(|| {
                    error(item, instruction.span, "masked gather requires passthru")
                })?;
                Ok((
                    parse_name(mask, item, instruction.span)?,
                    parse_name(pass, item, instruction.span)?,
                ))
            })
            .transpose()?;
        Ok(VectorGather::Gather {
            result: result.clone(),
            ty,
            address: parse_address(args, item, instruction.span)?,
            mask,
        })
    } else if instruction.opcode == "vec.scatter" {
        if !instruction.results.is_empty() {
            return Err(error(
                item,
                instruction.span,
                "vec.scatter is a zero-result operation",
            ));
        }
        let (value, args) = main
            .strip_prefix("vec.scatter ")
            .and_then(|args| args.split_once(", "))
            .ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "vec.scatter requires value and address",
                )
            })?;
        Ok(VectorGather::Scatter {
            value: parse_name(value, item, instruction.span)?,
            address: parse_address(args, item, instruction.span)?,
            mask: mask
                .map(|mask| parse_name(mask, item, instruction.span))
                .transpose()?,
        })
    } else {
        Err(error(
            item,
            instruction.span,
            "unsupported vector gather/scatter opcode",
        ))
    }
}

fn parse_address(raw: &str, item: TextItem, span: Span) -> Result<Address, TextParseError> {
    let parts = raw.split(", ").collect::<Vec<_>>();
    match parts.as_slice() {
        [pointers] => Ok(Address::PointerVector(parse_name(pointers, item, span)?)),
        [base, offsets, scale] => {
            let scale = scale
                .parse::<u64>()
                .map_err(|_| error(item, span, "gather/scatter scale must be unsigned"))?;
            Ok(Address::Base {
                base: parse_name(base, item, span)?,
                offsets: parse_name(offsets, item, span)?,
                scale,
            })
        }
        _ => Err(error(
            item,
            span,
            "gather/scatter address must be pointer-vector or base, offsets, scale",
        )),
    }
}

pub(crate) fn load_type(operation: &VectorGather) -> Option<&Type> {
    match operation {
        VectorGather::Gather { ty, .. } => Some(ty),
        _ => None,
    }
}

pub(crate) fn validate(
    operation: &VectorGather,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    let source = |name: &str| {
        values
            .get(name)
            .ok_or_else(|| error(item, span, "gather/scatter SSA operand is undefined"))
    };
    let (vector, address, mask, result) = match operation {
        VectorGather::Gather {
            result,
            ty,
            address,
            mask,
        } => {
            if values.contains_key(result) {
                return Err(error(item, span, "duplicate vec.gather SSA result"));
            }
            (
                ty,
                address,
                mask.as_ref().map(|(mask, _)| mask.as_str()),
                Some((result.clone(), ty.clone())),
            )
        }
        VectorGather::Scatter {
            value,
            address,
            mask,
        } => (source(value)?, address, mask.as_deref(), None),
    };
    let Type::FixedVector { lanes, element } = vector else {
        return Err(error(
            item,
            span,
            "gather/scatter requires fixed-vector values",
        ));
    };
    if !matches!(element.as_ref(), Type::Integer(_) | Type::Float(_)) {
        return Err(error(
            item,
            span,
            "gather/scatter vector elements must be numeric",
        ));
    }
    match address {
        Address::PointerVector(name) => {
            if source(name)?
                != &(Type::FixedVector {
                    lanes: *lanes,
                    element: Box::new(Type::Pointer(tir_types::AddressSpace::Generic)),
                })
            {
                return Err(error(
                    item,
                    span,
                    "gather/scatter requires matching vector of generic pointers",
                ));
            }
        }
        Address::Base { base, offsets, .. } => {
            if !matches!(source(base)?, Type::Pointer(_) | Type::Address { .. }) {
                return Err(error(item, span, "gather/scatter base must be a pointer"));
            }
            let Some(Type::FixedVector {
                lanes: offset_lanes,
                element: offset_ty,
            }) = values.get(offsets)
            else {
                return Err(error(
                    item,
                    span,
                    "gather/scatter offsets must be a fixed integer vector",
                ));
            };
            if offset_lanes != lanes || !matches!(offset_ty.as_ref(), Type::Integer(_)) {
                return Err(error(
                    item,
                    span,
                    "gather/scatter offsets must match lanes and be integer",
                ));
            }
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
                "gather/scatter mask lanes or type do not match",
            ));
        }
    }
    if let VectorGather::Gather {
        mask: Some((_, pass)),
        ..
    } = operation
        && source(pass)? != vector
    {
        return Err(error(
            item,
            span,
            "gather passthru must match result vector type",
        ));
    }
    Ok(result)
}

pub(crate) fn encode(
    operation: &VectorGather,
    names: &[&str],
    types: &BinaryTypeTable,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let delta = |name: &str| {
        names
            .iter()
            .position(|value| *value == name)
            .map(|index| (names.len() - index) as u64)
            .ok_or_else(|| error(item, span, "undefined gather/scatter SSA operand"))
    };
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    let (opcode, address, masked) = match operation {
        VectorGather::Gather { address, mask, .. } => (0x0c9, address, mask.is_some()),
        VectorGather::Scatter { address, mask, .. } => (0x0ca, address, mask.is_some()),
    };
    let flags = if masked { MASK } else { 0 }
        | if matches!(address, Address::Base { .. }) {
            BASE
        } else {
            0
        };
    write(opcode)?;
    write(flags)?;
    write(if opcode == 0x0c9 { 1 } else { 0 })?;
    match operation {
        VectorGather::Gather { ty, .. } => {
            let index = crate::text_descriptor_reference::type_index(types, ty)
                .ok_or_else(|| error(item, span, "gather result type is not interned"))?;
            write(index as u64)?;
        }
        VectorGather::Scatter { value, .. } => write(delta(value)?)?,
    }
    match address {
        Address::PointerVector(ptrs) => write(delta(ptrs)?)?,
        Address::Base {
            base,
            offsets,
            scale,
        } => {
            write(delta(base)?)?;
            write(delta(offsets)?)?;
            write(*scale)?;
        }
    }
    match operation {
        VectorGather::Gather {
            mask: Some((mask, pass)),
            ..
        } => {
            write(delta(mask)?)?;
            write(delta(pass)?)?;
        }
        VectorGather::Scatter {
            mask: Some(mask), ..
        } => write(delta(mask)?)?,
        _ => {}
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.modifiers & !(MASK | BASE) != 0 {
        return Err(malformed("invalid gather/scatter modifier bits"));
    }
    let masked = instruction.modifiers & MASK != 0;
    let base_form = instruction.modifiers & BASE != 0;
    let mut ops = instruction.operands.iter();
    let target = if instruction.opcode == 0x0c9 {
        if instruction.result_count != 1 {
            return Err(malformed("gather must define one SSA result"));
        }
        let Some(BinaryOperand::Type(index)) = ops.next() else {
            return Err(malformed("gather lacks TYPES result index"));
        };
        *index
    } else if instruction.opcode == 0x0ca {
        if instruction.result_count != 0 {
            return Err(malformed("scatter cannot define a result"));
        }
        let Some(BinaryOperand::Value(value)) = ops.next() else {
            return Err(malformed("scatter requires value operand"));
        };
        values
            .get(*value as usize)
            .ok_or_else(|| malformed("scatter value is undefined"))?
            .1
    } else {
        return Err(malformed("unsupported gather/scatter opcode"));
    };
    let (lanes, element) = numeric_vector(types, target)?;
    let read_name = |index: u64| {
        values
            .get(index as usize)
            .map(|(name, ty)| (name.as_str(), *ty))
            .ok_or_else(|| malformed("gather/scatter SSA operand is undefined"))
    };
    let address = if base_form {
        let Some(BinaryOperand::Values(addresses)) = ops.next() else {
            return Err(malformed("base form requires two SSA operands"));
        };
        let [base, offsets] = addresses.as_slice() else {
            return Err(malformed("base form must have two addresses"));
        };
        let Some(BinaryOperand::Index(scale)) = ops.next() else {
            return Err(malformed("base form requires scale"));
        };
        let (base_name, base_ty) = read_name(*base)?;
        if !matches!(
            types.entries.get(base_ty as usize),
            Some(BinaryType::Pointer(_) | BinaryType::Address { .. })
        ) {
            return Err(malformed("gather/scatter base is not pointer"));
        }
        let (offset_name, offset_ty) = read_name(*offsets)?;
        let Some(BinaryType::FixedVector {
            lanes: count,
            element,
        }) = types.entries.get(offset_ty as usize)
        else {
            return Err(malformed("offsets must be integer vector"));
        };
        if *count != lanes
            || !matches!(
                types.entries.get(*element as usize),
                Some(BinaryType::Integer(_))
            )
        {
            return Err(malformed("offset vector has incompatible shape"));
        }
        format!("%{base_name}, %{offset_name}, {scale}")
    } else {
        let Some(BinaryOperand::Value(ptrs)) = ops.next() else {
            return Err(malformed("pointer-vector form requires one SSA operand"));
        };
        let (ptr_name, ptr_ty) = read_name(*ptrs)?;
        let Some(BinaryType::FixedVector {
            lanes: count,
            element,
        }) = types.entries.get(ptr_ty as usize)
        else {
            return Err(malformed("gather/scatter pointers must be fixed vector"));
        };
        if *count != lanes
            || !matches!(
                types.entries.get(*element as usize),
                Some(BinaryType::Pointer(0))
            )
        {
            return Err(malformed("gather/scatter pointers must match output lanes"));
        }
        format!("%{ptr_name}")
    };
    let mut suffix = String::new();
    if masked {
        if instruction.opcode == 0x0c9 {
            let Some(BinaryOperand::Values(mask_values)) = ops.next() else {
                return Err(malformed("masked gather requires mask and passthru"));
            };
            let [mask, passthru] = mask_values.as_slice() else {
                return Err(malformed("masked gather needs exactly two values"));
            };
            let (mask_name, mask_ty) = read_name(*mask)?;
            mask_ok(types, mask_ty, lanes)?;
            let (pass_name, pass_ty) = read_name(*passthru)?;
            if pass_ty != target {
                return Err(malformed("gather passthru type does not match"));
            }
            suffix.push_str(&format!(" [mask %{mask_name}, passthru %{pass_name}]"));
        } else {
            let Some(BinaryOperand::Value(mask)) = ops.next() else {
                return Err(malformed("masked scatter needs mask value"));
            };
            let (mask_name, mask_ty) = read_name(*mask)?;
            mask_ok(types, mask_ty, lanes)?;
            suffix.push_str(&format!(" [mask %{mask_name}]"));
        }
    }
    if ops.next().is_some() {
        return Err(malformed("gather/scatter has trailing binary operands"));
    }
    let line = if instruction.opcode == 0x0c9 {
        let type_text = crate::text_memory_disassembly::format_storage_type(types, target)?;
        format!(
            "    %v{} = vec.gather {type_text}, {address}{suffix}",
            values.len()
        )
    } else {
        let Some(BinaryOperand::Value(value)) = instruction.operands.first() else {
            unreachable!()
        };
        let (name, _) = read_name(*value)?;
        format!("    vec.scatter %{name}, {address}{suffix}")
    };
    writeln!(text, "{line}").map_err(|issue| malformed(issue.to_string()))?;
    if instruction.opcode == 0x0c9 {
        values.push((format!("v{}", values.len()), target));
    }
    let _ = element;
    Ok(())
}

fn numeric_vector(types: &BinaryTypeTable, index: u64) -> Result<(u64, u64), FormatError> {
    let Some(BinaryType::FixedVector { lanes, element }) = types.entries.get(index as usize) else {
        return Err(malformed("gather/scatter requires fixed vector"));
    };
    if !matches!(
        types.entries.get(*element as usize),
        Some(BinaryType::Integer(_) | BinaryType::Float(_))
    ) {
        return Err(malformed("gather/scatter values must be numeric"));
    }
    Ok((*lanes, *element))
}
fn mask_ok(types: &BinaryTypeTable, mask: u64, lanes: u64) -> Result<(), FormatError> {
    let Some(BinaryType::FixedVector {
        lanes: count,
        element,
    }) = types.entries.get(mask as usize)
    else {
        return Err(malformed("gather/scatter mask must be fixed vector"));
    };
    if *count != lanes
        || !matches!(
            types.entries.get(*element as usize),
            Some(BinaryType::Integer(1))
        )
    {
        return Err(malformed("gather/scatter mask shape disagrees with vector"));
    }
    Ok(())
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
        .ok_or_else(|| error(item, span, "gather/scatter expects SSA values"))
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
