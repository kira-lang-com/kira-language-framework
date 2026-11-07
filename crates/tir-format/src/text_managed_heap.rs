//! Typed shared/unique reference allocation and access (TIR 12.9).
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{AddressSpace, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum ManagedHeap {
    Alloc {
        result: String,
        pointee: Type,
        unique: bool,
    },
    Access {
        result: String,
        reference: String,
        opcode: u64,
    },
}

pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "heap.alloc.counted"
            | "heap.alloc.unique"
            | "ref.addr"
            | "uniq.addr"
            | "weak.new"
            | "weak.load"
            | "ref.unowned"
            | "ref.isunique"
    )
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<ManagedHeap, TextParseError> {
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "managed heap opcode must define one SSA result",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "managed heap source span is invalid",
            )
        })?
        .trim();
    let prefix = format!("%{result} = {} ", instruction.opcode);
    let argument = line.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "managed heap opcode lacks type/value",
        )
    })?;
    match instruction.opcode.as_str() {
        "heap.alloc.counted" | "heap.alloc.unique" => {
            let pointee = crate::parse_type(argument).map_err(|issue| {
                error(
                    item,
                    instruction.span,
                    format!("invalid managed heap pointee type: {issue}"),
                )
            })?;
            if matches!(pointee, Type::Never | Type::Unit | Type::Code) {
                return Err(error(
                    item,
                    instruction.span,
                    "managed heap allocation needs a stored value type",
                ));
            }
            Ok(ManagedHeap::Alloc {
                result: result.clone(),
                pointee,
                unique: instruction.opcode == "heap.alloc.unique",
            })
        }
        "ref.addr" | "uniq.addr" | "weak.new" | "weak.load" | "ref.unowned" | "ref.isunique" => {
            let reference = argument
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
                        "managed heap reference must be an SSA name",
                    )
                })?;
            let opcode = match instruction.opcode.as_str() {
                "ref.addr" => 0x162,
                "uniq.addr" => 0x163,
                "weak.new" => 0x167,
                "weak.load" => 0x168,
                "ref.unowned" => 0x169,
                _ => 0x16a,
            };
            Ok(ManagedHeap::Access {
                result: result.clone(),
                reference: reference.to_owned(),
                opcode,
            })
        }
        _ => Err(error(
            item,
            instruction.span,
            "unsupported managed heap opcode",
        )),
    }
}

pub(crate) fn result_type(
    operation: &ManagedHeap,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    match operation {
        ManagedHeap::Alloc {
            result,
            pointee,
            unique,
        } => Ok((
            result.clone(),
            if *unique {
                Type::Unique(Box::new(pointee.clone()))
            } else {
                Type::Reference(Box::new(pointee.clone()))
            },
        )),
        ManagedHeap::Access {
            result,
            reference,
            opcode,
        } => {
            let value = values
                .get(reference)
                .ok_or_else(|| error(item, span, "managed heap reference is undefined"))?;
            let ty = match (*opcode, value) {
                (0x162, Type::Reference(pointee)) | (0x163, Type::Unique(pointee)) => {
                    Type::Address {
                        pointee: pointee.clone(),
                        space: Some(AddressSpace::Generic),
                    }
                }
                (0x167, Type::Reference(pointee)) => Type::Weak(pointee.clone()),
                (0x168, Type::Weak(pointee)) => Type::Nominal {
                    path: "Optional".to_owned(),
                    arguments: vec![Type::Reference(pointee.clone())],
                },
                (0x169, Type::Reference(_)) => Type::Pointer(AddressSpace::Generic),
                (0x16a, Type::Reference(_)) => Type::Integer(1),
                _ => {
                    return Err(error(
                        item,
                        span,
                        "managed heap opcode requires matching ref/uniq input type",
                    ));
                }
            };
            Ok((result.clone(), ty))
        }
    }
}

pub(crate) fn required_type(operation: &ManagedHeap) -> Option<Type> {
    match operation {
        ManagedHeap::Alloc {
            pointee, unique, ..
        } => Some(if *unique {
            Type::Unique(Box::new(pointee.clone()))
        } else {
            Type::Reference(Box::new(pointee.clone()))
        }),
        _ => None,
    }
}

