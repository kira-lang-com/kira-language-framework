use std::collections::{BTreeSet, HashSet, VecDeque};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::FunctionBody;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(body: &FunctionBody) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if body.blocks.is_empty() {
        return diagnostics;
    }
    let edges = crate::text_cfg::text_edges(body);
    let mut incoming = vec![None::<Vec<BTreeSet<String>>>; body.blocks.len()];
    let mut reported_joins = HashSet::new();
    let mut queue = VecDeque::new();

    for root in 0..body.blocks.len() {
        if incoming[root].is_some() {
            continue;
        }
        incoming[root] = Some(Vec::new());
        queue.push_back(root);

        while let Some(block_index) = queue.pop_front() {
            let mut stack = incoming[block_index].clone().unwrap_or_default();
            let block = &body.blocks[block_index];
            for instruction in &block.instructions {
                match instruction.opcode.as_str() {
                    "stack.alloc" => {
                        if let [result] = instruction.results.as_slice() {
                            stack.push(BTreeSet::from([result.clone()]));
                        }
                    }
                    "stack.dealloc" => deallocate(instruction, &mut stack, &mut diagnostics),
                    _ => propagate_aliases(instruction, &mut stack),
                }
            }

            for edge in &edges[block_index] {
                let mut outgoing = stack.clone();
                for (argument, parameter) in edge
                    .arguments
                    .iter()
                    .zip(&body.blocks[edge.target].parameters)
                {
                    for aliases in &mut outgoing {
                        if aliases.contains(argument) {
                            aliases.insert(parameter.name.clone());
                        }
                    }
                }
                match incoming[edge.target].as_mut() {
                    None => {
                        incoming[edge.target] = Some(outgoing);
                        queue.push_back(edge.target);
                    }
                    Some(existing) => {
                        if existing.len() != outgoing.len() {
                            if reported_joins.insert(edge.target) {
                                diagnostics.push(
                                    Diagnostic::error(
                                        V2,
                                        format!(
                                            "stack allocations reaching block {} differ in depth across control-flow paths",
                                            body.blocks[edge.target].label
                                        ),
                                    )
                                    .with_primary_span(body.blocks[edge.target].span),
                                );
                            }
                            continue;
                        }
                        let compatible = existing
                            .iter()
                            .zip(&outgoing)
                            .all(|(left, right)| !left.is_disjoint(right));
                        if !compatible {
                            if reported_joins.insert(edge.target) {
                                diagnostics.push(
                                    Diagnostic::error(
                                        V2,
                                        format!(
                                            "stack allocation order reaching block {} differs across control-flow paths",
                                            body.blocks[edge.target].label
                                        ),
                                    )
                                    .with_primary_span(body.blocks[edge.target].span),
                                );
                            }
                            continue;
                        }
                        let mut changed = false;
                        for (left, right) in existing.iter_mut().zip(outgoing) {
                            let before = left.len();
                            left.extend(right);
                            changed |= before != left.len();
                        }
                        if changed {
                            queue.push_back(edge.target);
                        }
                    }
                }
            }
        }
    }
    diagnostics
}

fn deallocate(
    instruction: &tir_format::TextInstruction,
    stack: &mut Vec<BTreeSet<String>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !instruction.results.is_empty() {
        diagnostics.push(
            Diagnostic::error(V2, "stack.dealloc must not define a value")
                .with_primary_span(instruction.span),
        );
    }
    let uses = crate::ssa::text_value_uses(instruction);
    let Some((value, _)) = uses.first() else {
        diagnostics.push(
            Diagnostic::error(V2, "stack.dealloc requires a stack allocation address")
                .with_primary_span(instruction.span),
        );
        return;
    };
    if stack.last().is_some_and(|aliases| aliases.contains(value)) {
        stack.pop();
        return;
    }
    if let Some(position) = stack.iter().rposition(|aliases| aliases.contains(value)) {
        diagnostics.push(
            Diagnostic::error(
                V2,
                format!(
                    "stack.dealloc %{value} violates LIFO order; a more recent allocation is still active"
                ),
            )
            .with_primary_span(instruction.span),
        );
        stack.truncate(position);
    } else {
        diagnostics.push(
            Diagnostic::error(
                V2,
                format!("stack.dealloc %{value} does not name an active stack allocation"),
            )
            .with_primary_span(instruction.span),
        );
    }
}

fn propagate_aliases(instruction: &tir_format::TextInstruction, stack: &mut [BTreeSet<String>]) {
    let [result] = instruction.results.as_slice() else {
        return;
    };
    let uses = crate::ssa::text_value_uses(instruction);
    let sources = match instruction.opcode.as_str() {
        "addr.cast" | "addr.toptr" | "ptr.toaddr" | "freeze" => {
            uses.first().map(|(name, _)| vec![name.as_str()])
        }
        "select" if uses.len() >= 3 => Some(vec![
            uses[uses.len() - 2].0.as_str(),
            uses[uses.len() - 1].0.as_str(),
        ]),
        _ => None,
    };
    let Some(sources) = sources else {
        return;
    };
    for aliases in stack {
        if sources.iter().all(|source| aliases.contains(*source)) {
            aliases.insert(result.clone());
        }
    }
}
