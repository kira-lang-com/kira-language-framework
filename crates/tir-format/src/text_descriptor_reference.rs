//! Typed RIDL descriptor reference CODE (0x00d).
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{AddressSpace, Type};

use crate::types::{
    BinaryInstruction, BinaryOperand, BinarySignatureTable, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct DescriptorReference {
    pub(crate) result: String,
    pub(crate) ty: Type,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<DescriptorReference, TextParseError> {
    if instruction.opcode != "descriptor.ref" {
        return Err(error(item, instruction.span, "expected descriptor.ref"));
    }
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "descriptor.ref defines exactly one SSA value",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid descriptor.ref source span"))?
        .trim();
    let prefix = format!("%{result} = descriptor.ref ");
    let value = line.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "descriptor.ref requires an explicit type",
        )
    })?;
    let ty = crate::parse_type(value).map_err(|issue| {
        error(
            item,
            instruction.span,
            format!("invalid descriptor type: {issue}"),
        )
    })?;
    Ok(DescriptorReference {
        result: result.clone(),
        ty,
    })
}

pub(crate) fn validate(
    reference: &DescriptorReference,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    if values.contains_key(&reference.result) {
        return Err(error(item, span, "duplicate descriptor.ref SSA definition"));
    }
    Ok((
        reference.result.clone(),
        Type::Pointer(AddressSpace::Generic),
    ))
}

pub(crate) fn encode(
    reference: &DescriptorReference,
    types: &BinaryTypeTable,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let type_index = type_index(types, &reference.ty).ok_or_else(|| {
        error(
            item,
            span,
            "descriptor.ref target type has no representable TYPES entry",
        )
    })?;
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(0x00d)?;
    write(0)?;
    write(1)?;
    write(type_index as u64)?;
    Ok(())
}

pub(crate) fn type_index(types: &BinaryTypeTable, target: &Type) -> Option<usize> {
    let empty_signatures = BinarySignatureTable::default();
    types.entries.iter().enumerate().find_map(|(index, _)| {
        let spelling = crate::text_type_disassembly::format_type_reference(
            types,
            index as u64,
            &empty_signatures,
        )
        .ok()?;
        (crate::parse_type(&spelling).ok().as_ref() == Some(target)).then_some(index)
    })
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.opcode != 0x00d || instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed(
            "descriptor.ref requires zero modifiers and one SSA result",
        ));
    }
    let [BinaryOperand::Type(type_index)] = instruction.operands.as_slice() else {
        return Err(malformed(
            "descriptor.ref must have exactly one TYPES operand",
        ));
    };
    let ty = crate::text_type_disassembly::format_type_reference(types, *type_index, signatures)?;
    crate::parse_type(&ty)
        .map_err(|_| malformed("descriptor.ref type is not representable by text CODE"))?;
    let ptr = types
        .entries
        .iter()
        .position(|ty| matches!(ty, BinaryType::Pointer(0)))
        .ok_or_else(|| malformed("descriptor.ref result pointer type is not interned"))?;
    let name = format!("v{}", values.len());
    writeln!(text, "    %{name} = descriptor.ref {ty}")
        .map_err(|issue| malformed(issue.to_string()))?;
    values.push((name, ptr as u64));
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
