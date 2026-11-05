use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::FunctionBody;
use tir_format::text::{Token, TokenKind};

use crate::dominators::DominatorTree;
use crate::text_cfg::Successor;

const S1: Code = Code::new(Dialect::Tir, Area::ControlFlow, 1);
const S2: Code = Code::new(Dialect::Tir, Area::ControlFlow, 2);
const S3: Code = Code::new(Dialect::Tir, Area::ControlFlow, 3);
const S4: Code = Code::new(Dialect::Tir, Area::ControlFlow, 4);
const S5: Code = Code::new(Dialect::Tir, Area::ControlFlow, 5);
const S6: Code = Code::new(Dialect::Tir, Area::ControlFlow, 6);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConstructKind {
    Loop { continue_block: usize },
    Selection,
}

struct Construct {
    header: usize,
    merge: usize,
    kind: ConstructKind,
}

pub(crate) fn verify_structured(
    body: &FunctionBody,
    successors: &[Vec<Successor>],
    labels: &HashMap<&str, usize>,
    enforce: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !enforce && !has_annotations(body) {
        return;
    }

    let dominators = compute_dominators(successors, labels);
    let mut constructs = Vec::new();
    for (header, block) in body.blocks.iter().enumerate() {
        if has_loop_annotation(&block.header_tokens) {
            let merge = label_after(&block.header_tokens, "merge");
            let continuation = label_after(&block.header_tokens, "continue");
            match (merge, continuation) {
                (Some((merge_label, merge_span)), Some((continue_label, _))) => {
                    if let (Some(merge_index), Some(continue_index)) = (
                        labels.get(merge_label).copied(),
                        labels.get(continue_label).copied(),
                    ) {
                        constructs.push(make_construct(
                            header,
                            merge_index,
                            ConstructKind::Loop {
                                continue_block: continue_index,
                            },
                        ));
                    } else {
                        let unknown = if !labels.contains_key(merge_label) {
                            merge_label
                        } else {
                            continue_label
                        };
                        diagnostics.push(
                            Diagnostic::error(
                                S1,
                                format!("loop annotation names unknown block {unknown}"),
                            )
                            .with_primary_span(merge_span),
                        );
                    }
                }
                _ => diagnostics.push(
                    Diagnostic::error(S1, "loop header must name merge and continue blocks")
                        .with_primary_span(block.span),
                ),
            }
        }
        if let Some(instruction) = block.instructions.last()
            && matches!(instruction.opcode.as_str(), "cond.br" | "switch.int")
        {
            if let Some((merge_label, merge_span)) = label_after(&instruction.tokens, "merge") {
                if let Some(merge_index) = labels.get(merge_label).copied() {
                    constructs.push(make_construct(
                        header,
                        merge_index,
                        ConstructKind::Selection,
                    ));
                } else {
                    diagnostics.push(
                        Diagnostic::error(
                            S3,
                            format!("selection names unknown merge block {merge_label}"),
                        )
                        .with_primary_span(merge_span),
                    );
                }
            } else if enforce {
                diagnostics.push(
                    Diagnostic::error(S3, "structured selection is missing its merge block")
                        .with_primary_span(instruction.span),
                );
            }
        }
    }

    if enforce {
        for (source, edges) in successors.iter().enumerate() {
            for edge in edges {
                let Some(target) = labels.get(edge.label.as_str()).copied() else {
                    continue;
                };
                if dominators.dominates(target, source)
                    && !constructs.iter().any(|construct| {
                        construct.header == target
                            && matches!(construct.kind, ConstructKind::Loop { .. })
                    })
                {
                    diagnostics.push(
                        Diagnostic::error(S1, "back edge enters a block without a loop annotation")
                            .with_primary_span(edge.span),
                    );
                }
            }
        }
    }

    verify_constructs(
        body,
        successors,
        labels,
        &constructs,
        &dominators,
        diagnostics,
    );
}

