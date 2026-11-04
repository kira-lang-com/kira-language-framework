use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_types::{AddressSpace, FloatType, Type};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, FunctionRecordSummary, ImportCounts,
    ParameterItemSummary, ResultItemSummary, SignatureSummary, TvisaValueTypeRecord, TypeSection,
    VariantIndex,
};

const VFY5: Code = Code::new(Dialect::Tvisa, Area::Verifier, 5);
const VFY30: Code = Code::new(Dialect::Tvisa, Area::Verifier, 30);
const VFY42: Code = Code::new(Dialect::Tvisa, Area::Verifier, 42);

pub(super) fn infer_result_types(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    variants: &VariantIndex,
) -> Option<Vec<Option<u64>>> {
    if (0x500..=0x511).contains(&instruction.opcode) {
        return super::gpu_collectives::ray::infer_result_types(instruction, types);
    }
    if (0x480..=0x48d).contains(&instruction.opcode) {
        return super::gpu_collectives::texture::infer_result_types(
            instruction,
            value_types,
            types,
        );
    }
    if (0x560..=0x56b).contains(&instruction.opcode) {
        return super::gpu_collectives::advanced::infer_result_types(
            instruction,
            value_types,
            types,
        );
    }
    if instruction.opcode == 0x706
        && let Some(id) = runtime_callee_id(instruction)
    {
        let (_, results) = crate::runtime::runtime_call_signature_by_id(id)?;
        return Some(
            results
                .iter()
                .map(|result| binary_type_index(types, result))
                .collect(),
        );
    }
    if !matches!(instruction.opcode, 0x1a0 | 0x1a1 | 0x1a9 | 0x404) {
        return None;
    }
    let signature_index = signature_index(instruction, functions, imports, variants)?;
    let signature = types.signatures.get(signature_index as usize)?;
    Some(signature_result_types(signature, types))
}

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    caller_signature_index: u64,
    value_types: &[Option<u64>],
    types: &TypeSection,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    variants: &VariantIndex,
) -> Vec<Diagnostic> {
    let Some(caller) = types.signatures.get(caller_signature_index as usize) else {
        return Vec::new();
    };
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if let Some(id) = runtime_callee_id(instruction) {
            verify_runtime_call(
                instruction,
                id,
                caller,
                value_types,
                types,
                body,
                &mut diagnostics,
            );
            continue;
        }
        if !matches!(
            instruction.opcode,
            0x1a0 | 0x1a1 | 0x1a9 | 0x1aa | 0x1e7 | 0x404
        ) {
            continue;
        }
        let Some(signature_index) = signature_index(instruction, functions, imports, variants)
        else {
            continue;
        };
        let Some(callee) = types.signatures.get(signature_index as usize) else {
            continue;
        };
        verify_call_arguments(instruction, callee, value_types, types, &mut diagnostics);
        let results = signature_result_types(callee, types);
        match instruction.opcode {
            0x1a0 | 0x1a1 | 0x1a9 if usize::from(instruction.result_count) != results.len() => {
                diagnostics.push(Diagnostic::error(
                    VFY5,
                    format!(
                        "call defines {} results; signature yields {} leaves",
                        instruction.result_count,
                        results.len()
                    ),
                ));
            }
            0x1aa => verify_invoke_results(instruction, &results, body, types, &mut diagnostics),
            0x1e7 => verify_tail_call(caller, callee, &mut diagnostics),
            _ => {}
        }
    }
    diagnostics
}

