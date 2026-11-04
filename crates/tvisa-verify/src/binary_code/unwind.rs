use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, FunctionRecordSummary, ImportCounts,
    SignatureSummary, TvisaValueTypeRecord, TypeSection, UnwindFunctionRecord, UnwindSection,
};

const VFY16: Code = Code::new(Dialect::Tvisa, Area::Verifier, 16);
const NOUNWIND: u64 = 1 << 1;
const ADDRESS_TAKEN: u64 = 1;

pub(super) fn verify_section(
    section: &UnwindSection,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
) -> Vec<Diagnostic> {
    let total_functions = imports.functions.saturating_add(functions.len() as u64);
    section
        .functions
        .iter()
        .filter(|record| {
            record.function_index < imports.functions || record.function_index >= total_functions
        })
        .map(|record| {
            Diagnostic::error(
                VFY16,
                format!(
                    "UNWIND record names invalid defined function index {}",
                    record.function_index
                ),
            )
        })
        .collect()
}

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    function: &FunctionRecordSummary,
    record: Option<&UnwindFunctionRecord>,
    types: &TypeSection,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut landingpads = vec![false; body.blocks.len()];
    for (block_index, block) in body.blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            if instruction.opcode != 0x1d0 {
                if function.target != 0 && instruction.opcode == 0x1aa {
                    diagnostics.push(Diagnostic::error(
                        VFY16,
                        "GPU code cannot invoke an unwinding call",
                    ));
                }
                continue;
            }
            if instruction_index != 0 {
                diagnostics.push(Diagnostic::error(
                    VFY16,
                    "landingpad must be the first instruction in its block",
                ));
            } else {
                landingpads[block_index] = true;
                if block_index == 0 {
                    diagnostics.push(Diagnostic::error(
                        VFY16,
                        "function entry block cannot be a landingpad",
                    ));
                }
            }
            if function.target != 0 {
                diagnostics.push(Diagnostic::error(
                    VFY16,
                    "GPU code cannot contain landingpads",
                ));
            }
            if record.is_none() {
                diagnostics.push(Diagnostic::error(
                    VFY16,
                    "function with a landingpad has no UNWIND record",
                ));
            }
            diagnostics.extend(verify_clauses(
                instruction,
                record,
                types,
                functions,
                imports,
            ));
        }
    }

    if let Some(record) = record {
        if record.terminate_kind == 2 && function.attributes & NOUNWIND == 0 {
            diagnostics.push(Diagnostic::error(
                VFY16,
                format!(
                    "function @{} has nounwind terminate kind without the nounwind attribute",
                    function.name
                ),
            ));
        }
        if record.terminate_kind == 1 {
            let signature = types.signatures.get(function.signature_index as usize);
            let boundary = function.linkage == 2
                || (function.attributes & ADDRESS_TAKEN != 0
                    && signature.is_some_and(|signature| signature.convention == 1));
            if !boundary {
                diagnostics.push(Diagnostic::error(
                    VFY16,
                    format!(
                        "function @{} has boundary terminate kind without an exported or C callback boundary",
                        function.name
                    ),
                ));
            }
        }
    }

    let mut incoming_unwind = vec![0usize; body.blocks.len()];
    let mut incoming_normal = vec![0usize; body.blocks.len()];
    for block in &body.blocks {
        let Some(terminator) = block.instructions.last() else {
            continue;
        };
        if terminator.opcode == 0x1aa {
            let targets = terminator
                .operands
                .iter()
                .filter_map(|operand| match operand {
                    BinaryOperand::BlockTarget { block, .. } => usize::try_from(*block).ok(),
                    _ => None,
                });
            let mut targets = targets;
            if let Some(normal) = targets.next()
                && normal < incoming_normal.len()
            {
                incoming_normal[normal] = incoming_normal[normal].saturating_add(1);
            }
            if let Some(unwind) = targets.next() {
                if unwind >= landingpads.len() || !landingpads[unwind] {
                    diagnostics.push(Diagnostic::error(
                        VFY16,
                        format!("invoke unwind target block {unwind} must begin with landingpad"),
                    ));
                } else {
                    incoming_unwind[unwind] = incoming_unwind[unwind].saturating_add(1);
                }
            }
        } else {
            super::visit_edges(terminator, |target, _, _| {
                if let Ok(target) = usize::try_from(target)
                    && let Some(incoming) = incoming_normal.get_mut(target)
                {
                    *incoming = incoming.saturating_add(1);
                }
            });
        }
    }

    for (index, block) in body.blocks.iter().enumerate() {
        if landingpads[index] && incoming_normal[index] != 0 {
            diagnostics.push(Diagnostic::error(
                VFY16,
                format!("landingpad block {index} is reachable through a normal edge"),
            ));
        }
        if landingpads[index] && incoming_unwind[index] == 0 && incoming_normal[index] == 0 {
            diagnostics.push(Diagnostic::error(
                VFY16,
                format!("landingpad block {index} has no unwind predecessor"),
            ));
        }
        if block
            .instructions
            .iter()
            .any(|instruction| instruction.opcode == 0x1d0)
            && !landingpads[index]
        {
            diagnostics.push(Diagnostic::error(
                VFY16,
                format!("landingpad block {index} does not begin with landingpad"),
            ));
        }
    }
    diagnostics
}

