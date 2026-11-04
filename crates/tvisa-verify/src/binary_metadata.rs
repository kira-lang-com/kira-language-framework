use std::collections::{HashMap, HashSet};

use tir_diag::Diagnostic;
use tir_format::{BinaryMetadata, BinaryMetadataTable};
use tir_verify::DominatorTree;
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, TvisaImage, TvisaValueTypeRecord,
};

pub(super) fn verify(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    let metadata = match image.metadata_table(memory_limit) {
        Ok(metadata) => metadata,
        Err(error) => return vec![error.diagnostic],
    };
    if metadata == BinaryMetadataTable::default() {
        return Vec::new();
    }
    let bodies = match image.function_bodies(memory_limit) {
        Ok(bodies) => bodies,
        Err(error) => return vec![error.diagnostic],
    };
    let body_by_index: HashMap<_, _> = bodies
        .iter()
        .map(|body| (body.header.function_index, body))
        .collect();
    let types = match image.type_section(memory_limit) {
        Ok(types) => types,
        Err(error) => return vec![error.diagnostic],
    };
    let functions = match image.function_records(memory_limit) {
        Ok(functions) => functions,
        Err(error) => return vec![error.diagnostic],
    };
    let imports = match image.import_counts(memory_limit) {
        Ok(imports) => imports,
        Err(error) => return vec![error.diagnostic],
    };
    let variants = match image.variant_index(memory_limit) {
        Ok(variants) => variants,
        Err(error) => return vec![error.diagnostic],
    };
    let typed_functions: HashSet<_> = metadata
        .attachments
        .iter()
        .filter(|attachment| {
            matches!(
                &attachment.metadata,
                BinaryMetadata::Range { .. } | BinaryMetadata::NonNull | BinaryMetadata::Align(_)
            )
        })
        .map(|attachment| attachment.function_index)
        .collect();
    let value_types: HashMap<_, _> = bodies
        .iter()
        .filter(|body| typed_functions.contains(&body.header.function_index))
        .filter_map(|body| {
            functions
                .get(usize::try_from(body.header.function_index).ok()?)
                .map(|function| {
                    (
                        body.header.function_index,
                        super::binary_code::gpu_value_types(
                            body, function, &types, &functions, &imports, &variants,
                        ),
                    )
                })
        })
        .collect();
    let loop_functions: HashSet<_> = metadata
        .attachments
        .iter()
        .filter(|attachment| matches!(&attachment.metadata, BinaryMetadata::LoopHints { .. }))
        .map(|attachment| attachment.function_index)
        .collect();
    let dominators: HashMap<_, _> = bodies
        .iter()
        .filter(|body| loop_functions.contains(&body.header.function_index))
        .map(|body| (body.header.function_index, body_dominators(body)))
        .collect();
    let constants = match image.constants(memory_limit) {
        Ok(constants) => constants,
        Err(error) => return vec![error.diagnostic],
    };
    let mut diagnostics = Vec::new();
    for attachment in &metadata.attachments {
        let Some(body) = body_by_index.get(&attachment.function_index).copied() else {
            diagnostics.push(Diagnostic::error(
                VFY1,
                format!(
                    "METADATA references function index {} without a defined CODE body",
                    attachment.function_index
                ),
            ));
            continue;
        };
        let Some((target_block, instruction)) =
            instruction_at(body, attachment.instruction_ordinal)
        else {
            diagnostics.push(Diagnostic::error(
                VFY1,
                format!(
                    "METADATA instruction ordinal {} is outside function index {}",
                    attachment.instruction_ordinal, attachment.function_index
                ),
            ));
            continue;
        };
        let function = attachment.function_index;
        let ordinal = attachment.instruction_ordinal;
        let function_value_types = value_types
            .get(&function)
            .map(Vec::as_slice)
            .unwrap_or_default();
        match &attachment.metadata {
            BinaryMetadata::BranchWeights(weights) => {
                if branch_successor_count(instruction) != Some(weights.len()) {
                    diagnostics.push(Diagnostic::error(
                        VFY1,
                        format!("branch-weight metadata on function {function} instruction {ordinal} does not match its successor count"),
                    ));
                }
            }
            BinaryMetadata::LoopHints { .. } => {
                let is_backedge = instruction.terminator
                    && dominators.get(&function).is_some_and(|dominators| {
                        block_targets(instruction).into_iter().any(|target| {
                            usize::try_from(target).ok().is_some_and(|target| {
                                body.blocks
                                    .get(target)
                                    .is_some_and(|block| block.loop_merge.is_some())
                                    && dominators.dominates(target, target_block)
                            })
                        })
                    });
                if !is_backedge {
                    diagnostics.push(Diagnostic::error(
                        VFY1,
                        format!("loop-hint metadata on function {function} instruction {ordinal} is not on a loop backedge"),
                    ));
                }
            }
            BinaryMetadata::Range {
                type_index,
                intervals,
            } => {
                let integer_type = usize::try_from(*type_index)
                    .ok()
                    .and_then(|index| types.value_types.get(index))
                    .is_some_and(|ty| matches!(ty, TvisaValueTypeRecord::Integer { .. }));
                if !integer_type || !is_load(instruction.opcode) {
                    diagnostics.push(Diagnostic::error(
                        VFY1,
                        format!("range metadata on function {function} instruction {ordinal} requires an integer-valued load type"),
                    ));
                }
                if result_type_at(body, ordinal, function_value_types) != Some(*type_index) {
                    diagnostics.push(Diagnostic::error(
                        VFY1,
                        format!("range metadata type does not match the loaded value in function {function} instruction {ordinal}"),
                    ));
                }
                verify_range_constants(
                    &mut diagnostics,
                    *type_index,
                    intervals,
                    &constants,
                    function,
                    ordinal,
                );
            }
            BinaryMetadata::NonNull | BinaryMetadata::Align(_) => {
                let result_is_pointer = result_type_at(body, ordinal, function_value_types)
                    .and_then(|type_index| {
                        usize::try_from(type_index)
                            .ok()
                            .and_then(|index| types.value_types.get(index))
                    })
                    .is_some_and(|ty| matches!(ty, TvisaValueTypeRecord::Pointer { .. }));
                if !is_load(instruction.opcode) || !result_is_pointer {
                    diagnostics.push(Diagnostic::error(
                        VFY1,
                        format!("pointer-load metadata on function {function} instruction {ordinal} requires a pointer-valued load"),
                    ));
                }
            }
            BinaryMetadata::InvariantLoad => {
                if !is_load(instruction.opcode) {
                    diagnostics.push(Diagnostic::error(
                        VFY1,
                        format!("load metadata on function {function} instruction {ordinal} is not attached to a load"),
                    ));
                }
            }
            BinaryMetadata::NoAlias(_)
            | BinaryMetadata::AliasScope(_)
            | BinaryMetadata::Tbaa(_) => {
                if !is_memory_operation(instruction.opcode) {
                    diagnostics.push(Diagnostic::error(
                        VFY1,
                        format!("alias metadata on function {function} instruction {ordinal} is not attached to a memory operation"),
                    ));
                }
            }
            BinaryMetadata::ProfileId(_) => diagnostics.push(Diagnostic::error(
                VFY1,
                "TVISA METADATA cannot contain profile-id attachments",
            )),
        }
    }
    diagnostics
}

