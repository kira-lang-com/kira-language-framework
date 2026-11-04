use std::collections::{BTreeSet, HashSet, VecDeque};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::FunctionBody;

const V6: Code = Code::new(Dialect::Tir, Area::Verifier, 6);

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
            let mut scopes = incoming[block_index].clone().unwrap_or_default();
            let block = &body.blocks[block_index];
            for instruction in &block.instructions {
                match instruction.opcode.as_str() {
                    "borrow.begin" | "load.borrow" => {
                        if let [result] = instruction.results.as_slice() {
                            scopes.push(BTreeSet::from([result.clone()]));
                        }
                    }
                    "borrow.end" => {
                        let uses = crate::ssa::text_value_uses(instruction);
                        let borrowed = uses.first().map(|(name, _)| name.as_str());
                        match (borrowed, scopes.last()) {
                            (Some(borrowed), Some(open)) if open.contains(borrowed) => {
                                scopes.pop();
                            }
                            (Some(borrowed), Some(open)) => {
                                let innermost = open
                                    .iter()
                                    .map(|name| format!("%{name}"))
                                    .collect::<Vec<_>>()
                                    .join(" or ");
                                diagnostics.push(
                                    Diagnostic::error(
                                        V6,
                                        format!(
                                            "borrow.end %{borrowed} closes scopes out of order; innermost open scope includes {innermost}"
                                        ),
                                    )
                                    .with_primary_span(instruction.span),
                                );
                                if let Some(position) =
                                    scopes.iter().rposition(|scope| scope.contains(borrowed))
                                {
                                    scopes.truncate(position);
                                }
                            }
                            (Some(borrowed), None) => diagnostics.push(
                                Diagnostic::error(
                                    V6,
                                    format!("borrow.end %{borrowed} has no matching open scope"),
                                )
                                .with_primary_span(instruction.span),
                            ),
                            (None, _) => diagnostics.push(
                                Diagnostic::error(V6, "borrow.end requires a borrowed value")
                                    .with_primary_span(instruction.span),
                            ),
                        }
                    }
                    _ => {}
                }
            }

            if edges[block_index].is_empty() && !scopes.is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        V6,
                        format!(
                            "function exit leaves borrow scopes open: {}",
                            scopes
                                .iter()
                                .map(|aliases| {
                                    aliases.iter().next().map_or_else(
                                        || "<unknown>".to_owned(),
                                        |name| format!("%{name}"),
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                    .with_primary_span(block.span),
                );
            }

            for edge in &edges[block_index] {
                let target = edge.target;
                let mut edge_scopes = scopes.clone();
                for (argument, parameter) in
                    edge.arguments.iter().zip(&body.blocks[target].parameters)
                {
                    for scope in &mut edge_scopes {
                        if scope.contains(argument) {
                            scope.insert(parameter.name.clone());
                        }
                    }
                }
                match &mut incoming[target] {
                    None => {
                        incoming[target] = Some(edge_scopes);
                        queue.push_back(target);
                    }
                    Some(existing) => {
                        let compatible = existing.len() == edge_scopes.len()
                            && existing
                                .iter()
                                .zip(&edge_scopes)
                                .all(|(left, right)| left.iter().any(|name| right.contains(name)));
                        if !compatible && reported_joins.insert(target) {
                            diagnostics.push(
                                Diagnostic::error(
                                    V6,
                                    format!(
                                        "borrow scopes reaching block {} differ across control-flow paths",
                                        body.blocks[target].label
                                    ),
                                )
                                .with_primary_span(body.blocks[target].span),
                            );
                        } else if compatible {
                            for (left, right) in existing.iter_mut().zip(edge_scopes) {
                                left.extend(right);
                            }
                        }
                    }
                }
            }
        }
    }

    diagnostics
}