fn verify_call_arguments(
    instruction: &BinaryInstruction,
    signature: &SignatureSummary,
    value_types: &[Option<u64>],
    types: &TypeSection,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let expected = signature_parameter_types(signature);
    let Some(arguments) = call_arguments(instruction) else {
        diagnostics.push(Diagnostic::error(VFY5, "call is missing its argument list"));
        return;
    };
    let variadic = signature.flags & 2 != 0;
    if variadic && instruction.opcode != 0x1a9 {
        diagnostics.push(Diagnostic::error(
            VFY5,
            "calls to variadic signatures must use call.variadic",
        ));
    }
    if instruction.opcode == 0x1a9 {
        let fixed = instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Index(index) => Some(*index),
                _ => None,
            });
        if signature.convention != 1 || !variadic {
            diagnostics.push(Diagnostic::error(
                VFY5,
                "call.variadic requires a variadic c signature",
            ));
        }
        if fixed != signature.fixed_parameter_count {
            diagnostics.push(Diagnostic::error(
                VFY5,
                format!(
                    "call.variadic fixed count {fixed:?} does not match signature count {:?}",
                    signature.fixed_parameter_count
                ),
            ));
        }
    }
    let count_matches = if variadic {
        arguments.len() >= expected.len()
    } else {
        arguments.len() == expected.len()
    };
    if !count_matches {
        diagnostics.push(Diagnostic::error(
            VFY5,
            format!(
                "call has {} arguments; signature requires {} fixed leaves",
                arguments.len(),
                expected.len()
            ),
        ));
    }
    for (index, (argument, expected)) in arguments.iter().zip(&expected).enumerate() {
        let Some(actual) = usize::try_from(*argument)
            .ok()
            .and_then(|index| value_types.get(index))
            .copied()
            .flatten()
        else {
            continue;
        };
        let matches = expected.map_or_else(
            || {
                matches!(
                    types.value_types.get(actual as usize),
                    Some(TvisaValueTypeRecord::Pointer { address_space: 0 })
                )
            },
            |expected| actual == expected,
        );
        if !matches {
            diagnostics.push(Diagnostic::error(
                VFY5,
                format!(
                    "call argument {index} has type {actual}; expected {}",
                    expected.map_or_else(|| "pointer".to_owned(), |ty| ty.to_string())
                ),
            ));
        }
    }
}

fn verify_runtime_call(
    instruction: &BinaryInstruction,
    id: u16,
    caller: &SignatureSummary,
    value_types: &[Option<u64>],
    types: &TypeSection,
    body: &BinaryFunctionBody,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some((expected_arguments, expected_results)) =
        crate::runtime::runtime_call_signature_by_id(id)
    else {
        return;
    };
    let Some(arguments) = call_arguments(instruction) else {
        diagnostics.push(Diagnostic::error(
            VFY42,
            format!("runtime entry 0x{id:04X} is missing its argument list"),
        ));
        return;
    };
    if arguments.len() != expected_arguments.len() {
        diagnostics.push(Diagnostic::error(
            VFY42,
            format!(
                "runtime entry 0x{id:04X} has {} arguments; expected {}",
                arguments.len(),
                expected_arguments.len()
            ),
        ));
    }
    for (index, (argument, expected)) in arguments.iter().zip(&expected_arguments).enumerate() {
        let actual = usize::try_from(*argument)
            .ok()
            .and_then(|index| value_types.get(index))
            .copied()
            .flatten();
        if actual.is_some_and(|actual| !binary_type_matches(types, actual, expected)) {
            diagnostics.push(Diagnostic::error(
                VFY42,
                format!("runtime entry 0x{id:04X} argument {index} has the wrong type"),
            ));
        }
    }
    match instruction.opcode {
        0x706 if usize::from(instruction.result_count) != expected_results.len() => {
            diagnostics.push(Diagnostic::error(
                VFY42,
                format!(
                    "runtime entry 0x{id:04X} defines {} results; expected {}",
                    instruction.result_count,
                    expected_results.len()
                ),
            ));
        }
        0x1aa => verify_runtime_invoke_results(
            instruction,
            id,
            &expected_results,
            body,
            types,
            diagnostics,
        ),
        0x1e7 => verify_runtime_tail_results(caller, id, &expected_results, types, diagnostics),
        _ => {}
    }
}

fn verify_runtime_tail_results(
    caller: &SignatureSummary,
    id: u16,
    expected: &[Type],
    types: &TypeSection,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let actual = signature_result_types(caller, types);
    let compatible = caller.flags & 1 == 0
        && actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            actual.is_some_and(|actual| binary_type_matches(types, actual, expected))
        });
    if !compatible {
        diagnostics.push(Diagnostic::error(
            VFY42,
            format!("tail.call to runtime entry 0x{id:04X} does not match the caller's results"),
        ));
    }
}