fn verify_range_constants(
    diagnostics: &mut Vec<Diagnostic>,
    type_index: u64,
    intervals: &[(u64, u64)],
    constants: &[tvisa_format::ConstantRecord],
    function: u64,
    ordinal: u64,
) {
    let mut previous_upper: Option<Vec<u8>> = None;
    for &(lower_index, upper_index) in intervals {
        let lower = integer_constant(constants, lower_index, type_index);
        let upper = integer_constant(constants, upper_index, type_index);
        let (Some(lower), Some(upper)) = (lower, upper) else {
            diagnostics.push(Diagnostic::error(
                VFY1,
                format!("range metadata on function {function} instruction {ordinal} references a non-matching integer constant"),
            ));
            continue;
        };
        if !unsigned_less(&lower, &upper)
            || previous_upper
                .as_ref()
                .is_some_and(|previous| unsigned_less(&lower, previous))
        {
            diagnostics.push(Diagnostic::error(
                VFY1,
                format!("range metadata on function {function} instruction {ordinal} has empty, unordered, or overlapping intervals"),
            ));
        }
        previous_upper = Some(upper);
    }
}

fn integer_constant(
    constants: &[tvisa_format::ConstantRecord],
    index: u64,
    type_index: u64,
) -> Option<Vec<u8>> {
    let constant = constants.get(usize::try_from(index).ok()?)?;
    (constant.type_index == type_index).then(|| constant.value_bytes.clone())
}

