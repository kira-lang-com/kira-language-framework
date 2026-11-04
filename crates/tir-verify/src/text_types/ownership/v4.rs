use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::{FunctionBody, TextInstruction};
use tir_types::{Convention, Type};

use crate::gpu_profile::policy::TypePolicies;
use crate::signature_types::CallConventions;
use crate::text_cfg::TextEdge;

const V4: Code = Code::new(Dialect::Tir, Area::Verifier, 4);

#[derive(Clone, Copy)]
struct DefinitionSite {
    block: usize,
    instruction: usize,
    parameter: bool,
}

pub(super) fn verify(
    body: &FunctionBody,
    borrowed: &HashSet<String>,
    value_types: &HashMap<String, Type>,
    policies: &TypePolicies,
    source: &str,
    call_conventions: &CallConventions,
) -> Vec<Diagnostic> {
    let mut definitions = BTreeMap::new();
    for (block_index, block) in body.blocks.iter().enumerate() {
        for parameter in &block.parameters {
            definitions
                .entry(parameter.name.clone())
                .or_insert(DefinitionSite {
                    block: block_index,
                    instruction: 0,
                    parameter: true,
                });
        }
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            for result in &instruction.results {
                definitions.entry(result.clone()).or_insert(DefinitionSite {
                    block: block_index,
                    instruction: instruction_index + 1,
                    parameter: false,
                });
            }
        }
    }

    let edges = crate::text_cfg::text_edges(body);
    let mut diagnostics = Vec::new();
    for (value, definition) in definitions {
        if borrowed.contains(&value)
            || !value_types
                .get(&value)
                .is_some_and(|ty| !policies.is_trivial(ty))
        {
            continue;
        }
        verify_value(
            &value,
            definition,
            body,
            &edges,
            value_types,
            policies,
            source,
            call_conventions,
            &mut diagnostics,
        );
    }
    diagnostics
}

#[allow(clippy::too_many_arguments)]
fn verify_value(
    value: &str,
    definition: DefinitionSite,
    body: &FunctionBody,
    edges: &[Vec<TextEdge>],
    value_types: &HashMap<String, Type>,
    policies: &TypePolicies,
    source: &str,
    call_conventions: &CallConventions,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut pending = VecDeque::from([(definition.block, definition.instruction, false)]);
    let mut visited = HashSet::new();
    while let Some((block_index, start, initially_consumed)) = pending.pop_front() {
        if !visited.insert((block_index, start, initially_consumed)) {
            continue;
        }
        let Some(block) = body.blocks.get(block_index) else {
            continue;
        };
        let mut consumed = initially_consumed;
        for (instruction_index, instruction) in block.instructions.iter().enumerate().skip(start) {
            if !definition.parameter
                && start == 0
                && block_index == definition.block
                && instruction_index + 1 == definition.instruction
            {
                if !consumed {
                    report(
                        diagnostics,
                        value,
                        "not consumed before its next loop definition",
                        instruction.span,
                    );
                    return;
                }
                if instruction_uses(instruction, value) {
                    report(
                        diagnostics,
                        value,
                        "used after it was consumed",
                        instruction.span,
                    );
                    return;
                }
                consumed = false;
            } else if consumed && instruction_uses(instruction, value) {
                report(
                    diagnostics,
                    value,
                    "used after it was consumed",
                    instruction.span,
                );
                return;
            }
            if instruction.opcode == "guard" {
                let moved = consumed_values(instruction, value_types, source, call_conventions);
                let Some(deopt_state) = consume(value, consumed, &moved) else {
                    report(
                        diagnostics,
                        value,
                        "consumed more than once on the guard deoptimization path",
                        instruction.span,
                    );
                    return;
                };
                if !deopt_state {
                    report(
                        diagnostics,
                        value,
                        "not consumed on the guard deoptimization path",
                        instruction.span,
                    );
                    return;
                }
                continue;
            }

            let moved = consumed_values(instruction, value_types, source, call_conventions);
            let Some(next_consumed) = consume(value, consumed, &moved) else {
                report(
                    diagnostics,
                    value,
                    "consumed more than once",
                    instruction.span,
                );
                return;
            };
            consumed = next_consumed;

            if !crate::is_terminator(&instruction.opcode) {
                continue;
            }

            match instruction.opcode.as_str() {
                "trap" => return,
                "unreachable" if !has_atom(instruction, "ub") => return,
                "return" | "fail" | "resume" | "throw" | "tail.call" | "deopt" | "coro.finish" => {
                    if !consumed {
                        report(
                            diagnostics,
                            value,
                            &format!("not consumed on a path to `{}`", instruction.opcode),
                            instruction.span,
                        );
                        return;
                    }
                    return;
                }
                "unreachable" => {
                    if !consumed {
                        report(
                            diagnostics,
                            value,
                            "not consumed on a path to `unreachable [ub]`",
                            instruction.span,
                        );
                        return;
                    }
                    return;
                }
                _ => {
                    for edge in edges.get(block_index).into_iter().flatten() {
                        let Some(next_consumed) =
                            edge_consumed(value, consumed, edge, body, policies)
                        else {
                            report(
                                diagnostics,
                                value,
                                "consumed more than once while transferring to a block parameter",
                                instruction.span,
                            );
                            return;
                        };
                        if definition.parameter && edge.target == definition.block {
                            if !next_consumed {
                                report(
                                    diagnostics,
                                    value,
                                    "not consumed before its next loop parameter definition",
                                    instruction.span,
                                );
                                return;
                            }
                            pending.push_back((edge.target, 0, false));
                        } else {
                            pending.push_back((edge.target, 0, next_consumed));
                        }
                    }
                    break;
                }
            }
        }
    }
}

