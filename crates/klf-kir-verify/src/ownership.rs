//! Verifier rule V4 (section 11.7): every owned value is consumed exactly once
//! on every path to `return`, `fail` or `unreachable`; paths ending in `trap`
//! are exempt. This runs only on canonical KIR, after the mandatory passes have
//! inserted the copies and destroys that make ownership balance.

use crate::Definition;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{AccessEnforcement, AccessMode, BlockTarget, Function, Operation, Type};
use rustc_hash::FxHashMap as HashMap;
use std::collections::{BTreeMap, BTreeSet};

const V4: &str = "KLF-KIR-1004";
const V5: &str = "KLF-KIR-1005";
const V6: &str = "KLF-KIR-1006";
const V7: &str = "KLF-KIR-1007";
const OWN_CONSUME_WHILE_BORROWED: &str = "KLF-KIR-1102";

/// Verifier rule V7 (section 11.7): two overlapping `static` accesses to the
/// same storage must not conflict — two modifies, or a modify and a read.
/// Storage is approximated by the address value; scopes are tracked with a
/// forward open-access dataflow.
pub(crate) fn verify_accesses(
    function: &Function,
    blocks: &HashMap<Name, usize>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let count = function.blocks.len();
    if count == 0 {
        return;
    }
    let mut open_in: Vec<Option<BTreeMap<Name, (Name, AccessMode)>>> = vec![None; count];
    open_in[0] = Some(BTreeMap::new());
    let mut work = vec![0usize];
    while let Some(block_index) = work.pop() {
        let mut open = open_in[block_index].clone().unwrap_or_default();
        let block = &function.blocks[block_index];
        for instruction in &block.instructions {
            match &instruction.operation {
                Operation::AccessBegin {
                    mode,
                    enforcement: AccessEnforcement::Static,
                    address,
                } => {
                    let conflict = open.values().any(|(other_address, other_mode)| {
                        other_address == address
                            && (*mode == AccessMode::Modify || *other_mode == AccessMode::Modify)
                    });
                    if conflict {
                        diagnostics.push(
                            Diagnostic::error(
                                V7,
                                format!("conflicting static accesses to `%{address}`"),
                            )
                            .with_label(instruction.span, "overlapping static access (V7)"),
                        );
                    }
                    if let [token] = instruction.results.as_slice() {
                        open.insert(token.clone(), (address.clone(), *mode));
                    }
                }
                Operation::AccessEnd(token) => {
                    open.remove(token);
                }
                _ => {}
            }
        }
        for target in block
            .instructions
            .last()
            .map(|i| i.operation.targets())
            .unwrap_or_default()
        {
            if let Some(&successor) = blocks.get(&target.block)
                && open_in[successor].is_none()
            {
                open_in[successor] = Some(open.clone());
                work.push(successor);
            }
        }
    }
}

fn is_owned(ty: Option<&Type>) -> bool {
    matches!(
        ty,
        Some(Type::Reference(_) | Type::Unique(_) | Type::Weak(_))
    )
}

/// Count how many times a non-terminator operation consumes `value`.
fn consumed_by(operation: &Operation, value: &Name) -> usize {
    let count = |name: &Name| usize::from(name == value);
    match operation {
        Operation::Move(name) | Operation::Destroy(name) | Operation::IntoRaw(name) => count(name),
        Operation::Store { value: stored, .. } => count(stored),
        Operation::Call { arguments, .. } => arguments.iter().map(count).sum(),
        _ => 0,
    }
}

fn edge_consumes(target: &BlockTarget, value: &Name) -> usize {
    target.arguments.iter().filter(|arg| *arg == value).count()
}

/// Verify V4 for every owned SSA value in `function`.
pub(crate) fn verify_ownership(
    function: &Function,
    definitions: &HashMap<Name, Definition>,
    blocks: &HashMap<Name, usize>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let borrowed: BTreeSet<&Name> = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches!(instruction.operation, Operation::BorrowBegin(_)))
        .flat_map(|instruction| instruction.results.iter())
        .collect();

    for (name, definition) in definitions {
        // Borrowed values carry a `ref` type but are not owned; V6 covers them.
        if !is_owned(definition.ty.as_ref()) || borrowed.contains(name) {
            continue;
        }
        let mut checker = Checker {
            function,
            blocks,
            value: name,
            entry_state: HashMap::default(),
            diagnostics,
            reported: false,
        };
        checker.run(definition.block, definition.position);
    }
}