fn verify_runtime_invoke_results(
    instruction: &BinaryInstruction,
    id: u16,
    expected: &[Type],
    body: &BinaryFunctionBody,
    types: &TypeSection,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if usize::from(instruction.normal_edge_result_count) != expected.len() {
        diagnostics.push(Diagnostic::error(
            VFY42,
            format!("invoke runtime entry 0x{id:04X} has the wrong normal-edge result count"),
        ));
    }
    let Some(target_index) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::BlockTarget { block, .. } => usize::try_from(*block).ok(),
            _ => None,
        })
    else {
        return;
    };
    let Some(target) = body.blocks.get(target_index) else {
        return;
    };
    for (index, (actual, expected)) in target
        .parameter_types
        .iter()
        .take(expected.len())
        .zip(expected)
        .enumerate()
    {
        if !binary_type_matches(types, *actual, expected) {
            diagnostics.push(Diagnostic::error(
                VFY42,
                format!("invoke runtime entry 0x{id:04X} result {index} has the wrong type"),
            ));
        }
    }
}

fn runtime_callee_id(instruction: &BinaryInstruction) -> Option<u16> {
    let runtime = instruction.opcode == 0x706
        || (matches!(instruction.opcode, 0x1aa | 0x1e7)
            && (instruction.modifiers >> 48) & 0xf == 2);
    if !runtime {
        return None;
    }
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => u16::try_from(*index).ok(),
            _ => None,
        })
}

fn binary_type_index(types: &TypeSection, expected: &Type) -> Option<u64> {
    types
        .value_types
        .iter()
        .position(|actual| type_record_matches(expected, actual))
        .map(|index| index as u64)
}

fn binary_type_matches(types: &TypeSection, actual: u64, expected: &Type) -> bool {
    types
        .value_types
        .get(actual as usize)
        .is_some_and(|actual| type_record_matches(expected, actual))
}

fn type_record_matches(expected: &Type, actual: &TvisaValueTypeRecord) -> bool {
    match (expected, actual) {
        (Type::Integer(bits), TvisaValueTypeRecord::Integer { bits: actual }) => {
            u64::from(*bits) == *actual
        }
        (Type::Float(float), TvisaValueTypeRecord::Float { tag }) => *tag == float_type_tag(*float),
        (
            Type::Pointer(AddressSpace::Generic),
            TvisaValueTypeRecord::Pointer { address_space: 0 },
        )
        | (Type::Code, TvisaValueTypeRecord::Code) => true,
        _ => false,
    }
}

fn float_type_tag(float: FloatType) -> u8 {
    match float {
        FloatType::F16 => 0x02,
        FloatType::Bf16 => 0x03,
        FloatType::F32 => 0x04,
        FloatType::F64 => 0x05,
        FloatType::F128 => 0x06,
        FloatType::F8E4M3 => 0x07,
        FloatType::F8E5M2 => 0x08,
        FloatType::F6E2M3 => 0x09,
        FloatType::F6E3M2 => 0x0a,
        FloatType::F4E2M1 => 0x0b,
        FloatType::E8M0 => 0x0c,
    }
}

fn verify_invoke_results(
    instruction: &BinaryInstruction,
    results: &[Option<u64>],
    body: &BinaryFunctionBody,
    types: &TypeSection,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if usize::from(instruction.normal_edge_result_count) != results.len() {
        diagnostics.push(Diagnostic::error(
            VFY5,
            "invoke normal edge result count disagrees with its signature",
        ));
    }
    let Some((block, _)) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::BlockTarget { block, arguments } => Some((*block, arguments)),
            _ => None,
        })
    else {
        return;
    };
    let Some(target) = usize::try_from(block)
        .ok()
        .and_then(|index| body.blocks.get(index))
    else {
        return;
    };
    for (index, expected) in results.iter().enumerate() {
        let Some(actual) = target.parameter_types.get(index) else {
            break;
        };
        let matches = expected.map_or_else(
            || {
                matches!(
                    types.value_types.get(*actual as usize),
                    Some(TvisaValueTypeRecord::Pointer { address_space: 0 })
                )
            },
            |expected| *actual == expected,
        );
        if !matches {
            diagnostics.push(Diagnostic::error(
                VFY5,
                format!(
                    "invoke result {index} has type {}, expected {}",
                    actual,
                    expected.map_or_else(|| "pointer".to_owned(), |ty| ty.to_string())
                ),
            ));
        }
    }
}

