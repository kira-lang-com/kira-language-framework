//! Copy/move resolution (section 11.6 pass 2). When an owned value is consumed
//! more than once within a block, an implicit-copy type (`ref`, `weak`) gets a
//! `copy` inserted before all but the last consuming use; an affine type
//! (`uniq`) is reported as `own.use-after-move`. A `move` marks its operand dead,
//! so any later consume is a use-after-move regardless of policy.

use crate::common::{body_consumes, is_owned, value_types};
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{Function, Instruction, ModuleIndex, Operation, Type};

const USE_AFTER_MOVE: &str = "KLF-KIR-1101";

fn implicit_copy(ty: Option<&Type>) -> bool {
    matches!(ty, Some(Type::Reference(_) | Type::Weak(_)))
}

pub(crate) fn resolve_copies(
    function: &mut Function,
    index: &ModuleIndex<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let types = value_types(function, index);
    let owned: Vec<Name> = types
        .iter()
        .filter(|(_, ty)| is_owned(Some(ty)))
        .map(|(name, _)| name.clone())
        .collect();

    for value in owned {
        let implicit = implicit_copy(types.get(&value));
        for block in &mut function.blocks {
            resolve_in_block(block, &value, implicit, diagnostics);
        }
    }
}

fn resolve_in_block(
    block: &mut klf_kir::Block,
    value: &Name,
    implicit: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let consumes: Vec<(usize, bool)> = block
        .instructions
        .iter()
        .enumerate()
        .filter_map(|(index, instruction)| {
            let count = body_consumes(&instruction.operation, value)
                + terminator_consumes(&instruction.operation, value);
            (count > 0).then_some((index, matches!(instruction.operation, Operation::Move(_))))
        })
        .collect();

    if consumes.len() < 2 {
        return;
    }

    // A move must be the last consume; any consume after a move is illegal.
    if consumes[..consumes.len() - 1]
        .iter()
        .any(|(_, is_move)| *is_move)
    {
        report_use_after_move(block, value, diagnostics);
        return;
    }
    if !implicit {
        report_use_after_move(block, value, diagnostics);
        return;
    }

    // Insert a copy before every consuming use but the last, and rewrite that
    // use to the copy so the original reaches the final consume.
    let last = consumes.last().map(|(index, _)| *index).unwrap_or_default();
    let mut rewritten = Vec::with_capacity(block.instructions.len());
    let mut counter = 0usize;
    for (index, instruction) in block.instructions.drain(..).enumerate() {
        if index != last && body_consumes(&instruction.operation, value) > 0 {
            let copy_name = Name::new(format!("{value}.copy{counter}"));
            counter += 1;
            rewritten.push(Instruction {
                results: vec![copy_name.clone()],
                operation: Operation::Copy(value.clone()),
                debug_location: None,
                span: instruction.span,
            });
            let operation = replace_consumed(&instruction.operation, value, &copy_name);
            rewritten.push(Instruction {
                operation,
                ..instruction
            });
        } else {
            rewritten.push(instruction);
        }
    }
    block.instructions = rewritten;
}

fn terminator_consumes(operation: &Operation, value: &Name) -> usize {
    match operation {
        Operation::Return(Some(name)) | Operation::Fail(name) => usize::from(name == value),
        _ => 0,
    }
}

fn replace_consumed(operation: &Operation, value: &Name, replacement: &Name) -> Operation {
    let swap = |name: &Name| {
        if name == value {
            replacement.clone()
        } else {
            name.clone()
        }
    };
    match operation {
        Operation::Move(name) => Operation::Move(swap(name)),
        Operation::Destroy(name) => Operation::Destroy(swap(name)),
        Operation::IntoRaw(name) => Operation::IntoRaw(swap(name)),
        Operation::Store {
            mode,
            value: stored,
            address,
        } => Operation::Store {
            mode: *mode,
            value: swap(stored),
            address: address.clone(),
        },
        Operation::Call {
            callee,
            generic_args,
            arguments,
        } => {
            let mut arguments = arguments.clone();
            if let Some(slot) = arguments.iter_mut().find(|argument| *argument == value) {
                *slot = replacement.clone();
            }
            Operation::Call {
                callee: callee.clone(),
                generic_args: generic_args.clone(),
                arguments,
            }
        }
        other => other.clone(),
    }
}

fn report_use_after_move(block: &klf_kir::Block, value: &Name, diagnostics: &mut Vec<Diagnostic>) {
    let span = block
        .instructions
        .last()
        .map_or(block.span, |instruction| instruction.span);
    diagnostics.push(
        Diagnostic::error(
            USE_AFTER_MOVE,
            format!("value `%{value}` is consumed more than once but its type is not copyable"),
        )
        .with_label(span, "own.use-after-move"),
    );
}
