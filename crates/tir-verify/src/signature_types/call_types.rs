use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind};
use tir_format::{FunctionBody, TextInstruction};
use tir_types::{FunctionType, Type};

use super::CallConventions;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const V11: Code = Code::new(Dialect::Tir, Area::Verifier, 11);

pub(super) fn verify_function(
    source: &str,
    body: &FunctionBody,
    value_types: &HashMap<String, Type>,
    call_conventions: &CallConventions,
    source_offset: usize,
) -> Vec<Diagnostic> {
    let mut diagnostics =
        verify_implicit_call_edges(source, body, value_types, call_conventions, source_offset);
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if matches!(instruction.opcode.as_str(), "call" | "tail.call")
            && let Some((symbol, _, _)) =
                instruction
                    .tokens
                    .iter()
                    .enumerate()
                    .find_map(|(index, token)| {
                        is_punctuation(token, '@')
                            .then(|| crate::variant_text::read_symbol(&instruction.tokens, index))
                            .flatten()
                    })
            && let Some(expected) = call_conventions.direct_generic_parameter_count(&symbol)
        {
            match call_conventions.explicit_type_argument_count(instruction) {
                Some(actual) if actual != expected => diagnostics.push(
                    Diagnostic::error(
                        V11,
                        format!(
                            "{} has {actual} explicit generic type arguments; expected {expected}",
                            instruction.opcode
                        ),
                    )
                    .with_primary_span(shift_span(instruction.span, source_offset)),
                ),
                None if expected > 0 => diagnostics.push(
                    Diagnostic::error(
                        V11,
                        format!(
                            "{} requires {expected} explicit generic type arguments",
                            instruction.opcode
                        ),
                    )
                    .with_primary_span(shift_span(instruction.span, source_offset)),
                ),
                _ => {}
            }
        }
        if let Some((expected, _)) =
            call_conventions.call_result_shape(instruction, source, value_types)
            && instruction.results.len() != expected
        {
            diagnostics.push(
                Diagnostic::error(
                    V2,
                    format!(
                        "{} defines {} SSA results; expected {expected}",
                        instruction.opcode,
                        instruction.results.len()
                    ),
                )
                .with_primary_span(shift_span(instruction.span, source_offset)),
            );
        }
        let Some((arguments, parameters, variadic)) =
            call_shape(instruction, source, value_types, call_conventions)
        else {
            continue;
        };
        let count_mismatch = if variadic {
            arguments.len() < parameters.len()
        } else {
            arguments.len() != parameters.len()
        };
        if count_mismatch {
            let relation = if variadic { "at least" } else { "exactly" };
            diagnostics.push(
                Diagnostic::error(
                    V2,
                    format!(
                        "{} has {} fixed SSA arguments; expected {relation} {}",
                        instruction.opcode,
                        arguments.len(),
                        parameters.len()
                    ),
                )
                .with_primary_span(shift_span(instruction.span, source_offset)),
            );
            continue;
        }
        for (argument, expected) in arguments.iter().zip(&parameters) {
            let Some(actual) = value_types.get(argument) else {
                continue;
            };
            if actual == expected {
                continue;
            }
            let (Ok(actual), Ok(expected)) = (
                actual.canonical_text_in(call_conventions.module_path(), &HashMap::new()),
                expected.canonical_text_in(call_conventions.module_path(), &HashMap::new()),
            ) else {
                continue;
            };
            if actual != expected {
                diagnostics.push(
                    Diagnostic::error(
                        V2,
                        format!(
                            "{} argument %{argument} has type {actual}; expected {expected}",
                            instruction.opcode
                        ),
                    )
                    .with_primary_span(shift_span(instruction.span, source_offset)),
                );
            }
        }
    }
    diagnostics
}

