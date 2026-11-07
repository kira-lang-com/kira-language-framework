//! Non-generic nominal enum switch CODE (0x1e3). Case payloads are passed as
//! implicit block parameters; the encoded case list is in variant-index order.
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::text_control_flow_assembly::{
    atom, expect_punctuation, item_error, parse_label, parse_value, punct,
};
use crate::types::{
    BinaryBlock, BinaryInstruction, BinaryNominalKind, BinaryOperand, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, FunctionBody, TextInstruction, TextItem, TextParseError};

#[derive(Clone, Debug)]
pub(crate) struct EnumSwitch {
    pub(crate) scrutinee: String,
    pub(crate) cases: Vec<(String, String)>,
    pub(crate) default: Option<String>,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    item: TextItem,
) -> Result<EnumSwitch, TextParseError> {
    if instruction.opcode != "switch.enum" || !instruction.results.is_empty() {
        return Err(item_error(
            item,
            instruction.span,
            "switch.enum is a zero-result terminator",
        ));
    }
    let tokens = &instruction.tokens;
    let span = instruction.span;
    let mut cursor = 3;
    let scrutinee = parse_value(tokens, &mut cursor, item, span)?;
    expect_punctuation(tokens, &mut cursor, ',', item, span)?;
    let mut cases = Vec::new();
    let mut default = None;
    while punct(tokens, cursor, '[') {
        cursor += 1;
        if atom(tokens, cursor) == Some("default") {
            cursor += 1;
            if default.is_some() {
                return Err(item_error(item, span, "duplicate enum switch default"));
            }
            default = Some(parse_label(tokens, &mut cursor, item, span)?);
        } else {
            if default.is_some() {
                return Err(item_error(item, span, "enum cases must precede default"));
            }
            let variant = atom(tokens, cursor)
                .ok_or_else(|| item_error(item, span, "switch.enum requires a named variant"))?
                .to_owned();
            cursor += 1;
            expect_punctuation(tokens, &mut cursor, ':', item, span)?;
            let target = parse_label(tokens, &mut cursor, item, span)?;
            cases.push((variant, target));
        }
        expect_punctuation(tokens, &mut cursor, ']', item, span)?;
        if punct(tokens, cursor, ',') {
            cursor += 1;
        }
    }
    if cursor != tokens.len() || (cases.is_empty() && default.is_none()) {
        return Err(item_error(
            item,
            span,
            "invalid switch.enum case/default list",
        ));
    }
    Ok(EnumSwitch {
        scrutinee,
        cases,
        default,
    })
}

pub(crate) fn validate(
    switch: &EnumSwitch,
    body: &FunctionBody,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    if !matches!(values.get(&switch.scrutinee), Some(Type::Nominal { .. })) {
        return Err(item_error(
            item,
            span,
            "switch.enum scrutinee must be a nominal enum value",
        ));
    }
    let mut names = HashSet::new();
    for (name, target) in &switch.cases {
        if !names.insert(name) {
            return Err(item_error(
                item,
                span,
                "switch.enum has repeated variant name",
            ));
        }
        if !body.blocks.iter().any(|block| block.label == *target) {
            return Err(item_error(
                item,
                span,
                "switch.enum case target is not declared",
            ));
        }
    }
    if let Some(default) = &switch.default
        && !body.blocks.iter().any(|block| block.label == *default)
    {
        return Err(item_error(
            item,
            span,
            "switch.enum default target is not declared",
        ));
    }
    Ok(())
}

fn enum_definition<'a>(
    types: &'a BinaryTypeTable,
    ty: u64,
) -> Option<(usize, &'a crate::BinaryNominalDefinition)> {
    let BinaryType::Nominal {
        declaration,
        arguments,
    } = types.entries.get(ty as usize)?
    else {
        return None;
    };
    if !arguments.is_empty() {
        return None;
    }
    let index = *declaration as usize;
    if types.declarations.get(index)?.kind != BinaryNominalKind::Enum {
        return None;
    }
    let definition = types
        .definitions
        .iter()
        .find(|definition| definition.declaration == *declaration)?;
    Some((index, definition))
}

