use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryAnalysisSummary, BinaryCodeTable, BinaryGenericTable, BinaryGlobalTable, BinaryImport,
    BinaryOperand, BinaryProducer, BinarySignature, BinarySignatureTable, BinaryTarget, BinaryType,
    BinaryTypeTable, StringTable,
};

const V20: Code = Code::new(Dialect::Tir, Area::Verifier, 20);

/// Reconciles the ANALYSIS assembly inventory with the assembly instructions in CODE.
pub(super) fn verify_assembly_inventory(
    code: &BinaryCodeTable,
    signatures: &[BinarySignature],
    analysis: &BinaryAnalysisSummary,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut regions = analysis
        .assembly_regions
        .iter()
        .map(|region| ((region.function_index, region.instruction_ordinal), *region))
        .collect::<HashMap<_, _>>();

    for function in &code.functions {
        let Some(signature) = usize::try_from(function.function_index)
            .ok()
            .and_then(|index| signatures.get(index))
        else {
            continue;
        };

        let mut ordinal = 0u64;
        let mut fully_decoded = true;
        for block in &function.blocks {
            let Some(instructions) = &block.instructions else {
                fully_decoded = false;
                ordinal = ordinal.saturating_add(block.instruction_count);
                continue;
            };

            for (block_ordinal, instruction) in instructions.iter().enumerate() {
                let instruction_ordinal = ordinal.saturating_add(block_ordinal as u64);
                let key = (function.function_index, instruction_ordinal);
                let region = regions.remove(&key);
                if matches!(instruction.opcode, 0x220 | 0x221) {
                    let Some(assembly_index) =
                        instruction
                            .operands
                            .first()
                            .and_then(|operand| match operand {
                                BinaryOperand::Index(index) => Some(*index),
                                _ => None,
                            })
                    else {
                        diagnostics.push(assembly_error(
                            &signature.name,
                            instruction_ordinal,
                            "assembly instruction has no ASM record index",
                        ));
                        continue;
                    };
                    let Some(region) = region else {
                        diagnostics.push(assembly_error(
                            &signature.name,
                            instruction_ordinal,
                            "assembly instruction is missing from ANALYSIS",
                        ));
                        continue;
                    };
                    let Some(variant) = &signature.variant else {
                        diagnostics.push(assembly_error(
                            &signature.name,
                            instruction_ordinal,
                            "assembly instruction has no variant architecture",
                        ));
                        continue;
                    };
                    let expected_effects = if instruction.opcode == 0x220 {
                        ((instruction.modifiers >> 37) & 0x0f) as u8
                    } else {
                        0
                    };
                    if region.assembly_record_index != assembly_index {
                        diagnostics.push(assembly_error(
                            &signature.name,
                            instruction_ordinal,
                            "ANALYSIS names a different ASM record",
                        ));
                    }
                    if region.architecture != variant.architecture {
                        diagnostics.push(assembly_error(
                            &signature.name,
                            instruction_ordinal,
                            "ANALYSIS architecture does not match the function variant",
                        ));
                    }
                    if region.effects != expected_effects {
                        diagnostics.push(assembly_error(
                            &signature.name,
                            instruction_ordinal,
                            "ANALYSIS effects do not match the assembly modifiers",
                        ));
                    }
                } else if region.is_some() {
                    diagnostics.push(assembly_error(
                        &signature.name,
                        instruction_ordinal,
                        "ANALYSIS assembly region points to a non-assembly instruction",
                    ));
                }
            }
            ordinal = ordinal.saturating_add(block.instruction_count);
        }

        let invalid_ordinals = regions
            .keys()
            .filter(|(function_index, instruction_ordinal)| {
                *function_index == function.function_index && *instruction_ordinal >= ordinal
            })
            .copied()
            .collect::<Vec<_>>();
        for (_, instruction_ordinal) in invalid_ordinals {
            regions.remove(&(function.function_index, instruction_ordinal));
            diagnostics.push(assembly_error(
                &signature.name,
                instruction_ordinal,
                "ANALYSIS assembly region is outside the function body",
            ));
        }

        if fully_decoded {
            let unclaimed = regions
                .keys()
                .filter(|(function_index, _)| *function_index == function.function_index)
                .copied()
                .collect::<Vec<_>>();
            for (_, instruction_ordinal) in unclaimed {
                regions.remove(&(function.function_index, instruction_ordinal));
                diagnostics.push(assembly_error(
                    &signature.name,
                    instruction_ordinal,
                    "ANALYSIS assembly region does not identify an assembly instruction",
                ));
            }
        }
    }

    for ((function_index, instruction_ordinal), _) in regions {
        diagnostics.push(Diagnostic::error(
            V20,
            format!(
                "ANALYSIS assembly region references missing function {function_index} instruction {instruction_ordinal}"
            ),
        ));
    }
    diagnostics
}

