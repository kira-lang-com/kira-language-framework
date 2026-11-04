use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_verify::{StructuredGraphBlock, verify_structured_graph};
use tvisa_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand};

const S7: Code = Code::new(Dialect::Tvisa, Area::ControlFlow, 7);

pub(super) fn verify(body: &BinaryFunctionBody, successors: &[Vec<usize>]) -> Vec<Diagnostic> {
    let blocks = body
        .blocks
        .iter()
        .map(|block| StructuredGraphBlock {
            loop_merge: block
                .loop_merge
                .and_then(|index| usize::try_from(index).ok()),
            loop_continue: block
                .loop_continue
                .and_then(|index| usize::try_from(index).ok()),
            selection: block
                .instructions
                .last()
                .is_some_and(|instruction| matches!(instruction.opcode, 0x1e1 | 0x1e2)),
            selection_merge: block
                .instructions
                .last()
                .and_then(|instruction| {
                    matches!(instruction.opcode, 0x1e1 | 0x1e2)
                        .then(|| selection_merge(instruction))
                        .flatten()
                })
                .and_then(|index| usize::try_from(index).ok()),
        })
        .collect::<Vec<_>>();
    let mut diagnostics = verify_structured_graph(&blocks, successors, Dialect::Tvisa);
    diagnostics.extend(
        body.blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|instruction| matches!(instruction.opcode, 0x1e4 | 0x1aa | 0x1d0))
            .map(|instruction| {
                Diagnostic::error(
                    S7,
                    format!(
                        "GPU instruction opcode 0x{:03X} is forbidden by structured control flow",
                        instruction.opcode
                    ),
                )
            }),
    );
    diagnostics
}

fn selection_merge(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .rev()
        .find_map(|operand| match operand {
            BinaryOperand::BlockTarget { block, .. } => Some(*block),
            _ => None,
        })
}