fn verify_clauses(
    instruction: &BinaryInstruction,
    record: Option<&UnwindFunctionRecord>,
    types: &TypeSection,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let Some(BinaryOperand::Clauses(clauses)) = instruction
        .operands
        .iter()
        .find(|operand| matches!(operand, BinaryOperand::Clauses(_)))
    else {
        return diagnostics;
    };
    let mut saw_cleanup = false;
    for (kind, index) in clauses {
        match kind {
            0 if record.is_none_or(|record| *index >= record.type_ids.len() as u64) => {
                diagnostics.push(Diagnostic::error(
                    VFY16,
                    format!("landingpad catch.type index {index} is outside its UNWIND table"),
                ));
            }
            1 => match record
                .and_then(|record| record.filters.get(*index as usize))
                .copied()
            {
                Some(function_index) => {
                    if !valid_filter(function_index, types, functions, imports) {
                        diagnostics.push(Diagnostic::error(
                            VFY16,
                            format!(
                                "landingpad catch.filter function {function_index} has an invalid signature or may unwind"
                            ),
                        ));
                    }
                }
                None => diagnostics.push(Diagnostic::error(
                    VFY16,
                    format!("landingpad catch.filter index {index} is outside its UNWIND table"),
                )),
            },
            2 => {}
            3 if std::mem::replace(&mut saw_cleanup, true) => diagnostics.push(Diagnostic::error(
                VFY16,
                "landingpad has more than one cleanup clause",
            )),
            3 => {}
            _ => {}
        }
    }
    diagnostics
}

fn valid_filter(
    function_index: u64,
    types: &TypeSection,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
) -> bool {
    let signature_index = if function_index < imports.functions {
        imports
            .function_signatures
            .get(function_index as usize)
            .copied()
    } else {
        functions
            .get(function_index.saturating_sub(imports.functions) as usize)
            .map(|function| function.signature_index)
    };
    let Some(signature) = signature_index.and_then(|index| types.signatures.get(index as usize))
    else {
        return false;
    };
    signature_is_filter(signature, types)
        && (function_index < imports.functions
            || functions
                .get(function_index.saturating_sub(imports.functions) as usize)
                .is_some_and(|function| {
                    function.target == 0 && function.attributes & NOUNWIND != 0
                }))
}

fn signature_is_filter(signature: &SignatureSummary, types: &TypeSection) -> bool {
    if signature.flags & 7 != 0 || signature.parameters.len() != 2 || signature.results.len() != 1 {
        return false;
    }
    let [
        tvisa_format::ParameterItemSummary::Leaf {
            type_index: box_type,
        },
        tvisa_format::ParameterItemSummary::Leaf {
            type_index: typeid_type,
        },
    ] = signature.parameters.as_slice()
    else {
        return false;
    };
    let [
        tvisa_format::ResultItemSummary::Leaf {
            type_index: result_type,
        },
    ] = signature.results.as_slice()
    else {
        return false;
    };
    matches!(
        types.value_types.get(*box_type as usize),
        Some(TvisaValueTypeRecord::Pointer { address_space: 0 })
    ) && matches!(
        types.value_types.get(*typeid_type as usize),
        Some(TvisaValueTypeRecord::Integer { bits: 64 })
    ) && matches!(
        types.value_types.get(*result_type as usize),
        Some(TvisaValueTypeRecord::Integer { bits: 1 })
    )
}
