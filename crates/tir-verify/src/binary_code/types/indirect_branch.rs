//! V2 operand/provenance integrity for same-function block addresses and indirect branches.
use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryOperand, BinaryType, BinaryTypeTable};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function: &str,
) -> Vec<Diagnostic> {
    let mut origins = HashMap::new();
    let mut value_count = 0u64;
    for block in &body.blocks {
        value_count += block.parameters.len() as u64;
        for instruction in block.instructions.iter().flatten() {
            if instruction.opcode == 0x00f
                && instruction.result_count == 1
                && let [BinaryOperand::Index(target)] = instruction.operands.as_slice()
            {
                origins.insert(value_count, *target);
            }
            value_count += instruction.result_count;
        }
    }
    let mut diagnostics = Vec::new();
    for block in &body.blocks {
        for instruction in block.instructions.iter().flatten() {
            if instruction.opcode == 0x1e4 {
                let valid = match instruction.operands.as_slice() {
                    [
                        BinaryOperand::Value(address),
                        BinaryOperand::Indices(targets),
                    ] => {
                        instruction.terminator
                            && instruction.result_count == 0
                            && instruction.modifiers == 0
                            && usize::try_from(*address)
                                .ok()
                                .and_then(|index| value_types.get(index))
                                .copied()
                                .flatten()
                                .and_then(|ty| types.entries.get(ty as usize))
                                .is_some_and(|ty| matches!(ty, BinaryType::Pointer(0)))
                            && origins
                                .get(address)
                                .is_some_and(|target| targets.contains(target))
                            && !targets.is_empty()
                            && targets.iter().collect::<HashSet<_>>().len() == targets.len()
                            && targets.iter().all(|target| {
                                usize::try_from(*target)
                                    .ok()
                                    .and_then(|index| body.blocks.get(index))
                                    .is_some_and(|block| block.parameters.is_empty())
                            })
                    }
                    _ => false,
                };
                if !valid {
                    diagnostics.push(Diagnostic::error(
                        V2,
                        format!("function @{function} CODE offset {}: br.indirect must use a same-function block.addr and list unique parameterless targets including its address", instruction.offset),
                    ));
                }
            } else if origins
                .keys()
                .any(|origin| operands_reference(&instruction.operands, *origin))
            {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!("function @{function} CODE offset {}: block.addr value escapes outside br.indirect", instruction.offset),
                ));
            }
        }
    }
    diagnostics
}

fn operands_reference(operands: &[BinaryOperand], value: u64) -> bool {
    operands.iter().any(|operand| match operand {
        BinaryOperand::Value(id) | BinaryOperand::OptionalValue(Some(id)) => *id == value,
        BinaryOperand::Values(ids) => ids.contains(&value),
        BinaryOperand::BlockTarget { arguments, .. } => arguments.contains(&value),
        BinaryOperand::SwitchCases(cases) => cases.iter().any(|(_, _, args)| args.contains(&value)),
        _ => false,
    })
}