/// Checks the reason bits that can be derived directly from decoded CODE.
pub(super) fn verify_known_reason_bits(
    code: &BinaryCodeTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    analysis: &BinaryAnalysisSummary,
    producer: Option<&BinaryProducer>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let converter_without_ownership = producer_is_converter_without_ownership(producer);
    let producer_supplies_ownership = producer_supplies_ownership(producer);
    let wasm64_only = producer
        .is_some_and(|producer| producer.languages.len() == 1 && producer.languages[0] == "wasm64");
    for function in &code.functions {
        let Ok(index) = usize::try_from(function.function_index) else {
            continue;
        };
        let (Some(signature), Some(record)) =
            (signatures.records.get(index), analysis.functions.get(index))
        else {
            continue;
        };
        if record.trust_class == 1
            && !producer.is_some_and(|producer| {
                producer
                    .languages
                    .iter()
                    .any(|language| language == "wasm64")
            })
        {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "ANALYSIS marks @{} sandboxed without Wasm64 converter provenance",
                    signature.name
                ),
            ));
        }
        if wasm64_only && record.trust_class != 1 {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "ANALYSIS does not mark Wasm64 function @{} sandboxed",
                    signature.name
                ),
            ));
        }
        let all_instructions_decoded = function
            .blocks
            .iter()
            .all(|block| block.instructions.is_some());
        let value_types = if all_instructions_decoded {
            crate::binary_code::infer_value_types(function, signatures, types)
        } else {
            Vec::new()
        };
        let gpu_only = signature.target == BinaryTarget::Gpu;
        let mut has_sandbox_memory_access = false;
        let mut required_reasons = if converter_without_ownership {
            1 << 6
        } else {
            0
        };
        let mut unknown_reason_bits = 0u8;
        let mut has_unmanaged_instruction = false;
        for block in &function.blocks {
            let Some(instructions) = &block.instructions else {
                continue;
            };
            for instruction in instructions {
                match instruction.opcode {
                    0x220 | 0x221 => {
                        required_reasons |= 1 << 0;
                        has_unmanaged_instruction = true;
                    }
                    0x0aa => {
                        required_reasons |= 1 << 2;
                        has_unmanaged_instruction = true;
                    }
                    0x1a8 | 0x1a9 | 0x210..=0x213 => {
                        required_reasons |= 1 << 3;
                        has_unmanaged_instruction = true;
                    }
                    0x1a1 if is_foreign_indirect_call(instruction, types) => {
                        required_reasons |= 1 << 3;
                        has_unmanaged_instruction = true;
                    }
                    0x214 | 0x215 => {
                        required_reasons |= 1 << 4;
                        has_unmanaged_instruction = true;
                    }
                    0x123 if instruction.modifiers & (1 << 33) != 0 => {
                        required_reasons |= 1 << 5;
                        has_unmanaged_instruction = true;
                    }
                    0x187 if ((instruction.modifiers >> 48) & 0xf) >= 4 => {
                        required_reasons |= 1 << 5;
                        has_unmanaged_instruction = true;
                    }
                    0x270..=0x273 => {
                        required_reasons |= 1 << 7;
                        has_unmanaged_instruction = true;
                    }
                    0x164..=0x166 | 0x185 | 0x186 => {
                        required_reasons |= 1 << 1;
                        has_unmanaged_instruction = true;
                    }
                    0x127 | 0x128 if !gpu_only => {
                        required_reasons |= 1 << 1;
                        has_unmanaged_instruction = true;
                    }
                    0x146..=0x147 if !gpu_only => {
                        required_reasons |= 1 << 1;
                        has_unmanaged_instruction = true;
                        has_sandbox_memory_access = true;
                    }
                    0x148..=0x149 if !gpu_only => {
                        required_reasons |= 1 << 1;
                        has_unmanaged_instruction = true;
                    }
                    0x14a..=0x14c if !gpu_only => {
                        let pointer_count = if instruction.opcode == 0x14c { 1 } else { 2 };
                        let pointer_operands = (0..pointer_count)
                            .map(|operand| {
                                is_pointer_operand(instruction, operand, &value_types, types)
                            })
                            .collect::<Vec<_>>();
                        if pointer_operands
                            .iter()
                            .any(|is_pointer| *is_pointer == Some(true))
                        {
                            required_reasons |= 1 << 1;
                            has_unmanaged_instruction = true;
                            has_sandbox_memory_access = true;
                        } else if pointer_operands.iter().any(Option::is_none) {
                            unknown_reason_bits |= 1 << 1;
                        }
                    }
                    0x222 => has_unmanaged_instruction = true,
                    _ => {}
                }
            }
        }
        if record.trust_class == 1
            && has_sandbox_memory_access
            && analysis.sandbox_regions.is_empty()
        {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "sandboxed @{} performs raw memory access without a declared sandbox region",
                    signature.name
                ),
            ));
        }
        let missing = required_reasons & !record.reason_mask;
        if missing != 0 {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "ANALYSIS reason mask for @{} omits CODE-derived reason bits 0x{missing:02x}",
                    signature.name
                ),
            ));
        }
        if all_instructions_decoded {
            let unknown_reasons = unknown_reason_bits
                | if producer.is_none()
                    || (!converter_without_ownership && !producer_supplies_ownership)
                {
                    1 << 6
                } else {
                    0
                };
            let exactly_recomputed = !unknown_reasons;
            let unexpected = record.reason_mask & exactly_recomputed & !required_reasons;
            if unexpected != 0 {
                diagnostics.push(Diagnostic::error(
                    V20,
                    format!(
                        "ANALYSIS reason mask for @{} has CODE-unsupported bits 0x{unexpected:02x}",
                        signature.name
                    ),
                ));
            }
        }
        if record.trust_class == 0 && (record.reason_mask != 0 || has_unmanaged_instruction) {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "ANALYSIS marks @{} managed despite unmanaged reasons or instructions",
                    signature.name
                ),
            ));
        }
        if record.trust_class == 0 && (!all_instructions_decoded || unknown_reason_bits != 0) {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "ANALYSIS marks @{} managed while CODE has unmanaged behavior the verifier cannot rule out",
                    signature.name
                ),
            ));
        }
        if record.trust_class == 1 && (!all_instructions_decoded || unknown_reason_bits != 0) {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "ANALYSIS marks @{} sandboxed while CODE contains unresolved memory behavior",
                    signature.name
                ),
            ));
        }
        if record.trust_class == 0 && !producer_supplies_ownership {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "ANALYSIS marks @{} managed without KLF ownership provenance",
                    signature.name
                ),
            ));
        }
        if record.trust_class == 2
            && producer_supplies_ownership
            && all_instructions_decoded
            && !has_unmanaged_instruction
            && unknown_reason_bits == 0
            && record.reason_mask == 0
        {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "ANALYSIS marks KLF-owned @{} unmanaged despite no unmanaged instruction or reason",
                    signature.name
                ),
            ));
        }
    }
    diagnostics
}