fn unsigned_less(left: &[u8], right: &[u8]) -> bool {
    for index in (0..left.len().max(right.len())).rev() {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        if left_byte != right_byte {
            return left_byte < right_byte;
        }
    }
    false
}

fn instruction_at(body: &BinaryFunctionBody, ordinal: u64) -> Option<(usize, &BinaryInstruction)> {
    let mut current = 0u64;
    for (block_index, block) in body.blocks.iter().enumerate() {
        let end = current.checked_add(block.instructions.len() as u64)?;
        if ordinal < end {
            let index = usize::try_from(ordinal.checked_sub(current)?).ok()?;
            return Some((block_index, block.instructions.get(index)?));
        }
        current = end;
    }
    None
}

fn branch_successor_count(instruction: &BinaryInstruction) -> Option<usize> {
    match instruction.opcode {
        0x1e1 => Some(2),
        0x1e2 => instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::SwitchCases(cases) => cases.len().checked_add(1),
                _ => None,
            }),
        _ => None,
    }
}

fn block_targets(instruction: &BinaryInstruction) -> Vec<u64> {
    let mut targets = Vec::new();
    for operand in &instruction.operands {
        match operand {
            BinaryOperand::Block(block) => targets.push(*block),
            BinaryOperand::Blocks(blocks) => targets.extend_from_slice(blocks),
            BinaryOperand::BlockTarget { block, .. } => targets.push(*block),
            BinaryOperand::BlockTargets(blocks) => {
                targets.extend(blocks.iter().map(|(block, _)| *block));
            }
            BinaryOperand::SwitchCases(cases) => {
                targets.extend(cases.iter().map(|(_, block, _)| *block));
            }
            _ => {}
        }
    }
    targets
}

fn is_load(opcode: u64) -> bool {
    matches!(opcode, 0x140 | 0x141 | 0x146 | 0x200 | 0x270)
}

fn is_memory_operation(opcode: u64) -> bool {
    matches!(opcode, 0x140..=0x147 | 0x14a..=0x14c | 0x200..=0x203 | 0x270..=0x271)
}

fn body_dominators(body: &BinaryFunctionBody) -> DominatorTree {
    let successors = body
        .blocks
        .iter()
        .map(|block| {
            block
                .instructions
                .iter()
                .filter(|instruction| instruction.terminator)
                .flat_map(block_targets)
                .filter_map(|target| usize::try_from(target).ok())
                .filter(|target| *target < body.blocks.len())
                .collect()
        })
        .collect::<Vec<Vec<usize>>>();
    DominatorTree::compute(&successors)
}

fn result_type_at(
    body: &BinaryFunctionBody,
    ordinal: u64,
    value_types: &[Option<u64>],
) -> Option<u64> {
    let mut instruction_ordinal = 0u64;
    let mut value_index = 0usize;
    for block in &body.blocks {
        value_index = value_index.checked_add(block.parameter_types.len())?;
        for instruction in &block.instructions {
            if instruction_ordinal == ordinal {
                if instruction.result_count == 0 {
                    return None;
                }
                return value_types.get(value_index).copied().flatten();
            }
            instruction_ordinal = instruction_ordinal.checked_add(1)?;
            value_index = value_index.checked_add(usize::from(instruction.result_count))?;
        }
    }
    None
}

const VFY1: tir_diag::Code =
    tir_diag::Code::new(tir_diag::Dialect::Tvisa, tir_diag::Area::Verifier, 1);
