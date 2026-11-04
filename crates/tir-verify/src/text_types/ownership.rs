use std::collections::HashMap;
use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{FunctionBody, TextInstruction};
use tir_types::Type;

use crate::gpu_profile::policy::TypePolicies;

const V5: Code = Code::new(Dialect::Tir, Area::Verifier, 5);

mod v4;

pub(super) fn verify(
    body: &FunctionBody,
    initial_borrowed: &HashSet<String>,
    value_types: &HashMap<String, Type>,
    policies: &TypePolicies,
    source: &str,
    call_conventions: &crate::signature_types::CallConventions,
) -> Vec<Diagnostic> {
    let mut diagnostics = v4::verify(
        body,
        initial_borrowed,
        value_types,
        policies,
        source,
        call_conventions,
    );
    let borrowed = close_borrowed_flow(body, initial_borrowed.clone(), value_types, policies);
    for block in &body.blocks {
        for instruction in &block.instructions {
            if !consumes_borrowed_value(instruction) {
                continue;
            }
            for (value, _) in crate::ssa::text_value_uses(instruction) {
                if borrowed.contains(&value)
                    && value_types
                        .get(&value)
                        .is_some_and(|ty| !policies.is_trivial(ty))
                {
                    diagnostics.push(
                        Diagnostic::error(
                            V5,
                            format!(
                                "borrowed value %{value} is used by consuming instruction {}",
                                instruction.opcode
                            ),
                        )
                        .with_primary_span(instruction.span),
                    );
                }
            }
        }
    }

    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        let borrowed_arguments = match instruction.opcode.as_str() {
            "call" | "try.call" | "invoke" | "tail.call" => direct_call_borrowed_arguments(
                instruction,
                &borrowed,
                value_types,
                policies,
                call_conventions,
            ),
            "call.indirect" => indirect_call_borrowed_arguments(
                source,
                instruction,
                &borrowed,
                value_types,
                policies,
            ),
            "call.closure" => {
                closure_call_borrowed_arguments(instruction, &borrowed, value_types, policies)
            }
            "call.witness" | "call.vtable" => call_conventions
                .special_call(instruction, source)
                .map(|(arguments, conventions)| {
                    borrowed_call_arguments(
                        arguments,
                        &conventions,
                        &borrowed,
                        value_types,
                        policies,
                    )
                })
                .unwrap_or_default(),
            "call.foreign" => crate::ssa::text_value_uses(instruction)
                .into_iter()
                .filter(|(name, _)| {
                    borrowed.contains(name)
                        && value_types
                            .get(name)
                            .is_some_and(|ty| !policies.is_trivial(ty))
                })
                .map(|(name, _)| name)
                .collect(),
            "call.variadic" => {
                let mut values = direct_call_borrowed_arguments(
                    instruction,
                    &borrowed,
                    value_types,
                    policies,
                    call_conventions,
                );
                values.extend(variadic_call_borrowed_arguments(
                    instruction,
                    &borrowed,
                    value_types,
                    policies,
                ));
                values
            }
            _ => Vec::new(),
        };
        for value in borrowed_arguments {
            diagnostics.push(
                Diagnostic::error(
                    V5,
                    format!("borrowed value %{value} is passed to a consuming call"),
                )
                .with_primary_span(instruction.span),
            );
        }
    }

    for (block_index, edges) in crate::text_cfg::text_edges(body).iter().enumerate() {
        for edge in edges {
            for (argument, parameter) in edge
                .arguments
                .iter()
                .zip(&body.blocks[edge.target].parameters)
            {
                if borrowed.contains(argument)
                    && !matches!(
                        parameter.convention,
                        Some(tir_types::Convention::Borrow | tir_types::Convention::BorrowMut)
                    )
                    && !policies.is_trivial(&parameter.ty)
                {
                    diagnostics.push(
                        Diagnostic::error(
                            V5,
                            format!(
                                "borrowed value %{argument} is transferred to an owned parameter of block {}",
                                body.blocks[edge.target].label
                            ),
                        )
                        .with_primary_span(body.blocks[block_index].span),
                    );
                }
            }
        }
    }
    diagnostics
}