fn verify_implicit_call_edges(
    source: &str,
    body: &FunctionBody,
    value_types: &HashMap<String, Type>,
    call_conventions: &CallConventions,
    source_offset: usize,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if !matches!(instruction.opcode.as_str(), "try.call" | "invoke") {
            continue;
        }

        let targets = crate::text_cfg::instruction_targets(body, instruction);
        if instruction.opcode == "invoke"
            && let Some(unwind_target) = targets.get(1).and_then(|target| body.blocks.get(*target))
            && unwind_target
                .instructions
                .first()
                .is_none_or(|first| first.opcode != "landingpad")
        {
            diagnostics.push(
                Diagnostic::error(
                    V2,
                    format!(
                        "invoke unwind target {} does not begin with landingpad",
                        unwind_target.label
                    ),
                )
                .with_primary_span(shift_span(instruction.span, source_offset)),
            );
        }

        let shape = if let Some((function, _)) = indirect_call_type(source, instruction) {
            indirect_call_edge_shape(&function)
        } else if let Some(symbol) = direct_callee_symbol(instruction) {
            call_conventions.direct_call_edge_shape(instruction, source, &symbol, value_types)
        } else {
            None
        };
        let Some((normal_count, normal_type, failure_type)) = shape else {
            if let Some(symbol) = direct_callee_symbol(instruction)
                && call_conventions
                    .direct_generic_parameter_count(&symbol)
                    .is_some_and(|count| count > 0)
            {
                diagnostics.push(
                    Diagnostic::error(
                        V11,
                        format!(
                            "cannot resolve generic type arguments for {} from its call",
                            instruction.opcode
                        ),
                    )
                    .with_primary_span(shift_span(instruction.span, source_offset)),
                );
            }
            continue;
        };

        if let Some(normal_target) = targets.first().and_then(|target| body.blocks.get(*target)) {
            verify_implicit_edge_shape(
                instruction,
                &normal_target.label,
                "normal",
                &normal_target.parameters,
                normal_count,
                normal_type.as_ref(),
                call_conventions.module_path(),
                source_offset,
                instruction.opcode != "invoke",
                &mut diagnostics,
            );
        }

        if instruction.opcode == "try.call"
            && let (Some(error_target), Some(failure_type)) = (
                targets.get(1).and_then(|target| body.blocks.get(*target)),
                failure_type.as_ref(),
            )
        {
            let error_count = usize::from(!matches!(failure_type, Type::Unit | Type::Never));
            verify_implicit_edge_shape(
                instruction,
                &error_target.label,
                "error",
                &error_target.parameters,
                error_count,
                Some(failure_type),
                call_conventions.module_path(),
                source_offset,
                true,
                &mut diagnostics,
            );
        }
    }
    diagnostics
}

fn indirect_call_edge_shape(
    function: &FunctionType,
) -> Option<(usize, Option<Type>, Option<Type>)> {
    let (result_count, result_type) = if function.asynchronous {
        (1, None)
    } else {
        let result = *function.result.clone();
        let count = usize::from(!matches!(result, Type::Unit | Type::Never));
        (count, Some(result))
    };
    Some((
        result_count,
        result_type,
        function.failure.as_deref().cloned(),
    ))
}

fn direct_callee_symbol(instruction: &TextInstruction) -> Option<String> {
    instruction
        .tokens
        .iter()
        .enumerate()
        .find_map(|(index, token)| {
            is_punctuation(token, '@')
                .then(|| crate::variant_text::read_symbol(&instruction.tokens, index))
                .flatten()
                .map(|(symbol, _, _)| symbol)
        })
}

fn verify_implicit_edge_shape(
    instruction: &TextInstruction,
    target: &str,
    edge_name: &str,
    parameters: &[tir_format::TextParameter],
    expected_count: usize,
    expected_type: Option<&Type>,
    module_path: &str,
    source_offset: usize,
    check_count: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if check_count && parameters.len() != expected_count {
        diagnostics.push(
            Diagnostic::error(
                V2,
                format!(
                    "{} {edge_name} edge to {target} has {} block parameters; expected {expected_count}",
                    instruction.opcode,
                    parameters.len()
                ),
            )
            .with_primary_span(shift_span(instruction.span, source_offset)),
        );
        return;
    }
    if parameters.len() != expected_count || expected_count != 1 {
        return;
    }
    let (Some(expected_type), Some(actual)) = (expected_type, parameters.first()) else {
        return;
    };
    if &actual.ty == expected_type {
        return;
    }
    let (Ok(actual_type), Ok(expected_type)) = (
        actual.ty.canonical_text_in(module_path, &HashMap::new()),
        expected_type.canonical_text_in(module_path, &HashMap::new()),
    ) else {
        return;
    };
    if actual_type != expected_type {
        diagnostics.push(
            Diagnostic::error(
                V2,
                format!(
                    "{} {edge_name} edge to {target} passes {actual_type}; expected {expected_type}",
                    instruction.opcode
                ),
            )
            .with_primary_span(shift_span(instruction.span, source_offset)),
        );
    }
}