fn producer_is_converter_without_ownership(producer: Option<&BinaryProducer>) -> bool {
    let Some(producer) = producer else {
        return false;
    };
    !producer.languages.is_empty()
        && producer.languages.iter().all(|language| {
            matches!(
                language.as_str(),
                "cpp" | "rust" | "zig" | "llvm" | "wasm64" | "spirv"
            )
        })
}

fn producer_supplies_ownership(producer: Option<&BinaryProducer>) -> bool {
    producer.is_some_and(|producer| {
        producer.name == "klf"
            && !producer.languages.is_empty()
            && producer
                .languages
                .iter()
                .all(|language| matches!(language.as_str(), "kira" | "kavascript" | "ksl"))
    })
}

/// Checks the declared types of globals used as Wasm sandbox-region bounds.
pub(super) fn verify_sandbox_globals(
    analysis: &BinaryAnalysisSummary,
    globals: &BinaryGlobalTable,
    types: &BinaryTypeTable,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for region in &analysis.sandbox_regions {
        let Some(base) = usize::try_from(region.base_global_index)
            .ok()
            .and_then(|index| globals.records.get(index))
        else {
            continue;
        };
        let Some(size) = usize::try_from(region.size_global_index)
            .ok()
            .and_then(|index| globals.records.get(index))
        else {
            continue;
        };
        let base_is_pointer = usize::try_from(base.ty)
            .ok()
            .and_then(|index| types.entries.get(index))
            .is_some_and(|ty| matches!(ty, BinaryType::Pointer(0)));
        let size_is_i64 = usize::try_from(size.ty)
            .ok()
            .and_then(|index| types.entries.get(index))
            .is_some_and(|ty| matches!(ty, BinaryType::Integer(64)));
        if !base_is_pointer {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "ANALYSIS sandbox region {} base global @{} is not `ptr`",
                    region.memory_index, base.name
                ),
            ));
        }
        if !size_is_i64 {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "ANALYSIS sandbox region {} size global @{} is not `i64`",
                    region.memory_index, size.name
                ),
            ));
        }
    }
    diagnostics
}