fn edge_consumed(
    value: &str,
    consumed: bool,
    edge: &TextEdge,
    body: &FunctionBody,
    policies: &TypePolicies,
) -> Option<bool> {
    let target = body.blocks.get(edge.target)?;
    if consumed && edge.arguments.iter().any(|argument| argument == value) {
        return None;
    }
    let transferred = edge
        .arguments
        .iter()
        .zip(&target.parameters)
        .filter(|(argument, parameter)| {
            argument.as_str() == value
                && !matches!(
                    parameter.convention,
                    Some(Convention::Borrow | Convention::BorrowMut)
                )
                && !policies.is_trivial(&parameter.ty)
        })
        .count();
    if transferred > 1 || (transferred == 1 && consumed) {
        None
    } else {
        Some(consumed || transferred == 1)
    }
}

fn consumed_values(
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
    source: &str,
    call_conventions: &CallConventions,
) -> Vec<String> {
    let uses = crate::ssa::text_value_uses(instruction)
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    match instruction.opcode.as_str() {
        "move" | "destroy" | "into.raw" | "struct.destructure" | "tuple.destructure"
        | "enum.take" | "switch.enum" | "return" | "fail" | "resume" | "throw" | "coro.yield"
        | "coro.destroy" | "await" => uses.into_iter().take(1).collect(),
        "store" if matches!(memory_mode(instruction).as_deref(), Some("init" | "assign")) => {
            uses.into_iter().take(1).collect()
        }
        "struct.new" | "tuple.new" | "array.new" | "enum.new" => super::call_argument_names(
            instruction,
            crate::ssa::text_instruction_operands_start(&instruction.tokens),
        ),
        "closure.new" => uses.into_iter().skip(1).take(1).collect(),
        "guard" => uses.into_iter().skip(1).collect(),
        "deopt" => uses,
        "call" | "call.foreign" | "try.call" | "invoke" | "tail.call" | "coro.start" => {
            direct_call_arguments(instruction, call_conventions)
        }
        "call.variadic" => direct_call_arguments(instruction, call_conventions)
            .into_iter()
            .chain(super::variadic_argument_names(instruction))
            .collect(),
        "call.witness" | "call.vtable" => call_conventions
            .special_call(instruction, source)
            .map(|(arguments, conventions)| owned_arguments(arguments, &conventions))
            .unwrap_or_default(),
        "call.indirect" => indirect_call_arguments(source, instruction),
        "call.closure" => closure_call_arguments(instruction, value_types),
        _ => Vec::new(),
    }
}

fn direct_call_arguments(
    instruction: &TextInstruction,
    call_conventions: &CallConventions,
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
    owned_arguments(super::call_argument_names(instruction, end), conventions)
}

fn indirect_call_arguments(source: &str, instruction: &TextInstruction) -> Vec<String> {
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
    owned_arguments(
        super::call_argument_names_at(instruction, argument_open),
        &conventions,
    )
}

fn closure_call_arguments(
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
) -> Vec<String> {
    let uses = crate::ssa::text_value_uses(instruction);
    let Some((closure, _)) = uses.first() else {
        return Vec::new();
    };
    let Some(Type::Closure(closure)) = value_types.get(closure) else {
        return Vec::new();
    };
    let conventions = closure
        .signature
        .parameters
        .iter()
        .map(|parameter| parameter.convention)
        .collect::<Vec<_>>();
    owned_arguments(
        super::call_argument_names(
            instruction,
            crate::ssa::text_instruction_operands_start(&instruction.tokens),
        ),
        &conventions,
    )
}

fn owned_arguments(arguments: Vec<String>, conventions: &[Option<Convention>]) -> Vec<String> {
    arguments
        .into_iter()
        .zip(conventions)
        .filter_map(|(argument, convention)| {
            (!matches!(
                *convention,
                Some(Convention::Borrow | Convention::BorrowMut)
            ))
            .then_some(argument)
        })
        .collect()
}

fn memory_mode(instruction: &TextInstruction) -> Option<String> {
    let open = instruction
        .tokens
        .iter()
        .position(|token| matches!(token.kind, tir_format::text::TokenKind::Punctuation('[')))?;
    match instruction.tokens.get(open + 1).map(|token| &token.kind) {
        Some(tir_format::text::TokenKind::Atom(mode)) => Some(mode.clone()),
        _ => None,
    }
}

fn has_atom(instruction: &TextInstruction, expected: &str) -> bool {
    instruction.tokens.iter().any(
        |token| matches!(&token.kind, tir_format::text::TokenKind::Atom(atom) if atom == expected),
    )
}

fn consume(value: &str, consumed: bool, operands: &[String]) -> Option<bool> {
    let count = operands
        .iter()
        .filter(|operand| operand.as_str() == value)
        .count();
    if count > 1 || (count == 1 && consumed) {
        None
    } else {
        Some(consumed || count == 1)
    }
}

fn instruction_uses(instruction: &TextInstruction, value: &str) -> bool {
    crate::ssa::text_value_uses(instruction)
        .iter()
        .any(|(used, _)| used == value)
}

fn report(diagnostics: &mut Vec<Diagnostic>, value: &str, issue: &str, span: Span) {
    diagnostics.push(
        Diagnostic::error(V4, format!("owned value %{value} is {issue}")).with_primary_span(span),
    );
}
