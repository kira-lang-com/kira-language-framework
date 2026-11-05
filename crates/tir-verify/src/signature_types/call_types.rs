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
        if instruction.opcode == "call.foreign"
            && let Err(message) = call_conventions.verify_foreign_call_target(instruction)
        {
            diagnostics.push(
                Diagnostic::error(V2, message)
                    .with_primary_span(shift_span(instruction.span, source_offset)),
            );
        }
        let specialized_call = match call_conventions.specialized_direct_call(source, instruction) {
            Some(Ok(signature)) => Some(signature),
            Some(Err(message)) => {
                diagnostics.push(
                    Diagnostic::error(V2, message)
                        .with_primary_span(shift_span(instruction.span, source_offset)),
                );
                continue;
            }
            None => None,
        };
        if let Some(expected) = call_result_count(
            instruction,
            source,
            value_types,
            call_conventions,
            specialized_call.as_ref(),
        ) {
            if instruction.results.len() != expected {
                diagnostics.push(
                    Diagnostic::error(
                        V2,
                        format!(
                            "{} defines {} SSA results; its function type requires {expected}",
                            instruction.opcode,
                            instruction.results.len()
                        ),
                    )
                    .with_primary_span(shift_span(instruction.span, source_offset)),
                );
            }
        }
        let Some((arguments, parameters, variadic)) = call_shape(
            instruction,
            source,
            value_types,
            call_conventions,
            specialized_call.as_ref(),
        ) else {
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

        let signature = if let Some((function, _)) = indirect_call_type(source, instruction) {
            Some(function)
        } else {
            match call_conventions.specialized_implicit_call(source, instruction, value_types) {
                Some(Ok(function)) => Some(function),
                Some(Err(message)) => {
                    diagnostics.push(
                        Diagnostic::error(V11, message)
                            .with_primary_span(shift_span(instruction.span, source_offset)),
                    );
                    None
                }
                None => None,
            }
        };
        let Some(signature) = signature else {
            continue;
        };

        let (normal_count, normal_type) = if signature.asynchronous {
            (1, direct_callee_symbol(instruction).map(Type::Frame))
        } else {
            let result = *signature.result.clone();
            (
                usize::from(!matches!(result, Type::Unit | Type::Never)),
                Some(result),
            )
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
                signature.failure.as_deref(),
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

fn call_result_count(
    instruction: &TextInstruction,
    source: &str,
    value_types: &HashMap<String, Type>,
    call_conventions: &CallConventions,
    specialized_call: Option<&FunctionType>,
) -> Option<usize> {
    if let Some(signature) =
        call_result_signature(instruction, source, value_types, specialized_call)
    {
        return signature_result_count(&signature.result, signature.asynchronous);
    }
    if matches!(instruction.opcode.as_str(), "call.witness" | "call.vtable") {
        let result = call_conventions.special_result(instruction, source)?;
        return signature_result_count(&result.result, result.asynchronous);
    }
    if !matches!(
        instruction.opcode.as_str(),
        "call" | "call.foreign" | "call.variadic"
    ) {
        return None;
    }
    let (symbol, _, _) = instruction
        .tokens
        .iter()
        .enumerate()
        .find_map(|(index, token)| {
            is_punctuation(token, '@')
                .then(|| crate::variant_text::read_symbol(&instruction.tokens, index))
                .flatten()
        })?;
    let result = call_conventions.direct_result(&symbol)?;
    signature_result_count(&result.result, result.asynchronous)
}

fn signature_result_count(result: &Type, asynchronous: bool) -> Option<usize> {
    if asynchronous {
        Some(1)
    } else if matches!(result, Type::Unit | Type::Never) {
        Some(0)
    } else if matches!(result, Type::Generic(_)) {
        None
    } else {
        Some(1)
    }
}

fn call_result_signature(
    instruction: &TextInstruction,
    source: &str,
    value_types: &HashMap<String, Type>,
    specialized_call: Option<&FunctionType>,
) -> Option<FunctionType> {
    match instruction.opcode.as_str() {
        "call" => specialized_call.cloned(),
        "call.indirect" => indirect_call_type(source, instruction).map(|(signature, _)| signature),
        "call.closure" => {
            let text_uses = crate::ssa::text_value_uses(instruction);
            let (name, _) = text_uses.first()?;
            match value_types.get(name)? {
                Type::Closure(closure) => Some(closure.signature.clone()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn call_shape(
    instruction: &TextInstruction,
    source: &str,
    value_types: &HashMap<String, Type>,
    call_conventions: &CallConventions,
    specialized_call: Option<&FunctionType>,
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
        if matches!(instruction.opcode.as_str(), "call" | "tail.call")
            && let Some(signature) = specialized_call
        {
            return Some((
                argument_names_at(&instruction.tokens, first_call_paren(&instruction.tokens)?),
                parameter_types(signature),
                false,
            ));
        }
        if matches!(instruction.opcode.as_str(), "try.call" | "invoke")
            && let Some(signature) =
                call_conventions.specialized_implicit_call(source, instruction, value_types)
        {
            let signature = signature.ok()?;
            return Some((
                argument_names_at(&instruction.tokens, first_call_paren(&instruction.tokens)?),
                parameter_types(&signature),
                false,
            ));
        }
        let (symbol, _, _) = instruction
            .tokens
            .iter()
            .enumerate()
            .find_map(|(index, token)| {
                is_punctuation(token, '@')
                    .then(|| crate::variant_text::read_symbol(&instruction.tokens, index))
                    .flatten()
            })?;
        let parameters = call_conventions.direct_parameter_types(&symbol)?;
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