fn call_shape(
    instruction: &TextInstruction,
    source: &str,
    value_types: &HashMap<String, Type>,
    call_conventions: &CallConventions,
) -> Option<(Vec<String>, Vec<Type>, bool)> {
    if (instruction.opcode == "call.indirect"
        || instruction.opcode == "try.call" && indirect_call_type(source, instruction).is_some())
        && let Some((function, open)) = indirect_call_type(source, instruction)
    {
        return Some((
            argument_names_at(&instruction.tokens, open),
            parameter_types(&function),
            false,
        ));
    }
    if instruction.opcode == "call.closure" {
        let uses = crate::ssa::text_value_uses(instruction);
        let (name, _) = uses.first()?;
        let Type::Closure(closure) = value_types.get(name)? else {
            return None;
        };
        let open = first_call_paren(&instruction.tokens)?;
        return Some((
            argument_names_at(&instruction.tokens, open),
            parameter_types(&closure.signature),
            false,
        ));
    }
    if matches!(instruction.opcode.as_str(), "call.witness" | "call.vtable") {
        let (arguments, parameters) =
            call_conventions.special_parameter_types(instruction, source)?;
        return Some((arguments, parameters, false));
    }
    if matches!(
        instruction.opcode.as_str(),
        "call" | "call.foreign" | "call.variadic" | "try.call" | "invoke" | "tail.call"
    ) {
        let (symbol, _, _) = instruction
            .tokens
            .iter()
            .enumerate()
            .find_map(|(index, token)| {
                is_punctuation(token, '@')
                    .then(|| crate::variant_text::read_symbol(&instruction.tokens, index))
                    .flatten()
            })?;
        let parameters = call_conventions.direct_call_parameter_types(
            instruction,
            source,
            &symbol,
            value_types,
        )?;
        let open = first_call_paren(&instruction.tokens)?;
        return Some((
            argument_names_at(&instruction.tokens, open),
            parameters,
            instruction.opcode == "call.variadic",
        ));
    }
    None
}

fn parameter_types(function: &FunctionType) -> Vec<Type> {
    function
        .parameters
        .iter()
        .map(|parameter| parameter.ty.clone())
        .collect()
}

fn first_call_paren(tokens: &[Token]) -> Option<usize> {
    let start = crate::ssa::text_instruction_operands_start(tokens);
    let mut angle_depth = 0_i32;
    (start..tokens.len()).find(|index| {
        if is_punctuation(&tokens[*index], '(') && angle_depth == 0 {
            return true;
        }
        match tokens[*index].kind {
            TokenKind::Punctuation('<') => angle_depth += 1,
            TokenKind::Punctuation('>') => angle_depth -= 1,
            _ => {}
        }
        false
    })
}

fn argument_names_at(tokens: &[Token], open: usize) -> Vec<String> {
    let mut depth = 0_i32;
    let Some(close) = (open..tokens.len()).find(|index| {
        if is_punctuation(&tokens[*index], '(') {
            depth += 1;
        } else if is_punctuation(&tokens[*index], ')') {
            depth -= 1;
            if depth == 0 {
                return true;
            }
        }
        false
    }) else {
        return Vec::new();
    };
    tokens[open + 1..close]
        .windows(2)
        .filter_map(|pair| match (&pair[0].kind, &pair[1].kind) {
            (TokenKind::Punctuation('%'), TokenKind::Atom(name)) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

fn indirect_call_type(
    source: &str,
    instruction: &TextInstruction,
) -> Option<(FunctionType, usize)> {
    super::indirect_call_type(source, instruction)
}

fn shift_span(span: Span, offset: usize) -> Span {
    Span::new(
        span.start.saturating_add(offset),
        span.end.saturating_add(offset),
    )
}
