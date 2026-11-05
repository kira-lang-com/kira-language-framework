//! Conservative text lowering for direct scalar stack and address operations.
//! Dynamic allocation sizes, named address spaces, and raw memory are deferred
//! until their full canonical encodings can be verified.
use std::collections::HashMap;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryType, BinaryTypeTable};
use crate::{TextInstruction, TextItem, TextParseError};

pub(crate) enum MemoryOperation {
    StackAlloc {
        ty: Type,
    },
    StackDealloc {
        address: String,
    },
    Load {
        opcode: u64,
        selector: u64,
        address: String,
    },
    Store {
        opcode: u64,
        selector: u64,
        source: String,
        destination: String,
    },
    Destroy {
        address: String,
    },
}

impl MemoryOperation {
    pub(crate) fn result_type(
        &self,
        values: &HashMap<String, Type>,
        item: TextItem,
        span: Span,
    ) -> Result<Option<Type>, TextParseError> {
        let address_pointee = |address: &str| match values.get(address) {
            Some(Type::Address { pointee, .. }) => Ok(pointee.as_ref().clone()),
            _ => Err(error(
                item,
                span,
                format!("%{address} is not a defined address"),
            )),
        };
        match self {
            Self::StackAlloc { ty } => Ok(Some(Type::Address {
                pointee: Box::new(ty.clone()),
                space: None,
            })),
            Self::StackDealloc { address } | Self::Destroy { address } => {
                address_pointee(address)?;
                Ok(None)
            }
            Self::Load { address, .. } => Ok(Some(address_pointee(address)?)),
            Self::Store {
                opcode,
                source,
                destination,
                ..
            } => {
                let pointee = address_pointee(destination)?;
                let source_type = if *opcode == 0x142 {
                    values.get(source).cloned()
                } else {
                    Some(address_pointee(source)?)
                };
                if source_type.as_ref() != Some(&pointee) {
                    return Err(error(
                        item,
                        span,
                        "memory source type does not match destination pointee",
                    ));
                }
                Ok(None)
            }
        }
    }

