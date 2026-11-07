use crate::text::{Token, TokenKind};
use crate::text_function_body_helpers::simple_block_header;
use crate::{FunctionBody, TextItem, TextParseError};
use tir_diag::Span;
use tir_types::{FunctionType, Type};

#[derive(Clone, Debug)]
pub(crate) enum ControlTerminator {
    Branch(ControlTarget),
    Conditional {
        condition: String,
        when_true: ControlTarget,
        when_false: ControlTarget,
        merge: Option<String>,
    },
    Return(Option<String>),
    Fail(String),
    Unreachable {
        undefined_behavior: bool,
    },
    Trap(u16),
    Longjmp(crate::text_longjmp::Longjmp),
    TailCall(crate::text_direct_call::DirectCall),
    Switch(crate::text_switch_int::SwitchInt),
    EnumSwitch(crate::text_switch_enum::EnumSwitch),
    Indirect(crate::text_indirect_branch::IndirectBranch),
}

#[derive(Clone, Debug)]
pub(crate) struct ControlTarget {
    pub(crate) label: String,
    pub(crate) arguments: Vec<String>,
}

pub(crate) fn uses_supported_control_flow(body: &FunctionBody) -> bool {
    body.blocks.len() > 1
        || body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| instruction.opcode == "block.addr")
        || body.blocks.iter().any(|block| {
            block.instructions.last().is_some_and(|instruction| {
                matches!(
                    instruction.opcode.as_str(),
                    "br" | "cond.br"
                        | "switch.int"
                        | "switch.enum"
                        | "unreachable"
                        | "trap"
                        | "tail.call"
                        | "longjmp"
                        | "fail"
                        | "br.indirect"
                )
            })
        })
}

