//! The values an overflow edge hands its path, rebuilt on the edge. A value
//! the hot code no longer needs once it branched should not stay alive for
//! the overflow path alone, nor be shared with it: a load shared with a
//! second user is no longer folded into the instruction that reads it.

use super::super::util::{fresh, instruction};
use klf_core::Name;
use klf_kir::{BinaryOperator, Block, ConvertKind, Instruction, LoadMode, Operation, Overflow};

/// Builds the instructions of an edge leaving `source`.
pub(super) struct Edge<'a> {
    source: &'a Block,
    undo: Option<Undo>,
    like: &'a Instruction,
    pub(super) instructions: Vec<Instruction>,
    rebuilt: Vec<(Name, Name)>,
}

/// An addition or subtraction whose overflow takes the edge. Its wrapped
/// result gives the left operand back, often the value a loop carries.
struct Undo {
    operator: BinaryOperator,
    result: Name,
    lhs: Name,
    rhs: Name,
}

impl<'a> Edge<'a> {
    pub(super) fn new(source: &'a Block, target: &Name, like: &'a Instruction) -> Self {
        Self {
            source,
            undo: overflowed(source, target),
            like,
            instructions: Vec::new(),
            rebuilt: Vec::new(),
        }
    }

    /// `value` as the edge computes it, or `value` itself.
    pub(super) fn value(&mut self, value: &Name) -> Name {
        if let Some((_, again)) = self.rebuilt.iter().find(|(old, _)| old == value) {
            return again.clone();
        }
        let again = if let Some((load, kind, ty)) = reread(self.source, value) {
            let loaded = fresh(value.as_str());
            self.push(loaded.clone(), load);
            let again = fresh(value.as_str());
            let convert = Operation::Convert {
                kind,
                value: loaded,
                ty,
            };
            self.push(again.clone(), convert);
            again
        } else if let Some(undo) = self
            .undo
            .as_ref()
            .filter(|undo| undo.lhs == *value && undo.lhs != undo.rhs)
        {
            let (operator, result, rhs) = (undo.operator, undo.result.clone(), undo.rhs.clone());
            let rhs = self.value(&rhs);
            let inverse = if operator == BinaryOperator::Add {
                BinaryOperator::Sub
            } else {
                BinaryOperator::Add
            };
            let again = fresh(value.as_str());
            let operation = Operation::Binary {
                operator: inverse,
                overflow: Overflow::Wrap,
                signed: false,
                lhs: result,
                rhs,
            };
            self.push(again.clone(), operation);
            again
        } else {
            return value.clone();
        };
        self.rebuilt.push((value.clone(), again.clone()));
        again
    }

    fn push(&mut self, result: Name, operation: Operation) {
        self.instructions
            .push(instruction(vec![result], operation, self.like));
    }
}

fn overflowed(block: &Block, target: &Name) -> Option<Undo> {
    let Some(Operation::ConditionalBranch(branch)) =
        block.instructions.last().map(|last| &last.operation)
    else {
        return None;
    };
    if branch.then_target.block != *target || branch.else_target.block == *target {
        return None;
    }
    block
        .instructions
        .iter()
        .find_map(|current| match &current.operation {
            Operation::BinaryOverflow {
                operator: operator @ (BinaryOperator::Add | BinaryOperator::Sub),
                lhs,
                rhs,
                ..
            } if current.results.get(1) == Some(&branch.condition) => Some(Undo {
                operator: *operator,
                result: current.results[0].clone(),
                lhs: lhs.clone(),
                rhs: rhs.clone(),
            }),
            _ => None,
        })
}

/// An extended load of `block` nothing after it writes over: the load and
/// the extension, read again on the edge.
fn reread(block: &Block, value: &Name) -> Option<(Operation, ConvertKind, klf_kir::Type)> {
    let (at, extended) = block
        .instructions
        .iter()
        .enumerate()
        .find(|(_, current)| current.results.first() == Some(value))?;
    let Operation::Convert {
        kind: kind @ (ConvertKind::ZeroExtend | ConvertKind::SignExtend),
        value: loaded,
        ty,
    } = &extended.operation
    else {
        return None;
    };
    let (start, load) = block.instructions[..at]
        .iter()
        .enumerate()
        .find(|(_, current)| current.results.first() == Some(loaded))?;
    if !matches!(
        load.operation,
        Operation::Load {
            mode: LoadMode::Trivial,
            ..
        }
    ) {
        return None;
    }
    let last = block.instructions.len() - 1;
    let untouched = block.instructions[start + 1..last].iter().all(|current| {
        super::super::clean::pure(&current.operation)
            || matches!(current.operation, Operation::BinaryOverflow { .. })
    });
    untouched.then(|| (load.operation.clone(), *kind, ty.clone()))
}
