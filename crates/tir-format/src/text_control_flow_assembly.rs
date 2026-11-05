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
}

#[derive(Clone, Debug)]
pub(crate) struct ControlTarget {
    pub(crate) label: String,
    pub(crate) arguments: Vec<String>,
}

pub(crate) fn uses_supported_control_flow(body: &FunctionBody) -> bool {
    body.blocks.len() > 1
        || body.blocks.iter().any(|block| {
            block
                .instructions
                .last()
                .is_some_and(|instruction| matches!(instruction.opcode.as_str(), "br" | "cond.br"))
        })
}

pub(crate) fn validate_control_flow_body(
    body: &FunctionBody,
    signature: &FunctionType,
    source: &str,
    item: TextItem,
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
        let mut values = body.blocks[0]
            .parameters
            .iter()
            .map(|parameter| (parameter.name.clone(), parameter.ty.clone()))
            .collect::<std::collections::HashMap<_, _>>();
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
        let terminator =
            parse_terminator(instruction, signature.result.as_ref() == &Type::Unit, item)?;
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
        }
    }
    Ok(())
}

pub(crate) fn parse_terminator(
    instruction: &crate::TextInstruction,
    returns_unit: bool,
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
        "return" => parse_return(tokens, returns_unit, item, instruction.span),
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

fn parse_target(
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

fn parse_label(
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

fn parse_value(
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

fn expect_atom(
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

fn expect_punctuation(
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

fn check_target(
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

fn validate_target_arguments(
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

fn atom(tokens: &[Token], index: usize) -> Option<&str> {
    match tokens.get(index).map(|token| &token.kind) {
        Some(TokenKind::Atom(value)) => Some(value),
        _ => None,
    }
}

fn punct(tokens: &[Token], index: usize, value: char) -> bool {
    matches!(tokens.get(index).map(|token| &token.kind), Some(TokenKind::Punctuation(found)) if *found == value)
}

fn item_error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
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

    fn module(body: &str) -> String {
        format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.route(%a: i32, %cond: i1) -> i32 engine(native) {{\n{body}\n}}\n"
        )
    }

    #[test]
    fn multi_block_parameters_round_trip() {
        let source = module(
            "bb0(%a: i32, %cond: i1):\n  cond.br %cond, bb1(%a), bb2(%a)\nbb1(%left: i32):\n  br bb2(%left)\nbb2(%right: i32):\n  return %right",
        );
        let parsed = parse_text_module(&source).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("block arguments assemble");
        let module = BinaryModule::decode(&bytes).expect("binary decodes");
        let text = disassemble_text_module(&module, u64::MAX).expect("binary disassembles");
        let rebuilt = assemble_text_module(&parse_text_module(&text).expect("text reparses"))
            .expect("canonical text reassembles");
        assert_eq!(bytes, rebuilt);
    }

    #[test]
    fn arithmetic_inside_multiple_blocks_round_trips() {
        let source = module(
            "bb0(%a: i32, %cond: i1):\n  %twice = add %a, %a\n  cond.br %cond, bb1(%twice), bb2(%a)\nbb1(%left: i32):\n  %four = const.int i32, 4\n  %sum = add %left, %four\n  br bb2(%sum)\nbb2(%right: i32):\n  %zero = const.int i32, 0\n  %result = add %right, %zero\n  return %result",
        );
        let parsed = parse_text_module(&source).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("multi-block operations assemble");
        let module = BinaryModule::decode(&bytes).expect("binary decodes");
        let text = disassemble_text_module(&module, u64::MAX).expect("binary disassembles");
        let rebuilt = assemble_text_module(&parse_text_module(&text).expect("text reparses"))
            .expect("canonical text reassembles");
        assert_eq!(bytes, rebuilt);
    }

    #[test]
    fn loop_back_edge_preserves_ssa_numbering() {
        let source = module(
            "bb0(%a: i32, %cond: i1):\n  br bb1(%a)\nbb1(%i: i32):\n  %one = const.int i32, 1\n  %next = add %i, %one\n  cond.br %cond, bb1(%next), bb2(%next)\nbb2(%result: i32):\n  return %result",
        );
        let parsed = parse_text_module(&source).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("loop assembles");
        let decoded = BinaryModule::decode(&bytes).expect("binary decodes");
        let canonical_text =
            disassemble_text_module(&decoded, u64::MAX).expect("loop disassembles");
        let rebuilt =
            assemble_text_module(&parse_text_module(&canonical_text).expect("text reparses"))
                .expect("canonical text reassembles");
        assert_eq!(bytes, rebuilt);
    }

    #[test]
    fn dominating_cross_block_value_round_trips() {
        let source = module(
            "bb0(%a: i32, %cond: i1):\n  %shared = add %a, %a\n  cond.br %cond, bb1(%shared), bb2(%a)\nbb1(%left: i32):\n  %one = const.int i32, 1\n  %sum = add %left, %shared\n  br bb2(%sum)\nbb2(%right: i32):\n  %answer = add %right, %shared\n  return %answer",
        );
        let parsed = parse_text_module(&source).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("dominating definitions assemble");
        let binary = BinaryModule::decode(&bytes).expect("binary decodes");
        let canonical = disassemble_text_module(&binary, u64::MAX).expect("binary disassembles");
        let reconstructed =
            assemble_text_module(&parse_text_module(&canonical).expect("text reparses"))
                .expect("canonical text reassembles");
        assert_eq!(bytes, reconstructed);
    }

    #[test]
    fn non_dominating_cross_block_value_is_rejected() {
        let source = module(
            "bb0(%a: i32, %cond: i1):\n  cond.br %cond, bb1(%a), bb2(%a)\nbb1(%left: i32):\n  %local = add %left, %left\n  br bb2(%local)\nbb2(%right: i32):\n  %invalid = add %right, %local\n  return %invalid",
        );
        let parsed = parse_text_module(&source).expect("text parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "a value from a non-dominating sibling block must not assemble"
        );
    }

    #[test]
    fn explicit_trivial_ownership_in_multiple_blocks_round_trips() {
        let source = module(
            "bb0(%a: i32, %cond: i1):\n  %duplicate = copy %a\n  cond.br %cond, bb1(%duplicate), bb2(%a)\nbb1(%left: i32):\n  %taken = move %left\n  br bb2(%taken)\nbb2(%right: i32):\n  destroy %a\n  return %right",
        );
        let parsed = parse_text_module(&source).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("explicit ownership assembles");
        let decoded = BinaryModule::decode(&bytes).expect("binary decodes");
        let text = disassemble_text_module(&decoded, u64::MAX).expect("binary disassembles");
        let rebuilt = assemble_text_module(&parse_text_module(&text).expect("text reparses"))
            .expect("canonical text reassembles");
        assert_eq!(bytes, rebuilt);
    }

    #[test]
    fn multi_block_argument_types_and_arity_are_checked() {
        let invalid = [
            "bb0(%a: i32, %cond: i1):\n  br bb1(%cond)\nbb1(%other: i32):\n  return %other",
            "bb0(%a: i32, %cond: i1):\n  br bb1()\nbb1(%other: i32):\n  return %other",
            "bb0(%a: i32, %cond: i1):\n  br bb1(%a)\nbb1(%other: i32):\n  return %undefined",
        ];
        for body in invalid {
            let parsed = parse_text_module(&module(body)).expect("text parses");
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid branch assembled: {body}"
            );
        }
    }
}