/// The operands a non-terminator or exit operation consumes.
fn consuming_operands(operation: &Operation) -> Vec<&Name> {
    match operation {
        Operation::Move(name)
        | Operation::Destroy(name)
        | Operation::IntoRaw(name)
        | Operation::Fail(name)
        | Operation::Store { value: name, .. }
        | Operation::Return(Some(name)) => vec![name],
        Operation::Call { arguments, .. } => arguments.iter().collect(),
        _ => Vec::new(),
    }
}

/// Verifier rules V5 and V6: borrowed values (the results of `borrow.begin`) are
/// never consumed, and every borrow scope is matched and closed on every path.
pub(crate) fn verify_borrows(
    function: &Function,
    blocks: &HashMap<Name, usize>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let borrowed: BTreeSet<Name> = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches!(instruction.operation, Operation::BorrowBegin(_)))
        .flat_map(|instruction| instruction.results.iter().cloned())
        .collect();

    // V5: a borrowed value must not be consumed, stored, returned or transferred.
    for block in &function.blocks {
        for instruction in &block.instructions {
            let mut consumed = consuming_operands(&instruction.operation);
            for target in instruction.operation.targets() {
                consumed.extend(target.arguments.iter());
            }
            for name in consumed {
                if borrowed.contains(name) {
                    diagnostics.push(
                        Diagnostic::error(V5, format!("borrowed value `%{name}` is consumed"))
                            .with_label(instruction.span, "a borrow may not be consumed (V5)"),
                    );
                }
            }
        }
    }

    verify_scopes(function, blocks, diagnostics);
}

fn verify_scopes(
    function: &Function,
    blocks: &HashMap<Name, usize>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let count = function.blocks.len();
    // Each open borrow maps its token to the value it borrows.
    let mut open_in: Vec<Option<BTreeMap<Name, Name>>> = vec![None; count];
    if count == 0 {
        return;
    }
    open_in[0] = Some(BTreeMap::new());
    let mut work = vec![0usize];
    while let Some(block_index) = work.pop() {
        let mut open = open_in[block_index].clone().unwrap_or_default();
        let block = &function.blocks[block_index];
        for instruction in &block.instructions {
            let mut consumed = consuming_operands(&instruction.operation);
            for target in instruction.operation.targets() {
                consumed.extend(target.arguments.iter());
            }
            for name in consumed {
                if open.values().any(|borrowed| borrowed == name) {
                    diagnostics.push(
                        Diagnostic::error(
                            OWN_CONSUME_WHILE_BORROWED,
                            format!("value `%{name}` is consumed while it is borrowed"),
                        )
                        .with_label(instruction.span, "own.consume-while-borrowed"),
                    );
                }
            }
            match &instruction.operation {
                Operation::BorrowBegin(value) => {
                    for result in &instruction.results {
                        open.insert(result.clone(), value.clone());
                    }
                }
                Operation::BorrowEnd(value) => {
                    let was_open = open.remove(value).is_some();
                    if !was_open {
                        diagnostics.push(
                            Diagnostic::error(
                                V6,
                                format!("`borrow.end %{value}` has no matching open borrow"),
                            )
                            .with_label(instruction.span, "unmatched borrow.end (V6)"),
                        );
                    }
                }
                _ => {}
            }
        }
        let terminator = &block.instructions.last().map(|i| &i.operation);
        let exits = matches!(
            terminator,
            Some(Operation::Return(_) | Operation::Fail(_) | Operation::Unreachable)
        );
        if exits && !open.is_empty() {
            diagnostics.push(
                Diagnostic::error(V6, "a borrow scope is still open at a function exit")
                    .with_label(function.span, "unclosed borrow scope (V6)"),
            );
        }
        for target in block
            .instructions
            .last()
            .map(|i| i.operation.targets())
            .unwrap_or_default()
        {
            let Some(&successor) = blocks.get(&target.block) else {
                continue;
            };
            match &open_in[successor] {
                Some(existing) if *existing != open => diagnostics.push(
                    Diagnostic::error(V6, "borrow scopes differ on the paths into a block")
                        .with_label(function.span, "inconsistent borrow scopes (V6)"),
                ),
                Some(_) => {}
                None => {
                    open_in[successor] = Some(open.clone());
                    work.push(successor);
                }
            }
        }
    }
}

