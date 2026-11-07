use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{AddressSpace, Type};

use crate::text_direct_call::Callees;
use crate::types::{
    BinaryGlobalTable, BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct GlobalAddress {
    result: String,
    name: String,
    opcode: u64,
}

pub(crate) fn handles(name: &str) -> bool {
    matches!(name, "global.addr" | "tls.addr")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<GlobalAddress, TextParseError> {
    let opcode = match instruction.opcode.as_str() {
        "global.addr" => 0x00b,
        "tls.addr" => 0x00c,
        _ => {
            return Err(error(
                item,
                instruction.span,
                "expected global.addr or tls.addr",
            ));
        }
    };
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "global address reference defines one SSA value",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "global address source span is invalid",
            )
        })?
        .trim();
    let prefix = format!("%{result} = {} @", instruction.opcode);
    let name = line
        .strip_prefix(&prefix)
        .filter(|name| {
            !name.is_empty()
                && !name.starts_with('.')
                && !name.ends_with('.')
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "global address requires a declared @global",
            )
        })?;
    Ok(GlobalAddress {
        result: result.clone(),
        name: name.to_owned(),
        opcode,
    })
}

pub(crate) fn validate(
    operation: &GlobalAddress,
    values: &HashMap<String, Type>,
    symbols: &Callees,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    if values.contains_key(&operation.result) {
        return Err(error(item, span, "duplicate global address SSA value"));
    }
    let global = symbols.globals.get(&operation.name).ok_or_else(|| {
        error(
            item,
            span,
            "global address symbol is not declared in GLOBALS",
        )
    })?;
    if operation.opcode == 0x00c && !global.thread_local {
        return Err(error(
            item,
            span,
            "tls.addr requires a thread_local GLOBALS declaration",
        ));
    }
    Ok((
        operation.result.clone(),
        Type::Pointer(AddressSpace::Generic),
    ))
}

pub(crate) fn encode(
    operation: &GlobalAddress,
    symbols: &Callees,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let global = symbols
        .globals
        .get(&operation.name)
        .ok_or_else(|| error(item, span, "global address symbol is undeclared"))?;
    let symbol = global
        .index
        .checked_mul(4)
        .and_then(|index| index.checked_add(1))
        .ok_or_else(|| error(item, span, "GLOBALS index overflows tagged symbol format"))?;
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(operation.opcode)?;
    write(0)?;
    write(1)?;
    write(symbol)?;
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    globals: &BinaryGlobalTable,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let mnemonic = match instruction.opcode {
        0x00b => "global.addr",
        0x00c => "tls.addr",
        _ => return Err(malformed("unsupported global address opcode")),
    };
    if instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed(
            "global address reference must define one SSA value without modifiers",
        ));
    }
    let [BinaryOperand::Symbol { kind: 1, index }] = instruction.operands.as_slice() else {
        return Err(malformed(
            "global address reference requires one GLOBALS symbol",
        ));
    };
    let global = usize::try_from(*index)
        .ok()
        .and_then(|index| globals.records.get(index))
        .ok_or_else(|| malformed("global address references an invalid GLOBALS index"))?;
    if instruction.opcode == 0x00c && !global.thread_local {
        return Err(malformed(
            "tls.addr references a global without thread_local",
        ));
    }
    let ty = types
        .entries
        .iter()
        .position(|ty| matches!(ty, BinaryType::Pointer(0)))
        .ok_or_else(|| malformed("global address pointer type is not interned"))?;
    let name = format!("v{}", values.len());
    writeln!(text, "    %{name} = {mnemonic} @{}", global.name)
        .map_err(|issue| malformed(issue.to_string()))?;
    values.push((name, ty as u64));
    Ok(())
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
