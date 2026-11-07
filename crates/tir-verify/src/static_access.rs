use std::collections::{BTreeSet, HashSet, VecDeque};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::TokenKind;
use tir_format::{FunctionBody, TextInstruction};

const V7: Code = Code::new(Dialect::Tir, Area::Verifier, 7);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Read,
    Modify,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OpenAccess {
    tokens: BTreeSet<String>,
    addresses: BTreeSet<String>,
    mode: Mode,
}

pub(super) fn verify(body: &FunctionBody) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if body.blocks.is_empty() {
        return diagnostics;
    }

    let edges = crate::text_cfg::text_edges(body);
    let mut incoming = vec![None::<Vec<OpenAccess>>; body.blocks.len()];
    let mut reported_conflicts = HashSet::new();
    let mut queue = VecDeque::new();
    for root in 0..body.blocks.len() {
        if incoming[root].is_some() {
            continue;
        }
        incoming[root] = Some(Vec::new());
        queue.push_back(root);

        while let Some(block_index) = queue.pop_front() {
            let mut open = incoming[block_index].clone().unwrap_or_default();
            for (instruction_index, instruction) in
                body.blocks[block_index].instructions.iter().enumerate()
            {
                match instruction.opcode.as_str() {
                    "access.begin" => begin_access(
                        instruction,
                        (block_index, instruction_index),
                        &mut reported_conflicts,
                        &mut open,
                        &mut diagnostics,
                    ),
                    "access.end" => end_access(instruction, &mut open),
                    _ => {}
                }
            }

            for edge in &edges[block_index] {
                let target = edge.target;
                let mut outgoing = open.clone();
                for (argument, parameter) in
                    edge.arguments.iter().zip(&body.blocks[target].parameters)
                {
                    for access in &mut outgoing {
                        if access.tokens.contains(argument) {
                            access.tokens.insert(parameter.name.clone());
                        }
                        if access.addresses.contains(argument) {
                            access.addresses.insert(parameter.name.clone());
                        }
                    }
                }

                match &mut incoming[target] {
                    None => {
                        incoming[target] = Some(outgoing);
                        queue.push_back(target);
                    }
                    Some(existing) => {
                        if merge_accesses(existing, outgoing) {
                            queue.push_back(target);
                        }
                    }
                }
            }
        }
    }
    diagnostics
}

fn begin_access(
    instruction: &TextInstruction,
    site: (usize, usize),
    reported_conflicts: &mut HashSet<(usize, usize)>,
    open: &mut Vec<OpenAccess>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some((static_access, mode)) = access_mode(instruction) else {
        return;
    };
    if !static_access {
        return;
    }
    let uses = crate::ssa::text_value_uses(instruction);
    let (Some(address), [token]) = (uses.first(), instruction.results.as_slice()) else {
        return;
    };
    let addresses = BTreeSet::from([address.0.clone()]);
    let conflict = open.iter().any(|existing| {
        !existing.addresses.is_disjoint(&addresses)
            && (mode == Mode::Modify || existing.mode == Mode::Modify)
    });
    if conflict && reported_conflicts.insert(site) {
        diagnostics.push(
            Diagnostic::error(
                V7,
                format!("conflicting static accesses to `%{}`", address.0),
            )
            .with_primary_span(instruction.span),
        );
    }
    open.push(OpenAccess {
        tokens: BTreeSet::from([token.clone()]),
        addresses,
        mode,
    });
}

fn end_access(instruction: &TextInstruction, open: &mut Vec<OpenAccess>) {
    let uses = crate::ssa::text_value_uses(instruction);
    let Some((token, _)) = uses.first() else {
        return;
    };
    open.retain(|access| !access.tokens.contains(token));
}

fn access_mode(instruction: &TextInstruction) -> Option<(bool, Mode)> {
    // Only bracketed modifiers count. SSA identifiers such as `%modify` and
    // `%dynamic` are ordinary names and must not influence access checking.
    let mut checking = true; // TIR default: static
    let mut mode = Mode::Read; // TIR default: read
    for group in instruction.tokens.windows(3) {
        let [open, value, close] = group else {
            continue;
        };
        if !matches!(&open.kind, TokenKind::Punctuation('['))
            || !matches!(&close.kind, TokenKind::Punctuation(']'))
        {
            continue;
        }
        if let TokenKind::Atom(atom) = &value.kind {
            match atom.as_str() {
                "read" => mode = Mode::Read,
                "modify" => mode = Mode::Modify,
                "static" => checking = true,
                "dynamic" | "unchecked" => checking = false,
                _ => return None,
            }
        }
    }
    Some((checking, mode))
}

fn merge_accesses(existing: &mut Vec<OpenAccess>, incoming: Vec<OpenAccess>) -> bool {
    let mut changed = false;
    for mut access in incoming {
        if let Some(current) = existing
            .iter_mut()
            .find(|current| !current.tokens.is_disjoint(&access.tokens))
        {
            let before = current.clone();
            current.tokens.append(&mut access.tokens);
            current.addresses.append(&mut access.addresses);
            if access.mode == Mode::Modify {
                current.mode = Mode::Modify;
            }
            changed |= *current != before;
        } else {
            existing.push(access);
            changed = true;
        }
    }
    changed
}
