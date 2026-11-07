use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::leb::IntegerImmediate;
use crate::text_control_flow_assembly::{
    ControlTarget, atom, expect_atom, expect_punctuation, item_error, parse_label, parse_target,
    parse_value, punct, validate_target_arguments,
};
use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, FunctionBody, TextInstruction, TextItem, TextParseError};

#[derive(Clone, Debug)]
pub(crate) struct SwitchInt {
    pub(crate) scrutinee: String,
    pub(crate) cases: Vec<(IntegerImmediate, ControlTarget)>,
    pub(crate) default: ControlTarget,
    pub(crate) merge: Option<String>,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    item: TextItem,
) -> Result<SwitchInt, TextParseError> {
    if instruction.opcode != "switch.int" || !instruction.results.is_empty() {
        return Err(item_error(
            item,
            instruction.span,
            "switch.int is a zero-result terminator",
        ));
    }
    let tokens = &instruction.tokens;
    let span = instruction.span;
    let mut cursor = 3;
    let scrutinee = parse_value(tokens, &mut cursor, item, span)?;
    expect_punctuation(tokens, &mut cursor, ',', item, span)?;
    let mut cases = Vec::new();
    while punct(tokens, cursor, '[') {
        cursor += 1;
        let negative = if punct(tokens, cursor, '-') {
            cursor += 1;
            true
        } else {
            if punct(tokens, cursor, '+') {
                cursor += 1;
            }
            false
        };
        let number = atom(tokens, cursor)
            .ok_or_else(|| item_error(item, span, "switch.int case requires an integer literal"))?;
        let magnitude = crate::numeric_literals::parse_unsigned_integer(number)
            .ok_or_else(|| item_error(item, span, "switch.int case integer is invalid"))?;
        if negative && (magnitude == 0 || magnitude > (1u128 << 127)) {
            return Err(item_error(
                item,
                span,
                "switch.int case negative magnitude is invalid",
            ));
        }
        cursor += 1;
        expect_punctuation(tokens, &mut cursor, ':', item, span)?;
        let target = parse_target(tokens, &mut cursor, item, span)?;
        expect_punctuation(tokens, &mut cursor, ']', item, span)?;
        cases.push((
            IntegerImmediate {
                negative,
                magnitude,
            },
            target,
        ));
        if punct(tokens, cursor, ',') {
            cursor += 1;
        }
    }
    expect_atom(tokens, &mut cursor, "default", item, span)?;
    let default = parse_target(tokens, &mut cursor, item, span)?;
    let merge = if punct(tokens, cursor, '[') {
        cursor += 1;
        expect_atom(tokens, &mut cursor, "merge", item, span)?;
        let label = parse_label(tokens, &mut cursor, item, span)?;
        expect_punctuation(tokens, &mut cursor, ']', item, span)?;
        Some(label)
    } else {
        None
    };
    if cursor != tokens.len() {
        return Err(item_error(
            item,
            span,
            "unexpected switch.int terminator operands",
        ));
    }
    Ok(SwitchInt {
        scrutinee,
        cases,
        default,
        merge,
    })
}

pub(crate) fn validate(
    switch: &SwitchInt,
    body: &FunctionBody,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let Type::Integer(width) = values
        .get(&switch.scrutinee)
        .ok_or_else(|| item_error(item, span, "switch.int discriminator has no SSA definition"))?
    else {
        return Err(item_error(
            item,
            span,
            "switch.int discriminator must be an integer",
        ));
    };
    if !(1..=128).contains(width) {
        return Err(item_error(
            item,
            span,
            "switch.int case width is unsupported",
        ));
    }
    let mut seen = HashSet::new();
    for (literal, target) in &switch.cases {
        if !fits(*literal, *width) {
            return Err(item_error(
                item,
                span,
                "switch.int case does not fit discriminator width",
            ));
        }
        if !seen.insert(bits(*literal, *width)) {
            return Err(item_error(
                item,
                span,
                "switch.int has duplicate case values",
            ));
        }
        validate_target_arguments(body, values, target, item, span)?;
    }
    validate_target_arguments(body, values, &switch.default, item, span)?;
    if let Some(merge) = &switch.merge
        && !body.blocks.iter().any(|block| block.label == *merge)
    {
        return Err(item_error(
            item,
            span,
            "switch.int merge block is not declared",
        ));
    }
    Ok(())
}

