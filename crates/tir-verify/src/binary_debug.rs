use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryCodeTable, BinaryDebugTable, BinaryGlobalTable, BinarySignatureTable};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(
    debug: &BinaryDebugTable,
    signatures: Option<&BinarySignatureTable>,
    code: Option<&BinaryCodeTable>,
    globals: Option<&BinaryGlobalTable>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if let Some(signatures) = signatures {
        for scope in &debug.scopes {
            if scope.function_index >= signatures.records.len() as u64 {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "DEBUG scope refers to out-of-range function index {}",
                        scope.function_index
                    ),
                ));
            }
        }
        for location in &debug.locations {
            if location.function_index >= signatures.records.len() as u64 {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "DEBUG location refers to out-of-range function index {}",
                        location.function_index
                    ),
                ));
            }
        }
    }

    if let Some(globals) = globals {
        for record in &debug.data {
            if record.data_index >= globals.records.len() as u64 {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "DEBUG DATA record refers to out-of-range GLOBALS index {}",
                        record.data_index
                    ),
                ));
            }
        }
    }

    let Some(code) = code else {
        return diagnostics;
    };
    for location in &debug.locations {
        if !code
            .functions
            .iter()
            .any(|body| body.function_index == location.function_index)
        {
            diagnostics.push(Diagnostic::error(
                V2,
                format!(
                    "DEBUG location refers to function index {} without a CODE body",
                    location.function_index
                ),
            ));
        }
    }
    for body in &code.functions {
        let function_index = body.function_index;
        let instruction_count = body.blocks.iter().try_fold(0u64, |count, block| {
            count.checked_add(block.instruction_count)
        });
        if let Some(instruction_count) = instruction_count {
            for location in debug
                .locations
                .iter()
                .filter(|location| location.function_index == function_index)
            {
                if location.instruction_ordinal >= instruction_count {
                    diagnostics.push(Diagnostic::error(
                        V2,
                        format!(
                            "DEBUG location ordinal {} is outside function index {}",
                            location.instruction_ordinal, function_index
                        ),
                    ));
                }
            }
        } else {
            diagnostics.push(Diagnostic::error(
                V2,
                format!("CODE instruction count overflows for function index {function_index}"),
            ));
        }

        for block in &body.blocks {
            let Some(instructions) = &block.instructions else {
                continue;
            };
            for instruction in instructions {
                let record_index = instruction
                    .operands
                    .iter()
                    .find_map(|operand| match operand {
                        tir_format::BinaryOperand::Index(index) => Some(*index),
                        _ => None,
                    });
                match instruction.opcode {
                    0x240 | 0x241 => {
                        let variable = record_index
                            .and_then(|index| usize::try_from(index).ok())
                            .and_then(|index| debug.variables.get(index));
                        let Some(variable) = variable else {
                            diagnostics.push(Diagnostic::error(
                                V2,
                                format!(
                                    "debug.value or debug.addr references an invalid DEBUG variable in function index {function_index}"
                                ),
                            ));
                            continue;
                        };
                        if debug
                            .scopes
                            .get(variable.scope as usize)
                            .is_none_or(|scope| scope.function_index != function_index)
                        {
                            diagnostics.push(Diagnostic::error(
                                V2,
                                format!(
                                    "DEBUG variable used by function index {function_index} belongs to another function"
                                ),
                            ));
                        }
                    }
                    0x242 => {
                        let label = record_index
                            .and_then(|index| usize::try_from(index).ok())
                            .and_then(|index| debug.labels.get(index));
                        let Some(label) = label else {
                            diagnostics.push(Diagnostic::error(
                                V2,
                                format!(
                                    "debug.label references an invalid DEBUG label in function index {function_index}"
                                ),
                            ));
                            continue;
                        };
                        if debug
                            .scopes
                            .get(label.scope as usize)
                            .is_none_or(|scope| scope.function_index != function_index)
                        {
                            diagnostics.push(Diagnostic::error(
                                V2,
                                format!(
                                    "DEBUG label used by function index {function_index} belongs to another function"
                                ),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    diagnostics
}