pub(crate) fn has_annotations(body: &FunctionBody) -> bool {
    body.blocks.iter().any(|block| {
        has_loop_annotation(&block.header_tokens)
            || block
                .instructions
                .iter()
                .any(|instruction| label_after(&instruction.tokens, "merge").is_some())
    })
}

fn make_construct(header: usize, merge: usize, kind: ConstructKind) -> Construct {
    Construct {
        header,
        merge,
        kind,
    }
}

impl Construct {
    fn contains(&self, block: usize, dominators: &DominatorTree) -> bool {
        dominators.dominates(self.header, block) && !dominators.dominates(self.merge, block)
    }
}

#[derive(Clone, Copy)]
struct ActiveIntervals {
    first: Option<(usize, usize)>,
    second: Option<(usize, usize)>,
}

impl ActiveIntervals {
    const EMPTY: Self = Self {
        first: None,
        second: None,
    };

    fn ranges(self) -> [Option<(usize, usize)>; 2] {
        [self.first, self.second]
    }
}

fn active_intervals(construct: &Construct, dominators: &DominatorTree) -> ActiveIntervals {
    let (start, end) = dominators.interval(construct.header);
    if !dominators.dominates(construct.header, construct.merge) {
        return ActiveIntervals {
            first: Some((start, end)),
            second: None,
        };
    }
    if construct.header == construct.merge {
        return ActiveIntervals::EMPTY;
    }
    let (merge_start, merge_end) = dominators.interval(construct.merge);
    ActiveIntervals {
        first: (start < merge_start).then_some((start, merge_start)),
        second: (merge_end < end).then_some((merge_end, end)),
    }
}

fn constructs_intersect(left: &Construct, right: &Construct, dominators: &DominatorTree) -> bool {
    let left = active_intervals(left, dominators).ranges();
    let right = active_intervals(right, dominators).ranges();
    left.into_iter().flatten().any(|left| {
        right
            .into_iter()
            .flatten()
            .any(|right| left.0 < right.1 && right.0 < left.1)
    })
}

fn construct_is_subset(
    subset: &Construct,
    superset: &Construct,
    dominators: &DominatorTree,
) -> bool {
    let superset = active_intervals(superset, dominators).ranges();
    active_intervals(subset, dominators)
        .ranges()
        .into_iter()
        .flatten()
        .all(|subset| {
            superset
                .into_iter()
                .flatten()
                .any(|superset| superset.0 <= subset.0 && subset.1 <= superset.1)
        })
}