/// Checks indirect-call IDs that can be derived from the encoded call types.
pub(super) fn verify_indirect_call_typeids(
    code: &BinaryCodeTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    analysis: &BinaryAnalysisSummary,
    module_path: &str,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut available_ids = HashMap::new();
    for (index, ty) in types.entries.iter().enumerate() {
        if !matches!(ty, BinaryType::Function(_)) {
            continue;
        }
        let index = index as u64;
        if let Ok(type_id) = crate::binary_constants::canonical_type_id(types, index, module_path) {
            available_ids.insert(index, type_id);
        }
    }

    let recorded_ids = analysis
        .indirect_call_typeids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let mut required_ids = std::collections::HashSet::new();
    let mut all_call_types_known = true;
    for function in &code.functions {
        let all_instructions_decoded = function
            .blocks
            .iter()
            .all(|block| block.instructions.is_some());
        if !all_instructions_decoded {
            all_call_types_known = false;
        }
        let value_types = if all_instructions_decoded {
            Some(crate::binary_code::infer_value_types(
                function, signatures, types,
            ))
        } else {
            None
        };
        for block in &function.blocks {
            let Some(instructions) = &block.instructions else {
                continue;
            };
            for instruction in instructions {
                let function_type = match instruction.opcode {
                    0x1a1 | 0x404 => instruction.operands.iter().find_map(|operand| {
                        if let BinaryOperand::Type(index) = operand {
                            Some(*index)
                        } else {
                            None
                        }
                    }),
                    0x1a3 => {
                        let resolved = value_types
                            .as_ref()
                            .and_then(|value_types| value_type_at(instruction, 0, value_types))
                            .and_then(|closure_type| {
                                usize::try_from(closure_type)
                                    .ok()
                                    .and_then(|index| types.entries.get(index))
                            })
                            .and_then(|ty| match ty {
                                BinaryType::Closure { function_type, .. } => Some(*function_type),
                                _ => None,
                            });
                        if resolved.is_none() {
                            all_call_types_known = false;
                        }
                        resolved
                    }
                    0x1a5 | 0x1a6 => {
                        let function_type = crate::binary_code::witness_or_vtable_function_type(
                            instruction,
                            signatures,
                            generics,
                            strings,
                            imports,
                            globals,
                        );
                        if function_type.is_none() {
                            all_call_types_known = false;
                        }
                        function_type
                    }
                    _ => None,
                };
                if matches!(instruction.opcode, 0x1a1 | 0x1a3 | 0x1a5 | 0x1a6 | 0x404) {
                    match function_type
                        .and_then(|type_index| available_ids.get(&type_index).copied())
                    {
                        Some(type_id) => {
                            required_ids.insert(type_id);
                        }
                        None => all_call_types_known = false,
                    }
                }
            }
        }
    }
    for type_id in required_ids.difference(&recorded_ids) {
        diagnostics.push(Diagnostic::error(
            V20,
            format!("ANALYSIS omits indirect-call typeid 0x{type_id:016x}"),
        ));
    }
    if all_call_types_known {
        for type_id in recorded_ids.difference(&required_ids) {
            if available_ids.values().any(|available| available == type_id) {
                diagnostics.push(Diagnostic::error(
                    V20,
                    format!("ANALYSIS includes unused indirect-call typeid 0x{type_id:016x}"),
                ));
            } else {
                diagnostics.push(Diagnostic::error(
                    V20,
                    format!(
                        "ANALYSIS indirect-call typeid 0x{type_id:016x} does not identify a function type"
                    ),
                ));
            }
        }
    }
    diagnostics
}

fn value_type_at(
    instruction: &tir_format::BinaryInstruction,
    operand_index: usize,
    value_types: &[Option<u64>],
) -> Option<u64> {
    let value = instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        })
        .nth(operand_index)?;
    usize::try_from(*value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn is_pointer_operand(
    instruction: &tir_format::BinaryInstruction,
    operand_index: usize,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> Option<bool> {
    let Some(value) = instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        })
        .nth(operand_index)
    else {
        return None;
    };
    let type_index = usize::try_from(*value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()?;
    let ty = usize::try_from(type_index)
        .ok()
        .and_then(|index| types.entries.get(index))?;
    Some(matches!(ty, BinaryType::Pointer(_)))
}

fn is_foreign_indirect_call(
    instruction: &tir_format::BinaryInstruction,
    types: &BinaryTypeTable,
) -> bool {
    instruction.opcode == 0x1a1
        && instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Type(index) => usize::try_from(*index)
                    .ok()
                    .and_then(|index| types.entries.get(index)),
                _ => None,
            })
            .is_some_and(|ty| matches!(ty, BinaryType::Function(function) if function.foreign))
}

fn assembly_error(function: &str, ordinal: u64, message: &str) -> Diagnostic {
    Diagnostic::error(
        V20,
        format!("{message} at instruction {ordinal} in @{function}"),
    )
}