pub(crate) fn encode(
    switch: &EnumSwitch,
    body: &FunctionBody,
    types: &BinaryTypeTable,
    parameter_indices: &[Vec<u64>],
    values: &[&str],
    labels: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let parameter = body
        .blocks
        .iter()
        .flat_map(|block| &block.parameters)
        .find(|parameter| parameter.name == switch.scrutinee)
        .ok_or_else(|| {
            item_error(
                item,
                span,
                "switch.enum currently requires an enum SSA block parameter",
            )
        })?;
    let Type::Nominal { path, arguments } = &parameter.ty else {
        return Err(item_error(
            item,
            span,
            "switch.enum requires a nominal enum",
        ));
    };
    if !arguments.is_empty() {
        return Err(item_error(
            item,
            span,
            "generic enum switches require type specialization",
        ));
    }
    let declaration = types
        .declarations
        .iter()
        .position(|decl| decl.name == *path && decl.kind == BinaryNominalKind::Enum)
        .ok_or_else(|| item_error(item, span, "switch.enum declaration is not an enum"))?;
    let definition = types
        .definitions
        .iter()
        .find(|def| def.declaration == declaration as u64)
        .ok_or_else(|| item_error(item, span, "switch.enum has no enum definition"))?;
    let mut cases = Vec::new();
    for (variant, target) in &switch.cases {
        let index = definition
            .variants
            .iter()
            .position(|entry| entry.name == *variant)
            .ok_or_else(|| item_error(item, span, "unknown enum switch variant"))?;
        let target_index = labels
            .iter()
            .position(|label| *label == target)
            .ok_or_else(|| item_error(item, span, "enum switch target is undeclared"))?;
        if parameter_indices.get(target_index) != Some(&definition.variants[index].payload) {
            return Err(item_error(
                item,
                span,
                "enum case block parameters differ from variant payload",
            ));
        }
        cases.push((index as u64, target_index as u64));
    }
    cases.sort_unstable_by_key(|(variant, _)| *variant);
    if cases.windows(2).any(|two| two[0].0 >= two[1].0) {
        return Err(item_error(item, span, "duplicate enum variant case"));
    }
    let default = switch
        .default
        .as_ref()
        .map(|name| {
            labels
                .iter()
                .position(|label| *label == name)
                .ok_or_else(|| item_error(item, span, "enum switch default target is undeclared"))
        })
        .transpose()?;
    if default.is_none() && cases.len() != definition.variants.len() {
        return Err(item_error(
            item,
            span,
            "nonexhaustive enum switch requires a default target",
        ));
    }
    if default.is_some_and(|index| {
        parameter_indices
            .get(index)
            .is_none_or(|parameters| !parameters.is_empty())
    }) {
        return Err(item_error(
            item,
            span,
            "enum switch default target must not take payload parameters",
        ));
    }
    let value = values
        .iter()
        .position(|name| *name == switch.scrutinee)
        .ok_or_else(|| item_error(item, span, "enum switch operand SSA value is undefined"))?;
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes)
            .map_err(|issue| item_error(item, span, issue.to_string()))
    };
    write(0x1e3)?;
    write(0)?;
    write(0)?;
    write((values.len() - value) as u64)?;
    write(cases.len() as u64)?;
    for (variant, target) in cases {
        write(variant)?;
        write(target)?;
    }
    write(default.map_or(0, |index| index as u64 + 1))?;
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &[(String, u64)],
    blocks: &[BinaryBlock],
) -> Result<(), FormatError> {
    if instruction.opcode != 0x1e3
        || instruction.result_count != 0
        || instruction.modifiers != 0
        || !instruction.terminator
    {
        return Err(malformed("invalid switch.enum result or modifiers"));
    }
    let [
        BinaryOperand::Value(value),
        BinaryOperand::SwitchEnumCases(cases),
        BinaryOperand::OptionalIndex(default),
    ] = instruction.operands.as_slice()
    else {
        return Err(malformed("switch.enum operand shape is invalid"));
    };
    let (name, ty) = usize::try_from(*value)
        .ok()
        .and_then(|index| values.get(index))
        .ok_or_else(|| malformed("switch.enum selector is undefined"))?;
    let (_, definition) = enum_definition(types, *ty)
        .ok_or_else(|| malformed("switch.enum selector has no nongeneric enum definition"))?;
    if default.is_none() && cases.len() != definition.variants.len() {
        return Err(malformed("nonexhaustive switch.enum lacks default"));
    }
    write!(text, "switch.enum %{name}").map_err(formatting_error)?;
    let mut prior = None;
    for (variant, target) in cases {
        if prior.is_some_and(|previous| previous >= variant) {
            return Err(malformed(
                "switch.enum case indexes are not strictly increasing",
            ));
        }
        prior = Some(variant);
        let item = definition
            .variants
            .get(*variant as usize)
            .ok_or_else(|| malformed("switch.enum variant index is out of range"))?;
        let block = blocks
            .get(*target as usize)
            .ok_or_else(|| malformed("switch.enum target is out of range"))?;
        if block
            .parameters
            .iter()
            .map(|param| param.ty)
            .collect::<Vec<_>>()
            != item.payload
        {
            return Err(malformed(
                "switch.enum target parameters disagree with variant payload",
            ));
        }
        write!(text, ", [{}: bb{target}]", item.name).map_err(formatting_error)?;
    }
    if let Some(default) = default {
        let block = blocks
            .get(*default as usize)
            .ok_or_else(|| malformed("switch.enum default is out of range"))?;
        if !block.parameters.is_empty() {
            return Err(malformed(
                "switch.enum default target has payload parameters",
            ));
        }
        write!(text, ", [default bb{default}]").map_err(formatting_error)?;
    }
    text.push('\n');
    Ok(())
}

fn formatting_error(issue: std::fmt::Error) -> FormatError {
    malformed(issue.to_string())
}
fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