fn verify_constructs(
    body: &FunctionBody,
    successors: &[Vec<Successor>],
    labels: &HashMap<&str, usize>,
    constructs: &[Construct],
    dominators: &DominatorTree,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut merges = HashMap::new();
    for construct in constructs {
        if let Some(previous) = merges.insert(construct.merge, construct.header) {
            diagnostics.push(
                Diagnostic::error(S5, "a block is the merge of more than one construct")
                    .with_primary_span(body.blocks[construct.header].span)
                    .with_note_at(body.blocks[previous].span, "the other construct is here"),
            );
        }
        if !dominators.dominates(construct.header, construct.merge) {
            diagnostics.push(
                Diagnostic::error(S3, "construct header does not dominate its merge block")
                    .with_primary_span(body.blocks[construct.merge].span),
            );
        }
        if let ConstructKind::Loop { continue_block } = construct.kind {
            if !construct.contains(continue_block, dominators) {
                diagnostics.push(
                    Diagnostic::error(S1, "loop header does not dominate its continue block")
                        .with_primary_span(body.blocks[continue_block].span),
                );
            }
            let mut back_edges = Vec::new();
            for (source, edges) in successors.iter().enumerate() {
                if construct.contains(source, dominators) {
                    for edge in edges {
                        if edge.label == body.blocks[construct.header].label {
                            back_edges.push((source, edge));
                        }
                    }
                }
            }
            if back_edges.len() != 1 || back_edges[0].0 != continue_block {
                diagnostics.push(
                    Diagnostic::error(
                        S1,
                        "the continue block must provide the loop's only back edge to its header",
                    )
                    .with_primary_span(body.blocks[continue_block].span),
                );
            }
        }
    }

    for left in 0..constructs.len() {
        for right in left + 1..constructs.len() {
            let a = &constructs[left];
            let b = &constructs[right];
            let intersects = constructs_intersect(a, b, dominators);
            if intersects
                && !construct_is_subset(a, b, dominators)
                && !construct_is_subset(b, a, dominators)
            {
                diagnostics.push(
                    Diagnostic::error(S4, "structured constructs overlap without nesting")
                        .with_primary_span(body.blocks[constructs[right].header].span)
                        .with_note_at(
                            body.blocks[constructs[left].header].span,
                            "the overlapping construct starts here",
                        ),
                );
            }
        }
    }

    for construct in constructs {
        let enclosing_loops = constructs
            .iter()
            .filter(|outer| {
                outer.header != construct.header
                    && matches!(outer.kind, ConstructKind::Loop { .. })
                    && construct_is_subset(construct, outer, dominators)
            })
            .collect::<Vec<_>>();
        for (source, edges) in successors.iter().enumerate() {
            if !construct.contains(source, dominators) {
                continue;
            }
            for edge in edges {
                let Some(target) = labels.get(edge.label.as_str()).copied() else {
                    continue;
                };
                if construct.contains(target, dominators) {
                    continue;
                }
                let allowed = target == construct.merge
                    || enclosing_loops.iter().any(|outer| {
                        target == outer.merge
                            || matches!(outer.kind, ConstructKind::Loop { continue_block } if target == continue_block)
                    });
                if !allowed {
                    let code = if matches!(construct.kind, ConstructKind::Loop { .. }) {
                        S2
                    } else {
                        S3
                    };
                    diagnostics.push(
                        Diagnostic::error(
                            code,
                            "edge leaves a structured construct at an invalid block",
                        )
                        .with_primary_span(edge.span),
                    );
                }
            }
        }
        for (source, edges) in successors.iter().enumerate() {
            if construct.contains(source, dominators) {
                continue;
            }
            for edge in edges {
                let Some(target) = labels.get(edge.label.as_str()).copied() else {
                    continue;
                };
                if construct.contains(target, dominators) && target != construct.header {
                    diagnostics.push(
                        Diagnostic::error(S6, "edge enters a construct other than at its header")
                            .with_primary_span(edge.span),
                    );
                }
            }
        }
    }
}

fn compute_dominators(
    successors: &[Vec<Successor>],
    labels: &HashMap<&str, usize>,
) -> DominatorTree {
    let adjacency = successors
        .iter()
        .map(|edges| {
            edges
                .iter()
                .filter_map(|edge| labels.get(edge.label.as_str()).copied())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    DominatorTree::compute(&adjacency)
}

fn label_after<'a>(tokens: &'a [Token], keyword: &str) -> Option<(&'a str, tir_diag::Span)> {
    for (index, token) in tokens.iter().enumerate() {
        if !matches!(&token.kind, TokenKind::Atom(atom) if atom == keyword)
            || matches!(
                index
                    .checked_sub(1)
                    .and_then(|previous| tokens.get(previous))
                    .map(|previous| &previous.kind),
                Some(TokenKind::Punctuation('.' | '%' | '$' | '@'))
            )
        {
            continue;
        }
        if let Some(Token {
            kind: TokenKind::Atom(label),
            span,
        }) = tokens.get(index + 1)
        {
            return Some((label, *span));
        }
    }
    None
}

fn has_loop_annotation(tokens: &[Token]) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        matches!(&token.kind, TokenKind::Atom(atom) if atom == "loop")
            && matches!(
                index
                    .checked_sub(1)
                    .and_then(|previous| tokens.get(previous))
                    .map(|previous| &previous.kind),
                Some(TokenKind::Punctuation(':'))
            )
            && matches!(
                tokens.get(index + 1).map(|next| &next.kind),
                Some(TokenKind::Punctuation('('))
            )
    })
}