fn verify_tail_call(
    caller: &SignatureSummary,
    callee: &SignatureSummary,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if callee.convention == 0
        && (caller.convention != 0
            || caller.flags & 1 != callee.flags & 1
            || caller.results != callee.results)
    {
        diagnostics.push(Diagnostic::error(
            VFY30,
            "tail.call to a ridl function requires a ridl caller with identical results and failing status",
        ));
    }
}

fn signature_parameter_types(signature: &SignatureSummary) -> Vec<Option<u64>> {
    let mut types = Vec::new();
    for parameter in &signature.parameters {
        match parameter {
            ParameterItemSummary::Leaf { type_index } => types.push(Some(*type_index)),
            ParameterItemSummary::Group(group) => {
                types.extend(group.members.iter().map(|member| Some(member.type_index)));
            }
            ParameterItemSummary::ByRef { .. }
            | ParameterItemSummary::SRet { .. }
            | ParameterItemSummary::Context
            | ParameterItemSummary::CAggregate { .. } => types.push(None),
        }
    }
    types
}

fn signature_result_types(signature: &SignatureSummary, types: &TypeSection) -> Vec<Option<u64>> {
    let mut results = Vec::new();
    for result in &signature.results {
        match result {
            ResultItemSummary::Leaf { type_index } => results.push(Some(*type_index)),
            ResultItemSummary::Group(group) => {
                results.extend(group.members.iter().map(|member| Some(member.type_index)));
            }
        }
    }
    if signature.flags & 1 != 0 {
        results.push(
            types
                .value_types
                .iter()
                .position(|ty| matches!(ty, TvisaValueTypeRecord::Pointer { address_space: 0 }))
                .map(|index| index as u64),
        );
    }
    results
}

fn call_arguments(instruction: &BinaryInstruction) -> Option<Vec<u64>> {
    let end = instruction
        .operands
        .iter()
        .position(|operand| {
            matches!(
                operand,
                BinaryOperand::BlockTarget { .. }
                    | BinaryOperand::BlockTargets(_)
                    | BinaryOperand::SwitchCases(_)
                    | BinaryOperand::Blocks(_)
            )
        })
        .unwrap_or(instruction.operands.len());
    instruction.operands[..end]
        .iter()
        .rev()
        .find_map(|operand| match operand {
            BinaryOperand::Value(value) => Some(vec![*value]),
            BinaryOperand::Values(values) => Some(values.clone()),
            _ => None,
        })
}

fn signature_index(
    instruction: &BinaryInstruction,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    variants: &VariantIndex,
) -> Option<u64> {
    match instruction.opcode {
        0x1a0 => instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Symbol(symbol) => {
                    symbol_signature(*symbol, functions, imports, variants)
                }
                _ => None,
            }),
        0x404 => instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Index(index) => Some(*index),
                _ => None,
            }),
        0x1a1 | 0x1a9 => instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Index(index) => Some(*index),
                _ => None,
            }),
        0x1aa | 0x1e7 => match (instruction.modifiers >> 48) & 0xf {
            0 => instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Symbol(symbol) => {
                        symbol_signature(*symbol, functions, imports, variants)
                    }
                    _ => None,
                }),
            1 => instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Index(index) => Some(*index),
                    _ => None,
                }),
            _ => None,
        },
        _ => None,
    }
}

fn symbol_signature(
    symbol: u64,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    variants: &VariantIndex,
) -> Option<u64> {
    let index = symbol >> 2;
    match symbol & 3 {
        0 => function_space_signature(index, functions, imports),
        2 => variants
            .sets
            .get(usize::try_from(index).ok()?)?
            .members
            .first()
            .and_then(|member| function_space_signature(member.function_index, functions, imports)),
        _ => None,
    }
}

fn function_space_signature(
    function_index: u64,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
) -> Option<u64> {
    if function_index < imports.functions {
        imports
            .function_signatures
            .get(usize::try_from(function_index).ok()?)
            .copied()
    } else {
        functions
            .get(usize::try_from(function_index - imports.functions).ok()?)
            .map(|function| function.signature_index)
    }
}
