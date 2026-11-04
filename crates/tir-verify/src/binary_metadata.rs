use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryCodeTable, BinaryConstantTable, BinaryFunctionBody, BinaryImport, BinaryInstruction,
    BinaryMetadata, BinaryMetadataTable, BinaryOperand, BinarySignatureTable, BinaryType,
    BinaryTypeTable,
};

use crate::DominatorTree;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(
    metadata: &BinaryMetadataTable,
    signatures: &BinarySignatureTable,
    code: &BinaryCodeTable,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    imports: Option<&[BinaryImport]>,
) -> Vec<Diagnostic> {
    let bodies: HashMap<_, _> = code
        .functions
        .iter()
        .map(|body| (body.function_index, body))
        .collect();
    let loop_functions: HashSet<_> = metadata
        .attachments
        .iter()
        .filter(|attachment| matches!(&attachment.metadata, BinaryMetadata::LoopHints { .. }))
        .map(|attachment| attachment.function_index)
        .collect();
    let dominators: HashMap<_, _> = code
        .functions
        .iter()
        .filter(|body| loop_functions.contains(&body.function_index))
        .map(|body| (body.function_index, body_dominators(body)))
        .collect();
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
    let value_types: HashMap<_, _> = code
        .functions
        .iter()
        .filter(|body| typed_functions.contains(&body.function_index))
        .map(|body| {
            (
                body.function_index,
                super::binary_code::infer_value_types(body, signatures, types, imports),
            )
        })
        .collect();
    let mut diagnostics = Vec::new();
    for attachment in &metadata.attachments {
        let Some(body) = bodies.get(&attachment.function_index).copied() else {
            diagnostics.push(Diagnostic::error(
                V2,
                format!(
                    "METADATA references function index {} without a CODE body",
                    attachment.function_index
                ),
            ));
            continue;
        };
        let Some((block_index, instruction_index, instruction)) =
            instruction_at(body, attachment.instruction_ordinal)
        else {
            diagnostics.push(Diagnostic::error(
                V2,
                format!(
                    "METADATA instruction ordinal {} is outside function index {}",
                    attachment.instruction_ordinal, attachment.function_index
                ),
            ));
            continue;
        };
        verify_attachment(
            &mut diagnostics,
            attachment,
            body,
            block_index,
            instruction_index,
            instruction,
            types,
            constants,
            dominators.get(&attachment.function_index),
            value_types
                .get(&attachment.function_index)
                .map(Vec::as_slice),
        );
    }
    diagnostics
}

fn verify_attachment(
    diagnostics: &mut Vec<Diagnostic>,
    attachment: &tir_format::BinaryMetadataAttachment,
    body: &BinaryFunctionBody,
    block_index: usize,
    instruction_index: usize,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    dominators: Option<&DominatorTree>,
    value_types: Option<&[Option<u64>]>,
) {
    let function = attachment.function_index;
    let ordinal = attachment.instruction_ordinal;
    match &attachment.metadata {
        BinaryMetadata::BranchWeights(weights) => {
            let successor_count = branch_successor_count(instruction);
            if successor_count != Some(weights.len()) {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "branch-weight metadata on function {function} instruction {ordinal} does not match its successor count"
                    ),
                ));
            }
        }
        BinaryMetadata::LoopHints { .. } => {
            let is_backedge = instruction.terminator
                && dominators.is_some_and(|dominators| {
                    block_targets(instruction).into_iter().any(|target| {
                        usize::try_from(target).ok().is_some_and(|target| {
                            body.blocks
                                .get(target)
                                .is_some_and(|block| block.loop_merge.is_some())
                                && dominators.dominates(target, block_index)
                        })
                    })
                });
            if !is_backedge {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "loop-hint metadata on function {function} instruction {ordinal} is not on a loop backedge"
                    ),
                ));
            }
        }
        BinaryMetadata::Range {
            type_index,
            intervals,
        } => {
            if !matches!(
                usize::try_from(*type_index)
                    .ok()
                    .and_then(|index| types.entries.get(index)),
                Some(BinaryType::Integer(_))
            ) {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!("range metadata on function {function} instruction {ordinal} has a non-integer type"),
                ));
            }
            if !is_load(instruction.opcode) {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!("range metadata on function {function} instruction {ordinal} is not attached to a load"),
                ));
            }
            let result_type = value_types.and_then(|value_types| {
                result_type_at(body, attachment.instruction_ordinal, value_types)
            });
            if result_type != Some(*type_index) {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!("range metadata type does not match the loaded value in function {function} instruction {ordinal}"),
                ));
            }
            verify_range_constants(
                diagnostics,
                *type_index,
                intervals,
                constants,
                function,
                ordinal,
            );
        }
        BinaryMetadata::NonNull => {
            let result_type = value_types.and_then(|value_types| {
                result_type_at(body, attachment.instruction_ordinal, value_types)
            });
            let is_pointer = result_type
                .and_then(|ty| usize::try_from(ty).ok())
                .and_then(|index| types.entries.get(index))
                .is_some_and(is_nonnull_type);
            if !is_load(instruction.opcode) || !is_pointer {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!("nonnull metadata on function {function} instruction {ordinal} requires a pointer-valued load"),
                ));
            }
        }
        BinaryMetadata::Align(_) => {
            let result_type = value_types.and_then(|value_types| {
                result_type_at(body, attachment.instruction_ordinal, value_types)
            });
            let is_pointer = result_type
                .and_then(|ty| usize::try_from(ty).ok())
                .and_then(|index| types.entries.get(index))
                .is_some_and(is_nonnull_type);
            if !is_load(instruction.opcode) || !is_pointer {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!("alignment metadata on function {function} instruction {ordinal} requires a pointer-valued load"),
                ));
            }
        }
        BinaryMetadata::NoAlias(_) | BinaryMetadata::AliasScope(_) | BinaryMetadata::Tbaa(_) => {
            if !is_memory_operation(instruction.opcode) {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!("alias metadata on function {function} instruction {ordinal} is not attached to a memory operation"),
                ));
            }
        }
        BinaryMetadata::ProfileId(_) => {}
        BinaryMetadata::InvariantLoad => {
            if !is_load(instruction.opcode) {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!("invariant-load metadata on function {function} instruction {ordinal} is not attached to a load"),
                ));
            }
        }
    }
    let _ = (block_index, instruction_index);
}