pub(crate) fn encode(
    operation: &ManagedHeap,
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
            .map(|pos| (values.len() - pos) as u64)
            .ok_or_else(|| error(item, span, "managed heap SSA argument is undefined"))
    };
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match operation {
        ManagedHeap::Alloc {
            pointee, unique, ..
        } => {
            let index = crate::text_descriptor_reference::type_index(types, pointee)
                .ok_or_else(|| error(item, span, "managed heap pointee type is not interned"))?;
            write(if *unique { 0x161 } else { 0x160 })?;
            write(0)?;
            write(1)?;
            write(index as u64)?;
        }
        ManagedHeap::Access {
            reference, opcode, ..
        } => {
            if *opcode == 0x168 {
                validate_optional_policy(types).map_err(|message| error(item, span, message))?;
            }
            write(*opcode)?;
            write(0)?;
            write(1)?;
            write(delta(reference)?)?;
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
    if instruction.result_count != 1 || instruction.modifiers != 0 {
        return Err(malformed(
            "managed heap operation must define one SSA result without modifiers",
        ));
    }
    match (instruction.opcode, instruction.operands.as_slice()) {
        (0x160 | 0x161, [BinaryOperand::Type(pointee)]) => {
            let entry = types
                .entries
                .get(*pointee as usize)
                .ok_or_else(|| malformed("managed heap allocation type index is out of range"))?;
            if matches!(entry, BinaryType::Unit | BinaryType::Never) {
                return Err(malformed("managed heap allocation type has no storage"));
            }
            let expected = if instruction.opcode == 0x160 {
                BinaryType::Reference { pointee: *pointee }
            } else {
                BinaryType::Unique { pointee: *pointee }
            };
            let ty_index = types
                .entries
                .iter()
                .position(|ty| *ty == expected)
                .ok_or_else(|| malformed("managed heap result type was not interned"))?;
            let name = format!("v{}", values.len());
            let ty_name = crate::text_type_disassembly::format_type_reference(
                types,
                *pointee,
                &crate::types::BinarySignatureTable::default(),
            )?;
            let opcode = if instruction.opcode == 0x160 {
                "heap.alloc.counted"
            } else {
                "heap.alloc.unique"
            };
            writeln!(text, "    %{name} = {opcode} {ty_name}").map_err(formatting_error)?;
            values.push((name, ty_index as u64));
            return Ok(());
        }
        (0x162 | 0x163 | 0x167..=0x16a, [BinaryOperand::Value(reference)]) => {
            let (name, ty_index) = usize::try_from(*reference)
                .ok()
                .and_then(|index| values.get(index))
                .ok_or_else(|| malformed("managed heap SSA reference index is invalid"))?;
            let value_type = types
                .entries
                .get(*ty_index as usize)
                .ok_or_else(|| malformed("managed heap SSA type index is invalid"))?;
            let expected = match (instruction.opcode, value_type) {
                (0x162, BinaryType::Reference { pointee })
                | (0x163, BinaryType::Unique { pointee }) => BinaryType::Address {
                    pointee: *pointee,
                    space: 0,
                },
                (0x167, BinaryType::Reference { pointee }) => {
                    BinaryType::Weak { pointee: *pointee }
                }
                (0x168, BinaryType::Weak { pointee }) => {
                    let declaration = validate_optional_policy(types).map_err(malformed)?;
                    let reference = types.entries.iter().position(|entry| {
                        matches!(entry, BinaryType::Reference { pointee: actual } if actual == pointee)
                    }).ok_or_else(|| malformed("weak.load requires an interned strong ref type"))?;
                    BinaryType::Nominal {
                        declaration: declaration as u64,
                        arguments: vec![reference as u64],
                    }
                }
                (0x169, BinaryType::Reference { .. }) => BinaryType::Pointer(0),
                (0x16a, BinaryType::Reference { .. }) => BinaryType::Integer(1),
                _ => {
                    return Err(malformed(
                        "managed heap opcode has wrong source ref/uniq type",
                    ));
                }
            };
            let result_ty = types
                .entries
                .iter()
                .position(|ty| *ty == expected)
                .ok_or_else(|| malformed("managed heap access result type was not interned"))?;
            let opcode = match instruction.opcode {
                0x162 => "ref.addr",
                0x163 => "uniq.addr",
                0x167 => "weak.new",
                0x168 => "weak.load",
                0x169 => "ref.unowned",
                _ => "ref.isunique",
            };
            let result_name = format!("v{}", values.len());
            writeln!(text, "    %{result_name} = {opcode} %{name}").map_err(formatting_error)?;
            values.push((result_name, result_ty as u64));
            return Ok(());
        }
        _ => return Err(malformed("managed heap binary operand shape is invalid")),
    }
}

fn validate_optional_policy(types: &BinaryTypeTable) -> Result<usize, &'static str> {
    let declarations = types
        .declarations
        .iter()
        .enumerate()
        .filter(|(_, declaration)| {
            declaration.name == "Optional"
                && declaration.kind == crate::types::BinaryNominalKind::Enum
                && declaration.generic_parameters.len() == 1
        })
        .collect::<Vec<_>>();
    let [(index, _)] = declarations.as_slice() else {
        return Err("weak.load requires exactly one generic Optional enum declaration");
    };
    let Some(definition) = types
        .definitions
        .iter()
        .find(|definition| definition.declaration == *index as u64)
    else {
        return Err("weak.load requires a complete Optional enum definition");
    };
    if !matches!(
        definition.policy.copy,
        crate::types::BinaryCopyPolicy::Implicit(_)
    ) || !matches!(
        definition.policy.destroy,
        crate::types::BinaryDestroyPolicy::Function(_)
    ) || definition.policy.pairing != 0
    {
        return Err("weak.load Optional<ref T> needs a counted copy/destroy lifecycle policy");
    }
    Ok(*index)
}

fn formatting_error(issue: std::fmt::Error) -> FormatError {
    malformed(issue.to_string())
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