struct Checker<'a> {
    function: &'a Function,
    blocks: &'a HashMap<Name, usize>,
    value: &'a Name,
    entry_state: HashMap<usize, bool>,
    diagnostics: &'a mut Vec<Diagnostic>,
    reported: bool,
}

impl Checker<'_> {
    fn run(&mut self, def_block: usize, position: Option<usize>) {
        let mut work = Vec::new();
        match position {
            None => {
                self.entry_state.insert(def_block, false);
                work.push((def_block, false, 0usize));
            }
            Some(index) => work.push((def_block, false, index + 1)),
        }
        while let Some((block, consumed, start)) = work.pop() {
            self.walk(block, consumed, start, &mut work);
            if self.reported {
                return;
            }
        }
    }

    fn walk(
        &mut self,
        block: usize,
        mut consumed: bool,
        start: usize,
        work: &mut Vec<(usize, bool, usize)>,
    ) {
        let kir_block = &self.function.blocks[block];
        for instruction in kir_block.instructions.iter().skip(start) {
            if instruction.operation.is_terminator() {
                self.terminator(&instruction.operation, consumed, work);
                return;
            }
            let count = consumed_by(&instruction.operation, self.value);
            if count + usize::from(consumed) > 1 {
                self.report("consumed more than once");
                return;
            }
            if count > 0 {
                consumed = true;
            }
        }
    }

    fn terminator(
        &mut self,
        operation: &Operation,
        consumed: bool,
        work: &mut Vec<(usize, bool, usize)>,
    ) {
        match operation {
            Operation::Return(value) => {
                let here = value.as_ref() == Some(self.value);
                self.exit(consumed, here, "return");
            }
            Operation::Fail(value) => {
                let here = value == self.value;
                self.exit(consumed, here, "fail");
            }
            Operation::Unreachable => self.exit(consumed, false, "unreachable"),
            Operation::Branch(target) => self.edge(target, consumed, work),
            Operation::ConditionalBranch(branch) => {
                let klf_kir::ConditionalBranch {
                    then_target,
                    else_target,
                    ..
                } = &**branch;
                self.edge(then_target, consumed, work);
                self.edge(else_target, consumed, work);
            }
            Operation::SwitchInt(switch) => {
                let klf_kir::IntegerSwitch { cases, default, .. } = &**switch;
                for (_, target) in cases {
                    self.edge(target, consumed, work);
                }
                self.edge(default, consumed, work);
            }
            Operation::SwitchEnum(switch) => {
                let klf_kir::EnumSwitch { cases, default, .. } = &**switch;
                for (_, target) in cases {
                    self.edge(target, consumed, work);
                }
                if let Some(default) = default {
                    self.edge(default, consumed, work);
                }
            }
            // `trap` exits are exempt from V4; other operations are not
            // terminators and never reach here.
            _ => {}
        }
    }

    fn exit(&mut self, consumed: bool, consumed_here: bool, terminator: &str) {
        if consumed && consumed_here {
            self.report("consumed more than once");
            return;
        }
        if !(consumed || consumed_here) {
            self.report(&format!("not consumed on a path to `{terminator}`"));
        }
    }

    fn edge(&mut self, target: &BlockTarget, consumed: bool, work: &mut Vec<(usize, bool, usize)>) {
        let taken = edge_consumes(target, self.value);
        if taken + usize::from(consumed) > 1 {
            self.report("consumed more than once");
            return;
        }
        let state = consumed || taken > 0;
        let Some(&successor) = self.blocks.get(&target.block) else {
            return;
        };
        match self.entry_state.get(&successor) {
            Some(&previous) if previous != state => {
                self.report("consumed on some paths but not others");
            }
            Some(_) => {}
            None => {
                self.entry_state.insert(successor, state);
                work.push((successor, state, 0));
            }
        }
    }

    fn report(&mut self, problem: &str) {
        if self.reported {
            return;
        }
        self.reported = true;
        self.diagnostics.push(
            Diagnostic::error(V4, format!("owned value `%{}` is {problem}", self.value))
                .with_label(self.function.span, "ownership is not balanced (V4)"),
        );
    }
}