fn verify_range_constants(
    diagnostics: &mut Vec<Diagnostic>,
    type_index: u64,
    intervals: &[(u64, u64)],
    constants: &BinaryConstantTable,
    function: u64,
    ordinal: u64,
) {
    let mut previous_upper: Option<Vec<u8>> = None;
    for &(lower_index, upper_index) in intervals {
        let lower = integer_constant(constants, lower_index, type_index);
        let upper = integer_constant(constants, upper_index, type_index);
        let (Some(lower), Some(upper)) = (lower, upper) else {
            diagnostics.push(Diagnostic::error(
                V2,
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
                V2,
                format!("range metadata on function {function} instruction {ordinal} has empty, unordered, or overlapping intervals"),
            ));
        }
        previous_upper = Some(upper);
    }
}

fn integer_constant(
    constants: &BinaryConstantTable,
    index: u64,
    type_index: u64,
) -> Option<Vec<u8>> {
    let constant = constants.records.get(usize::try_from(index).ok()?)?;
    if constant.ty != type_index {
        return None;
    }
    match &constant.value {
        tir_format::BinaryConstantValue::Integer(bytes) => Some(bytes.clone()),
        tir_format::BinaryConstantValue::Zero => Some(vec![0]),
        _ => None,
    }
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

fn result_type_at(
    body: &BinaryFunctionBody,
    ordinal: u64,
    value_types: &[Option<u64>],
) -> Option<u64> {
    let mut instruction_ordinal = 0u64;
    let mut value_index = 0usize;
    for block in &body.blocks {
        value_index = value_index.checked_add(block.parameters.len())?;
        let instructions = block.instructions.as_deref()?;
        for instruction in instructions {
            if instruction_ordinal == ordinal {
                if instruction.result_count == 0 {
                    return None;
                }
                return value_types.get(value_index).copied().flatten();
            }
            instruction_ordinal = instruction_ordinal.checked_add(1)?;
            value_index =
                value_index.checked_add(usize::try_from(instruction.result_count).ok()?)?;
        }
    }
    None
}

fn instruction_at(
    body: &BinaryFunctionBody,
    ordinal: u64,
) -> Option<(usize, usize, &BinaryInstruction)> {
    let mut current = 0u64;
    for (block_index, block) in body.blocks.iter().enumerate() {
        let end = current.checked_add(block.instruction_count)?;
        if ordinal < end {
            let instruction_index = usize::try_from(ordinal.checked_sub(current)?).ok()?;
            return Some((
                block_index,
                instruction_index,
                block.instructions.as_deref()?.get(instruction_index)?,
            ));
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
            BinaryOperand::BlockTarget { block, .. } => targets.push(*block),
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
                .as_deref()
                .unwrap_or_default()
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

fn is_nonnull_type(ty: &BinaryType) -> bool {
    matches!(
        ty,
        BinaryType::Pointer(_)
            | BinaryType::Reference { .. }
            | BinaryType::Unique { .. }
            | BinaryType::Weak { .. }
    )
}