fn close_borrowed_flow(
    body: &FunctionBody,
    mut borrowed: HashSet<String>,
    value_types: &HashMap<String, Type>,
    policies: &TypePolicies,
) -> HashSet<String> {
    loop {
        let mut changed = false;
        for block in &body.blocks {
            for instruction in &block.instructions {
                let result_types = instruction
                    .results
                    .iter()
                    .filter_map(|name| value_types.get(name).cloned())
                    .collect::<Vec<_>>();
                for name in super::aggregate::borrowed_results(
                    instruction,
                    &result_types,
                    &borrowed,
                    policies,
                ) {
                    changed |= borrowed.insert(name);
                }
            }
        }
        for edges in crate::text_cfg::text_edges(body) {
            for edge in edges {
                for (argument, parameter) in edge
                    .arguments
                    .iter()
                    .zip(&body.blocks[edge.target].parameters)
                {
                    if borrowed.contains(argument)
                        && matches!(
                            parameter.convention,
                            Some(tir_types::Convention::Borrow | tir_types::Convention::BorrowMut)
                        )
                        && !policies.is_trivial(&parameter.ty)
                    {
                        changed |= borrowed.insert(parameter.name.clone());
                    }
                }
            }
        }
        if !changed {
            return borrowed;
        }
    }
}

fn consumes_borrowed_value(instruction: &TextInstruction) -> bool {
    matches!(
        instruction.opcode.as_str(),
        "move" | "destroy" | "into.raw" | "store" | "return" | "fail" | "closure.new"
    )
}

fn direct_call_borrowed_arguments(
    instruction: &TextInstruction,
    borrowed: &HashSet<String>,
    value_types: &HashMap<String, Type>,
    policies: &TypePolicies,
    call_conventions: &crate::signature_types::CallConventions,
) -> Vec<String> {
    let Some((symbol, end, _)) =
        instruction
            .tokens
            .iter()
            .enumerate()
            .find_map(|(index, token)| {
                matches!(token.kind, tir_format::text::TokenKind::Punctuation('@'))
                    .then(|| crate::variant_text::read_symbol(&instruction.tokens, index))
                    .flatten()
            })
    else {
        return Vec::new();
    };
    let Some(conventions) = call_conventions.get(&symbol) else {
        return Vec::new();
    };
    borrowed_call_arguments(
        call_argument_names(instruction, end),
        conventions,
        borrowed,
        value_types,
        policies,
    )
}

fn variadic_call_borrowed_arguments(
    instruction: &TextInstruction,
    borrowed: &HashSet<String>,
    value_types: &HashMap<String, Type>,
    policies: &TypePolicies,
) -> Vec<String> {
    variadic_argument_names(instruction)
        .into_iter()
        .filter(|name| {
            borrowed.contains(name)
                && value_types
                    .get(name)
                    .is_some_and(|ty| !policies.is_trivial(ty))
        })
        .collect()
}