fn bits(value: IntegerImmediate, width: u16) -> u128 {
    let value = if value.negative {
        value.magnitude.wrapping_neg()
    } else {
        value.magnitude
    };
    if width == 128 {
        value
    } else {
        value & ((1u128 << width) - 1)
    }
}

fn fits(value: IntegerImmediate, width: u16) -> bool {
    let literal = format!(
        "{}{}",
        if value.negative { "-" } else { "" },
        value.magnitude
    );
    crate::numeric_literals::integer_literal_fits(&literal, width)
}

pub(crate) fn encode(
    switch: &SwitchInt,
    values: &[&str],
    labels: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    use crate::text_control_flow_encoding::{
        target_index, write_target, write_uleb, write_value_delta,
    };
    let current_value = values.len() as u64;
    write_uleb(0x1e2, bytes, item, span)?;
    write_uleb(0, bytes, item, span)?;
    write_uleb(0, bytes, item, span)?;
    write_value_delta(values, &switch.scrutinee, current_value, bytes, item, span)?;
    write_uleb(switch.cases.len() as u64, bytes, item, span)?;
    for (literal, target) in &switch.cases {
        crate::leb::write_integer_immediate(literal.negative, literal.magnitude, bytes)
            .map_err(|error| item_error(item, span, error.to_string()))?;
        write_target(values, labels, current_value, target, bytes, item, span)?;
    }
    write_target(
        values,
        labels,
        current_value,
        &switch.default,
        bytes,
        item,
        span,
    )?;
    let merge = if let Some(merge) = &switch.merge {
        target_index(labels, merge, item, span)? + 1
    } else {
        0
    };
    write_uleb(merge, bytes, item, span)
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &[(String, u64)],
) -> Result<(), FormatError> {
    use crate::text_control_flow_disassembly::{format_target, value_name};
    if instruction.opcode != 0x1e2
        || instruction.result_count != 0
        || !instruction.terminator
        || instruction.modifiers != 0
    {
        return Err(malformed("invalid switch.int terminator"));
    }
    let [
        BinaryOperand::Value(discriminator),
        BinaryOperand::SwitchCases(cases),
        BinaryOperand::BlockTarget {
            block: default,
            arguments: default_args,
        },
        BinaryOperand::Index(merge),
    ] = instruction.operands.as_slice()
    else {
        return Err(malformed("switch.int operand shape is invalid"));
    };
    let ty = values
        .get(*discriminator as usize)
        .and_then(|(_, ty)| types.entries.get(*ty as usize));
    let Some(BinaryType::Integer(width)) = ty else {
        return Err(malformed("switch.int discriminator must be an integer"));
    };
    let width = u16::try_from(*width).map_err(|_| malformed("switch.int width is unsupported"))?;
    if !(1..=128).contains(&width) {
        return Err(malformed("switch.int width is unsupported"));
    }
    write!(text, "switch.int %{}", value_name(*discriminator, values)?)
        .map_err(formatting_error)?;
    let mut seen = HashSet::new();
    for (case, block, args) in cases {
        if !fits(*case, width) || !seen.insert(bits(*case, width)) {
            return Err(malformed(
                "switch.int case duplicates or exceeds discriminator width",
            ));
        }
        write!(text, ", [{}: ", literal_text(*case)).map_err(formatting_error)?;
        format_target(text, *block, args, values)?;
        text.push(']');
    }
    text.push_str(", default ");
    format_target(text, *default, default_args, values)?;
    if *merge != 0 {
        write!(text, " [merge bb{}]", merge - 1).map_err(formatting_error)?;
    }
    text.push('\n');
    Ok(())
}

fn literal_text(value: IntegerImmediate) -> String {
    format!(
        "{}{}",
        if value.negative { "-" } else { "" },
        value.magnitude
    )
}
fn formatting_error(issue: std::fmt::Error) -> FormatError {
    malformed(issue.to_string())
}
fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