    pub(crate) fn encode(
        &self,
        values: &[&str],
        types: &BinaryTypeTable,
        output: &mut Vec<u8>,
        item: TextItem,
        span: Span,
    ) -> Result<(), TextParseError> {
        let (opcode, selector, result_count) = match self {
            Self::StackAlloc { .. } => (0x120, 0, 1),
            Self::StackDealloc { .. } => (0x121, 0, 0),
            Self::Load {
                opcode, selector, ..
            } => (*opcode, *selector, 1),
            Self::Store {
                opcode, selector, ..
            } => (*opcode, *selector, 0),
            Self::Destroy { .. } => (0x145, 0, 0),
        };
        let write = |value, output: &mut Vec<u8>| {
            crate::leb::write_uleb(value, output).map_err(|err| {
                error(
                    item,
                    span,
                    format!("memory instruction encoding failed: {err}"),
                )
            })
        };
        write(opcode, output)?;
        write(selector << 48, output)?;
        write(result_count, output)?;
        let delta = |name: &str| -> Result<u64, TextParseError> {
            let position = values
                .iter()
                .position(|candidate| *candidate == name)
                .ok_or_else(|| {
                    error(item, span, format!("undefined SSA memory operand %{name}"))
                })?;
            Ok((values.len() - position) as u64)
        };
        match self {
            Self::StackAlloc {
                ty: Type::Integer(width),
            } => {
                let index = types.entries.iter().position(|type_entry|
                    matches!(type_entry, BinaryType::Integer(bits) if *bits == u64::from(*width)))
                    .ok_or_else(|| error(item, span, "stack allocation's integer type is not interned"))?;
                write(index as u64, output)?;
                write(0, output)?; // no dynamic count
            }
            Self::StackAlloc { .. } => {
                return Err(error(
                    item,
                    span,
                    "only integer stack allocation types are currently supported",
                ));
            }
            Self::StackDealloc { address }
            | Self::Destroy { address }
            | Self::Load { address, .. } => write(delta(address)?, output)?,
            Self::Store {
                source,
                destination,
                ..
            } => {
                write(delta(source)?, output)?;
                write(delta(destination)?, output)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(
        opcode,
        "stack.alloc"
            | "stack.dealloc"
            | "load"
            | "load.borrow"
            | "store"
            | "copy.addr"
            | "move.addr"
            | "destroy.addr"
    )
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<MemoryOperation, TextParseError> {
    let span = instruction.span;
    let raw = source
        .get(span.start..span.end)
        .ok_or_else(|| error(item, span, "invalid source span for memory instruction"))?;
    let text = raw.trim();
    let statement = if let Some((left, right)) = text.split_once(" = ") {
        if instruction.results.as_slice() != [left.trim().trim_start_matches('%')]
            || !left.trim().starts_with('%')
        {
            return Err(error(
                item,
                span,
                "memory result name does not match SSA declaration",
            ));
        }
        right
    } else {
        if !instruction.results.is_empty() {
            return Err(error(item, span, "memory instruction result is malformed"));
        }
        text
    };
    let (opcode, operands) = statement.split_once(' ').unwrap_or((statement, ""));
    if opcode != instruction.opcode {
        return Err(error(item, span, "unexpected memory opcode"));
    }
    let operand = |argument: &str| parse_ssa(argument, item, span);
    let result_expected = matches!(opcode, "stack.alloc" | "load" | "load.borrow");
    if result_expected != (instruction.results.len() == 1) {
        return Err(error(
            item,
            span,
            "memory instruction has the wrong SSA result count",
        ));
    }
    match opcode {
        "stack.alloc" => {
            let ty = crate::parse_type(operands.trim()).map_err(|err| {
                error(item, span, format!("invalid stack allocation type: {err}"))
            })?;
            if !matches!(ty, Type::Integer(_)) {
                return Err(error(
                    item,
                    span,
                    "only integer stack allocation is implemented",
                ));
            }
            Ok(MemoryOperation::StackAlloc { ty })
        }
        "stack.dealloc" => Ok(MemoryOperation::StackDealloc {
            address: operand(operands)?,
        }),
        "destroy.addr" => Ok(MemoryOperation::Destroy {
            address: operand(operands)?,
        }),
        "load" | "load.borrow" => {
            let (selector, address) = if opcode == "load.borrow" {
                (0, operand(operands)?)
            } else {
                let (mode, rest) = take_mode(
                    operands,
                    "trivial",
                    &["trivial", "copy", "take"],
                    item,
                    span,
                )?;
                let index = match mode {
                    "trivial" => 0,
                    "copy" => 1,
                    "take" => 2,
                    _ => unreachable!(),
                };
                (index, operand(rest)?)
            };
            Ok(MemoryOperation::Load {
                opcode: if opcode == "load" { 0x140 } else { 0x141 },
                selector,
                address,
            })
        }
        "store" | "copy.addr" | "move.addr" => {
            let (source, destination_with_mode) = operands
                .split_once(" to ")
                .ok_or_else(|| error(item, span, "memory transfer requires `to`"))?;
            let (selector, source, destination) = if opcode == "store" {
                let (mode, rest) = take_mode(
                    source,
                    "trivial",
                    &["trivial", "init", "assign"],
                    item,
                    span,
                )?;
                (
                    match mode {
                        "trivial" => 0,
                        "init" => 1,
                        "assign" => 2,
                        _ => unreachable!(),
                    },
                    operand(rest)?,
                    operand(destination_with_mode)?,
                )
            } else {
                let (mode, rest) = take_mode(
                    destination_with_mode,
                    "init",
                    &["init", "assign"],
                    item,
                    span,
                )?;
                (
                    if mode == "init" { 0 } else { 1 },
                    operand(source)?,
                    operand(rest)?,
                )
            };
            Ok(MemoryOperation::Store {
                opcode: match opcode {
                    "store" => 0x142,
                    "copy.addr" => 0x143,
                    _ => 0x144,
                },
                selector,
                source,
                destination,
            })
        }
        _ => Err(error(item, span, "not a supported memory instruction")),
    }
}

fn take_mode<'a>(
    text: &'a str,
    default: &'static str,
    allowed: &[&'static str],
    item: TextItem,
    span: Span,
) -> Result<(&'static str, &'a str), TextParseError> {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix('[') {
        let (mode, after) = rest
            .split_once(']')
            .ok_or_else(|| error(item, span, "memory mode is not closed"))?;
        let Some(known) = allowed.iter().find(|known| **known == mode) else {
            return Err(error(item, span, "unsupported memory ownership selector"));
        };
        Ok((known, after.trim()))
    } else {
        Ok((default, trimmed))
    }
}

fn parse_ssa(text: &str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    let text = text.trim();
    let Some(name) = text.strip_prefix('%') else {
        return Err(error(item, span, "memory operand must be an SSA value"));
    };
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '.'))
    {
        return Err(error(item, span, "malformed memory SSA operand"));
    }
    Ok(name.to_owned())
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

#[cfg(test)]
mod tests {
    use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

    fn source(body: &str) -> String {
        format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.memory(%v: i32) -> i32 engine(native) {{\nbb0(%v: i32):\n{body}\n}}\n"
        )
    }

    fn round_trip(body: &str) {
        let parsed = parse_text_module(&source(body)).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("memory CODE assembles");
        let module = BinaryModule::decode(&bytes).expect("binary decodes");
        let text = disassemble_text_module(&module, u64::MAX).expect("memory CODE disassembles");
        let rebuilt =
            assemble_text_module(&parse_text_module(&text).expect("canonical text parses"))
                .expect("canonical text reassembles");
        assert_eq!(bytes, rebuilt);
    }

    #[test]
    fn stack_alloc_store_load_and_dealloc_round_trip() {
        round_trip(
            "  %slot = stack.alloc i32\n  store [init] %v to %slot\n  %loaded = load [trivial] %slot\n  stack.dealloc %slot\n  return %loaded",
        );
    }

    #[test]
    fn copy_addr_and_move_addr_round_trip() {
        round_trip(
            "  %src = stack.alloc i32\n  %dest = stack.alloc i32\n  store [init] %v to %src\n  copy.addr %src to [init] %dest\n  %loaded = load [take] %dest\n  destroy.addr %src\n  stack.dealloc %dest\n  stack.dealloc %src\n  return %loaded",
        );
        round_trip(
            "  %src = stack.alloc i32\n  %dest = stack.alloc i32\n  store [init] %v to %src\n  move.addr %src to [init] %dest\n  %loaded = load [take] %dest\n  stack.dealloc %dest\n  stack.dealloc %src\n  return %loaded",
        );
    }

    #[test]
    fn wrong_store_pointee_type_is_rejected() {
        let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.bad(%v: i1) -> unit engine(native) {\nbb0(%v: i1):\n  %slot = stack.alloc i32\n  store [init] %v to %slot\n  stack.dealloc %slot\n  return\n}\n";
        let parsed = parse_text_module(source).expect("text parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "wrong-typed store must not assemble"
        );
    }
}