fn variadic_argument_names(instruction: &TextInstruction) -> Vec<String> {
    let tokens = &instruction.tokens;
    let start = crate::ssa::text_instruction_operands_start(tokens);
    let mut angle_depth = 0_i32;
    let open = (start..tokens.len()).find(|index| {
        if matches!(
            tokens[*index].kind,
            tir_format::text::TokenKind::Punctuation('(')
        ) && angle_depth == 0
        {
            return true;
        }
        match tokens[*index].kind {
            tir_format::text::TokenKind::Punctuation('<') => angle_depth += 1,
            tir_format::text::TokenKind::Punctuation('>') => angle_depth -= 1,
            _ => {}
        }
        false
    });
    let Some(open) = open else {
        return Vec::new();
    };
    let mut depth = 0_i32;
    let Some(close) = (open..tokens.len()).find(|index| {
        match tokens[*index].kind {
            tir_format::text::TokenKind::Punctuation('(') => depth += 1,
            tir_format::text::TokenKind::Punctuation(')') => {
                depth -= 1;
                if depth == 0 {
                    return true;
                }
            }
            _ => {}
        }
        false
    }) else {
        return Vec::new();
    };
    let Some(separator) = (open + 1..close).find(|index| {
        matches!(
            tokens[*index].kind,
            tir_format::text::TokenKind::Punctuation(';')
        )
    }) else {
        return Vec::new();
    };
    tokens[separator + 1..close]
        .windows(2)
        .filter_map(|pair| match (&pair[0].kind, &pair[1].kind) {
            (
                tir_format::text::TokenKind::Punctuation('%'),
                tir_format::text::TokenKind::Atom(name),
            ) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn indirect_call_borrowed_arguments(
    source: &str,
    instruction: &TextInstruction,
    borrowed: &HashSet<String>,
    value_types: &HashMap<String, Type>,
    policies: &TypePolicies,
) -> Vec<String> {
    let Some((function, argument_open)) =
        crate::signature_types::indirect_call_type(source, instruction)
    else {
        return Vec::new();
    };
    let conventions = function
        .parameters
        .iter()
        .map(|parameter| parameter.convention)
        .collect::<Vec<_>>();
    borrowed_call_arguments(
        call_argument_names_at(instruction, argument_open),
        &conventions,
        borrowed,
        value_types,
        policies,
    )
}

fn closure_call_borrowed_arguments(
    instruction: &TextInstruction,
    borrowed: &HashSet<String>,
    value_types: &HashMap<String, Type>,
    policies: &TypePolicies,
) -> Vec<String> {
    let uses = crate::ssa::text_value_uses(instruction);
    let Some((closure_name, _)) = uses.first() else {
        return Vec::new();
    };
    let Some(Type::Closure(closure)) = value_types.get(closure_name) else {
        return Vec::new();
    };
    let conventions = closure
        .signature
        .parameters
        .iter()
        .map(|parameter| parameter.convention)
        .collect::<Vec<_>>();
    borrowed_call_arguments(
        call_argument_names(
            instruction,
            crate::ssa::text_instruction_operands_start(&instruction.tokens),
        ),
        &conventions,
        borrowed,
        value_types,
        policies,
    )
}

fn borrowed_call_arguments(
    arguments: Vec<String>,
    conventions: &[Option<tir_types::Convention>],
    borrowed: &HashSet<String>,
    value_types: &HashMap<String, Type>,
    policies: &TypePolicies,
) -> Vec<String> {
    arguments
        .into_iter()
        .zip(conventions)
        .filter(|(name, convention)| {
            borrowed.contains(name)
                && value_types
                    .get(name)
                    .is_some_and(|ty| !policies.is_trivial(ty))
                && !matches!(
                    **convention,
                    Some(tir_types::Convention::Borrow | tir_types::Convention::BorrowMut)
                )
        })
        .map(|(name, _)| name)
        .collect()
}

fn call_argument_names(instruction: &TextInstruction, start: usize) -> Vec<String> {
    let tokens = &instruction.tokens;
    let mut angle_depth = 0_i32;
    let open = (start..tokens.len()).find(|index| {
        if matches!(
            tokens[*index].kind,
            tir_format::text::TokenKind::Punctuation('(')
        ) && angle_depth == 0
        {
            return true;
        }
        match tokens[*index].kind {
            tir_format::text::TokenKind::Punctuation('<') => angle_depth += 1,
            tir_format::text::TokenKind::Punctuation('>') => angle_depth -= 1,
            _ => {}
        }
        false
    });
    let Some(open) = open else {
        return Vec::new();
    };
    call_argument_names_at(instruction, open)
}

fn call_argument_names_at(instruction: &TextInstruction, open: usize) -> Vec<String> {
    let tokens = &instruction.tokens;
    let mut depth = 0_i32;
    let close = (open..tokens.len()).find(|index| {
        match tokens[*index].kind {
            tir_format::text::TokenKind::Punctuation('(') => depth += 1,
            tir_format::text::TokenKind::Punctuation(')') => {
                depth -= 1;
                if depth == 0 {
                    return true;
                }
            }
            _ => {}
        }
        false
    });
    let Some(close) = close else {
        return Vec::new();
    };
    tokens[open + 1..close]
        .windows(2)
        .filter_map(|pair| match (&pair[0].kind, &pair[1].kind) {
            (
                tir_format::text::TokenKind::Punctuation('%'),
                tir_format::text::TokenKind::Atom(name),
            ) => Some(name.clone()),
            _ => None,
        })
        .collect()
}
