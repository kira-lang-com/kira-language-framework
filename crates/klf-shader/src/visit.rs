//! Walks over statement trees, shared by the validator, passes and
//! backends.

use crate::arena::Handle;
use crate::ir::{Block, Expression, SampleLevel, Statement};

/// Calls `visit` on every statement of `block`, outer before inner.
pub fn statements<'a>(block: &'a Block, visit: &mut impl FnMut(&'a Statement)) {
    for statement in &block.statements {
        visit(statement);
        for inner in nested(statement) {
            statements(inner, visit);
        }
    }
}

/// Calls `visit` on every statement of `block`, outer before inner, with
/// mutable access.
pub fn statements_mut(block: &mut Block, visit: &mut impl FnMut(&mut Statement)) {
    for statement in &mut block.statements {
        visit(statement);
        for inner in nested_mut(statement) {
            statements_mut(inner, visit);
        }
    }
}

/// The blocks directly inside `statement`.
#[must_use]
pub fn nested(statement: &Statement) -> Vec<&Block> {
    match statement {
        Statement::Block(block) => vec![block],
        Statement::If { accept, reject, .. } => vec![accept, reject],
        Statement::Switch { cases, .. } => cases.iter().map(|case| &case.body).collect(),
        Statement::Loop {
            body, continuing, ..
        } => vec![body, continuing],
        _ => Vec::new(),
    }
}

fn nested_mut(statement: &mut Statement) -> Vec<&mut Block> {
    match statement {
        Statement::Block(block) => vec![block],
        Statement::If { accept, reject, .. } => vec![accept, reject],
        Statement::Switch { cases, .. } => cases.iter_mut().map(|case| &mut case.body).collect(),
        Statement::Loop {
            body, continuing, ..
        } => vec![body, continuing],
        _ => Vec::new(),
    }
}

/// Rewrites every expression handle `statement` names directly (not those
/// of nested blocks, and not an `Emit` range).
pub fn map_statement(
    statement: &mut Statement,
    map: &mut impl FnMut(Handle<Expression>) -> Handle<Expression>,
) {
    let mut one = |handle: &mut Handle<Expression>| *handle = map(*handle);
    match statement {
        Statement::If { condition, .. } => one(condition),
        Statement::Switch { selector, .. } => one(selector),
        Statement::Loop { break_if, .. } => {
            if let Some(condition) = break_if {
                one(condition);
            }
        }
        Statement::Return { value } => {
            if let Some(value) = value {
                one(value);
            }
        }
        Statement::Store { pointer, value } => {
            one(pointer);
            one(value);
        }
        Statement::ImageStore {
            image,
            coordinate,
            array_index,
            value,
        } => {
            one(image);
            one(coordinate);
            if let Some(index) = array_index {
                one(index);
            }
            one(value);
        }
        Statement::Atomic {
            pointer,
            value,
            compare,
            result,
            ..
        } => {
            one(pointer);
            one(value);
            for extra in [compare, result].into_iter().flatten() {
                one(extra);
            }
        }
        Statement::Call {
            arguments, result, ..
        } => {
            arguments.iter_mut().for_each(&mut one);
            if let Some(result) = result {
                one(result);
            }
        }
        Statement::WorkgroupUniformLoad { pointer, result } => {
            one(pointer);
            one(result);
        }
        Statement::Emit(_)
        | Statement::Block(_)
        | Statement::Break
        | Statement::Continue
        | Statement::Kill
        | Statement::ControlBarrier(_)
        | Statement::MemoryBarrier(_) => {}
    }
}

/// The expressions `statement` reads directly.
#[must_use]
pub fn statement_operands(statement: &Statement) -> Vec<Handle<Expression>> {
    match statement {
        Statement::If { condition, .. } => vec![*condition],
        Statement::Switch { selector, .. } => vec![*selector],
        Statement::Loop { break_if, .. } => break_if.iter().copied().collect(),
        Statement::Block(_) => Vec::new(),
        other => {
            let mut found = Vec::new();
            map_statement(&mut other.clone(), &mut |handle| {
                found.push(handle);
                handle
            });
            found
        }
    }
}

/// The level operands of a sample, in order.
#[must_use]
pub fn level_operands(level: &SampleLevel) -> Vec<Handle<Expression>> {
    match level {
        SampleLevel::Auto | SampleLevel::Zero => Vec::new(),
        SampleLevel::Exact(value) | SampleLevel::Bias(value) => vec![*value],
        SampleLevel::Gradient { x, y } => vec![*x, *y],
    }
}