pub(crate) fn validate_control_flow_body(
    body: &FunctionBody,
    signature: &FunctionType,
    source: &str,
    item: TextItem,
    callees: Option<&crate::text_direct_call::Callees>,
) -> Result<(), TextParseError> {
    if body.blocks.is_empty() {
        return Err(item_error(item, body.span, "function body has no blocks"));
    }
    let entry = &body.blocks[0];
    if entry.label != "bb0"
        || entry.parameters.len() != signature.parameters.len()
        || entry
            .parameters
            .iter()
            .zip(&signature.parameters)
            .any(|(block, parameter)| {
                block.context != parameter.context
                    || block.convention != parameter.convention
                    || block.ty != parameter.ty
            })
    {
        return Err(item_error(
            item,
            entry.span,
            "entry block parameters do not match the function signature",
        ));
    }

    for block in &body.blocks {
        if !simple_block_header(block) {
            return Err(item_error(
                item,
                block.span,
                "text control-flow assembly does not support block annotations",
            ));
        }
        if block.instructions.is_empty() {
            return Err(item_error(
                item,
                block.span,
                "control-flow block has no terminator",
            ));
        }
    }

    let labels = body
        .blocks
        .iter()
        .map(|block| block.label.as_str())
        .collect::<Vec<_>>();
    if labels
        .iter()
        .enumerate()
        .any(|(index, label)| labels[..index].contains(label))
    {
        return Err(item_error(item, body.span, "duplicate basic block label"));
    }
    let terminators = body
        .blocks
        .iter()
        .map(|block| {
            parse_terminator(
                block
                    .instructions
                    .last()
                    .expect("all blocks have terminators"),
                signature.result.as_ref() == &Type::Unit,
                source,
                item,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let dominance = crate::text_control_flow_dominators::FlowDominators::new(&terminators, &labels);
    let mut prior_definitions: Vec<(String, Type, usize)> = Vec::new();
    let mut value_names = Vec::new();
    for block in &body.blocks {
        for parameter in &block.parameters {
            if value_names.contains(&parameter.name.as_str()) {
                return Err(item_error(item, parameter.span, "duplicate SSA value name"));
            }
            value_names.push(parameter.name.as_str());
        }
    }

    for (block_index, block) in body.blocks.iter().enumerate() {
        let (instruction, operations) = block
            .instructions
            .split_last()
            .ok_or_else(|| item_error(item, block.span, "control-flow block has no terminator"))?;
        let mut values = std::collections::HashMap::new();
        if dominance.dominates(0, block_index) {
            values.extend(
                body.blocks[0]
                    .parameters
                    .iter()
                    .map(|parameter| (parameter.name.clone(), parameter.ty.clone())),
            );
        }
        let dominating_blocks = (0..block_index)
            .filter(|prior| dominance.dominates(*prior, block_index))
            .collect::<std::collections::HashSet<_>>();
        for (name, ty, definition_block) in &prior_definitions {
            if dominating_blocks.contains(definition_block) {
                values.insert(name.clone(), ty.clone());
            }
        }
        if block_index != 0 {
            for parameter in &block.parameters {
                if values
                    .insert(parameter.name.clone(), parameter.ty.clone())
                    .is_some()
                {
                    return Err(item_error(item, parameter.span, "duplicate SSA value name"));
                }
            }
        }
        crate::text_straight_line_assembly::validate_operations(
            operations,
            &mut values,
            source,
            item,
            callees,
        )?;
        for operation in operations {
            for result in &operation.results {
                if value_names.contains(&result.as_str()) {
                    return Err(item_error(item, operation.span, "duplicate SSA value name"));
                }
                value_names.push(result.as_str());
                prior_definitions.push((result.clone(), values[result].clone(), block_index));
            }
        }
        if block_index != 0 {
            for parameter in &block.parameters {
                prior_definitions.push((parameter.name.clone(), parameter.ty.clone(), block_index));
            }
        }
        let terminator = parse_terminator(
            instruction,
            signature.result.as_ref() == &Type::Unit,
            source,
            item,
        )?;
        match terminator {
            ControlTerminator::Branch(target) => {
                validate_target_arguments(body, &values, &target, item, instruction.span)?;
            }
            ControlTerminator::Conditional {
                condition,
                when_true,
                when_false,
                merge,
            } => {
                let condition_type = values.get(&condition).ok_or_else(|| {
                    item_error(
                        item,
                        instruction.span,
                        "conditional branch operand lacks a dominating SSA definition",
                    )
                })?;
                if condition_type != &Type::Integer(1) {
                    return Err(item_error(
                        item,
                        instruction.span,
                        "conditional branch operand must have type i1",
                    ));
                }
                validate_target_arguments(body, &values, &when_true, item, instruction.span)?;
                validate_target_arguments(body, &values, &when_false, item, instruction.span)?;
                if let Some(merge) = merge {
                    check_target(&labels, &merge, item, instruction.span)?;
                }
            }
            ControlTerminator::Return(value) => match (signature.result.as_ref(), value) {
                (Type::Unit, None) => {}
                (Type::Unit, Some(_)) => {
                    return Err(item_error(
                        item,
                        instruction.span,
                        "unit return must not have an operand",
                    ));
                }
                (_, None) => {
                    return Err(item_error(
                        item,
                        instruction.span,
                        "return requires one block parameter operand",
                    ));
                }
                (result, Some(value)) => {
                    let value_type = values.get(&value).ok_or_else(|| {
                        item_error(
                            item,
                            instruction.span,
                            "return operand lacks a dominating SSA definition",
                        )
                    })?;
                    if value_type != result {
                        return Err(item_error(
                            item,
                            instruction.span,
                            "return operand type does not match the function result",
                        ));
                    }
                }
            },
            ControlTerminator::Fail(ref value) => {
                let expected = signature.failure.as_deref().ok_or_else(|| {
                    item_error(
                        item,
                        instruction.span,
                        "fail requires a function with a fails type",
                    )
                })?;
                if values.get(value) != Some(expected) {
                    return Err(item_error(
                        item,
                        instruction.span,
                        "fail operand is undefined or does not match the function error type",
                    ));
                }
            }
            ControlTerminator::Unreachable { .. } | ControlTerminator::Trap(_) => {}
            ControlTerminator::Longjmp(ref longjmp) => {
                crate::text_longjmp::validate(longjmp, &values, item, instruction.span)?;
            }
            ControlTerminator::Switch(ref switch) => {
                crate::text_switch_int::validate(switch, body, &values, item, instruction.span)?;
            }
            ControlTerminator::EnumSwitch(ref switch) => {
                crate::text_switch_enum::validate(switch, body, &values, item, instruction.span)?;
            }
            ControlTerminator::Indirect(ref branch) => {
                crate::text_indirect_branch::validate(
                    branch,
                    body,
                    &values,
                    item,
                    instruction.span,
                )?;
            }
            ControlTerminator::TailCall(ref call) => {
                let callees = callees.ok_or_else(|| {
                    item_error(
                        item,
                        instruction.span,
                        "tail.call requires module-wide signatures",
                    )
                })?;
                crate::text_direct_call::validate_tail(
                    call,
                    &values,
                    callees,
                    signature,
                    item,
                    instruction.span,
                )?;
            }
        }
    }
    crate::text_indirect_branch::verify_provenance(body, source, item)?;
    Ok(())
}

pub(crate) fn parse_terminator(
    instruction: &crate::TextInstruction,
    returns_unit: bool,
    source: &str,
    item: TextItem,
) -> Result<ControlTerminator, TextParseError> {
    let tokens = &instruction.tokens;
    match instruction.opcode.as_str() {
        "br" => {
            let mut cursor = 1;
            let target = parse_target(tokens, &mut cursor, item, instruction.span)?;
            if cursor != tokens.len() {
                return Err(item_error(item, instruction.span, "malformed br target"));
            }
            Ok(ControlTerminator::Branch(target))
        }
        "cond.br" => parse_conditional(tokens, item, instruction.span),
        "switch.int" => Ok(ControlTerminator::Switch(crate::text_switch_int::parse(
            instruction,
            item,
        )?)),
        "switch.enum" => Ok(ControlTerminator::EnumSwitch(
            crate::text_switch_enum::parse(instruction, item)?,
        )),
        "br.indirect" => Ok(ControlTerminator::Indirect(
            crate::text_indirect_branch::parse(instruction, source, item)?,
        )),
        "return" => parse_return(tokens, returns_unit, item, instruction.span),
        "fail" => {
            let mut cursor = 1;
            let value = parse_value(tokens, &mut cursor, item, instruction.span)?;
            if cursor != tokens.len() || !instruction.results.is_empty() {
                return Err(item_error(
                    item,
                    instruction.span,
                    "fail accepts exactly one SSA value and no results",
                ));
            }
            Ok(ControlTerminator::Fail(value))
        }
        "tail.call" => Ok(ControlTerminator::TailCall(
            crate::text_direct_call::parse_tail(instruction, source, item)?,
        )),
        "longjmp" => Ok(ControlTerminator::Longjmp(crate::text_longjmp::parse(
            instruction,
            source,
            item,
        )?)),
        "unreachable" => match tokens.as_slice() {
            [one] if matches!(&one.kind, TokenKind::Atom(name) if name == "unreachable") => {
                Ok(ControlTerminator::Unreachable {
                    undefined_behavior: false,
                })
            }
            [_, left, mode, right]
                if matches!(left.kind, TokenKind::Punctuation('['))
                    && matches!(right.kind, TokenKind::Punctuation(']')) =>
            {
                match &mode.kind {
                    TokenKind::Atom(mode) if mode == "trap" => Ok(ControlTerminator::Unreachable {
                        undefined_behavior: false,
                    }),
                    TokenKind::Atom(mode) if mode == "ub" => Ok(ControlTerminator::Unreachable {
                        undefined_behavior: true,
                    }),
                    _ => Err(item_error(
                        item,
                        instruction.span,
                        "unreachable expects [trap] or [ub]",
                    )),
                }
            }
            _ => Err(item_error(
                item,
                instruction.span,
                "invalid unreachable terminator",
            )),
        },
        "trap" => {
            let [_, code] = tokens.as_slice() else {
                return Err(item_error(
                    item,
                    instruction.span,
                    "trap requires a 16-bit code",
                ));
            };
            let TokenKind::Atom(code) = &code.kind else {
                return Err(item_error(
                    item,
                    instruction.span,
                    "trap code must be an integer",
                ));
            };
            let code = crate::numeric_literals::parse_unsigned_integer(code)
                .and_then(|value| u16::try_from(value).ok())
                .ok_or_else(|| item_error(item, instruction.span, "trap code must fit in u16"))?;
            Ok(ControlTerminator::Trap(code))
        }
        _ => Err(item_error(
            item,
            instruction.span,
            "unsupported terminator in text control-flow assembly",
        )),
    }
}

fn parse_conditional(
    tokens: &[Token],
    item: TextItem,
    span: Span,
) -> Result<ControlTerminator, TextParseError> {
    let mut cursor = 3;
    let condition = parse_value(tokens, &mut cursor, item, span)?;
    expect_punctuation(tokens, &mut cursor, ',', item, span)?;
    let when_true = parse_target(tokens, &mut cursor, item, span)?;
    expect_punctuation(tokens, &mut cursor, ',', item, span)?;
    let when_false = parse_target(tokens, &mut cursor, item, span)?;
    let merge = if punct(tokens, cursor, '[') {
        cursor += 1;
        expect_atom(tokens, &mut cursor, "merge", item, span)?;
        let merge = parse_label(tokens, &mut cursor, item, span)?;
        expect_punctuation(tokens, &mut cursor, ']', item, span)?;
        Some(merge)
    } else if atom(tokens, cursor) == Some("merge") {
        cursor += 1;
        Some(parse_label(tokens, &mut cursor, item, span)?)
    } else {
        None
    };
    if cursor != tokens.len() {
        return Err(item_error(item, span, "malformed cond.br terminator"));
    }
    Ok(ControlTerminator::Conditional {
        condition,
        when_true,
        when_false,
        merge,
    })
}

fn parse_return(
    tokens: &[Token],
    returns_unit: bool,
    item: TextItem,
    span: Span,
) -> Result<ControlTerminator, TextParseError> {
    match (returns_unit, tokens.len()) {
        (true, 1) => Ok(ControlTerminator::Return(None)),
        (true, _) => Err(item_error(
            item,
            span,
            "unit return must not have an operand",
        )),
        (false, _) => {
            let mut cursor = 1;
            let value = parse_value(tokens, &mut cursor, item, span)?;
            if cursor != tokens.len() {
                return Err(item_error(item, span, "malformed return operand"));
            }
            Ok(ControlTerminator::Return(Some(value)))
        }
    }
}

pub(crate) fn parse_target(
    tokens: &[Token],
    cursor: &mut usize,
    item: TextItem,
    span: Span,
) -> Result<ControlTarget, TextParseError> {
    let label = parse_label(tokens, cursor, item, span)?;
    let mut arguments = Vec::new();
    if punct(tokens, *cursor, '(') {
        *cursor += 1;
        if !punct(tokens, *cursor, ')') {
            loop {
                arguments.push(parse_value(tokens, cursor, item, span)?);
                if !punct(tokens, *cursor, ',') {
                    break;
                }
                *cursor += 1;
            }
        }
        expect_punctuation(tokens, cursor, ')', item, span)?;
    }
    Ok(ControlTarget { label, arguments })
}

pub(crate) fn parse_label(
    tokens: &[Token],
    cursor: &mut usize,
    item: TextItem,
    span: Span,
) -> Result<String, TextParseError> {
    let label = atom(tokens, *cursor)
        .filter(|label| label.starts_with("bb") && label.len() > 2)
        .ok_or_else(|| item_error(item, span, "expected basic block label"))?;
    *cursor += 1;
    Ok(label.to_owned())
}

pub(crate) fn parse_value(
    tokens: &[Token],
    cursor: &mut usize,
    item: TextItem,
    span: Span,
) -> Result<String, TextParseError> {
    if !punct(tokens, *cursor, '%') {
        return Err(item_error(item, span, "expected value operand"));
    }
    *cursor += 1;
    let name =
        atom(tokens, *cursor).ok_or_else(|| item_error(item, span, "invalid value operand"))?;
    *cursor += 1;
    Ok(name.to_owned())
}

pub(crate) fn expect_atom(
    tokens: &[Token],
    cursor: &mut usize,
    expected: &str,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    if atom(tokens, *cursor) != Some(expected) {
        return Err(item_error(item, span, format!("expected `{expected}`")));
    }
    *cursor += 1;
    Ok(())
}

pub(crate) fn expect_punctuation(
    tokens: &[Token],
    cursor: &mut usize,
    expected: char,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    if !punct(tokens, *cursor, expected) {
        return Err(item_error(item, span, format!("expected `{expected}`")));
    }
    *cursor += 1;
    Ok(())
}

pub(crate) fn check_target(
    labels: &[&str],
    target: &str,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    if labels.contains(&target) {
        Ok(())
    } else {
        Err(item_error(
            item,
            span,
            "branch target does not name a block",
        ))
    }
}

pub(crate) fn validate_target_arguments(
    body: &FunctionBody,
    values: &std::collections::HashMap<String, Type>,
    target: &ControlTarget,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let target_block = body
        .blocks
        .iter()
        .find(|block| block.label == target.label)
        .ok_or_else(|| item_error(item, span, "branch target does not name a block"))?;
    if target.arguments.len() != target_block.parameters.len() {
        return Err(item_error(
            item,
            span,
            "branch argument count does not match target block parameters",
        ));
    }
    for (argument, parameter) in target.arguments.iter().zip(&target_block.parameters) {
        let argument_type = values.get(argument).ok_or_else(|| {
            item_error(
                item,
                span,
                "branch argument lacks a dominating SSA definition",
            )
        })?;
        if argument_type != &parameter.ty {
            return Err(item_error(
                item,
                span,
                "branch argument type does not match target block parameter",
            ));
        }
    }
    Ok(())
}

pub(crate) fn atom(tokens: &[Token], index: usize) -> Option<&str> {
    match tokens.get(index).map(|token| &token.kind) {
        Some(TokenKind::Atom(value)) => Some(value),
        _ => None,
    }
}

pub(crate) fn punct(tokens: &[Token], index: usize, value: char) -> bool {
    matches!(tokens.get(index).map(|token| &token.kind), Some(TokenKind::Punctuation(found)) if *found == value)
}

pub(crate) fn item_error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
