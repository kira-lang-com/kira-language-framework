use std::collections::BTreeSet;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    AnalysisSummary, AssemblyRegionSummary, BinaryFunctionBody, BinaryInstruction, BinaryOperand,
    FunctionRecordSummary, ImportCounts, IndirectCallType, TvisaImage, TypeSection,
};

const VFY50: Code = Code::new(Dialect::Tvisa, Area::Verifier, 50);
const ASM_REASON: u8 = 1 << 0;
const RAW_MEMORY_REASON: u8 = 1 << 1;
const INTTOPTR_REASON: u8 = 1 << 2;
const FOREIGN_REASON: u8 = 1 << 3;
const SETJMP_REASON: u8 = 1 << 4;
const UNCHECKED_ACCESS_REASON: u8 = 1 << 5;
const DEVICE_IO_REASON: u8 = 1 << 7;
const OTHER_UNMANAGED_REASONS: u8 = RAW_MEMORY_REASON
    | FOREIGN_REASON
    | SETJMP_REASON
    | UNCHECKED_ACCESS_REASON
    | (1 << 6)
    | DEVICE_IO_REASON;

pub(super) fn verify(
    image: &TvisaImage,
    bodies: &[BinaryFunctionBody],
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    types: &TypeSection,
    analysis: &AnalysisSummary,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    verify_function_trust(
        image,
        bodies,
        functions,
        imports,
        types,
        analysis,
        &mut diagnostics,
    );
    verify_indirect_call_types(bodies, analysis, &mut diagnostics);
    verify_assembly_regions(bodies, functions, imports, analysis, &mut diagnostics);
    diagnostics
}

fn verify_function_trust(
    image: &TvisaImage,
    bodies: &[BinaryFunctionBody],
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    types: &TypeSection,
    analysis: &AnalysisSummary,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if analysis.functions.len() != functions.len() {
        diagnostics.push(Diagnostic::error(
            VFY50,
            "ANALYSIS trust records do not match SIGNATURES",
        ));
        return;
    }
    for (index, function) in functions.iter().enumerate() {
        let record = analysis.functions[index];
        if record.trust_class != function.trust_class {
            diagnostics.push(Diagnostic::error(
                VFY50,
                format!(
                    "ANALYSIS trust class for @{} disagrees with SIGNATURES",
                    function.name
                ),
            ));
        }
    }

    for body in bodies {
        let Some(local_index) = body.header.function_index.checked_sub(imports.functions) else {
            continue;
        };
        let Some(index) = usize::try_from(local_index).ok() else {
            continue;
        };
        let (Some(function), Some(record)) = (functions.get(index), analysis.functions.get(index))
        else {
            continue;
        };
        let instructions = body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .collect::<Vec<_>>();
        let has_assembly = instructions
            .iter()
            .any(|instruction| matches!(instruction.opcode, 0x220 | 0x221));
        let has_inttoptr = instructions
            .iter()
            .any(|instruction| instruction.opcode == 0x0aa);
        let has_device_io = instructions
            .iter()
            .any(|instruction| (0x270..0x274).contains(&instruction.opcode));
        let has_foreign_call = instructions.iter().any(|instruction| {
            is_variadic_call(instruction)
                || is_platform_c_call(instruction, imports, types)
                || is_foreign_indirect_call(instruction, types)
        });
        let has_non_local_jump = instructions
            .iter()
            .any(|instruction| is_non_local_jump(instruction));
        let has_raw_memory = instructions.iter().any(|instruction| {
            instruction.opcode == 0x70a
                || runtime_entry_id(instruction)
                    .is_some_and(|index| matches!(index, 0x0001 | 0x0004 | 0x0005))
        });
        let has_unmanaged = instructions.iter().any(|instruction| {
            is_unmanaged_instruction(instruction)
                || is_platform_c_call(instruction, imports, types)
                || is_foreign_indirect_call(instruction, types)
        });

        verify_indirect_call_tags(&instructions, types, &function.name, diagnostics);

        require_reason(
            record.reason_mask,
            ASM_REASON,
            has_assembly,
            &function.name,
            "assembly",
            diagnostics,
        );
        require_reason(
            record.reason_mask,
            INTTOPTR_REASON,
            has_inttoptr,
            &function.name,
            "inttoptr",
            diagnostics,
        );
        require_reason(
            record.reason_mask,
            FOREIGN_REASON,
            has_foreign_call,
            &function.name,
            "foreign or variadic call",
            diagnostics,
        );
        require_reason(
            record.reason_mask,
            SETJMP_REASON,
            has_non_local_jump,
            &function.name,
            "setjmp/longjmp",
            diagnostics,
        );
        require_reason_present(
            record.reason_mask,
            RAW_MEMORY_REASON,
            has_raw_memory,
            &function.name,
            "raw memory",
            diagnostics,
        );
        require_reason(
            record.reason_mask,
            DEVICE_IO_REASON,
            has_device_io,
            &function.name,
            "device I/O",
            diagnostics,
        );

        let unowned_or_unsafe_reason = record.reason_mask & OTHER_UNMANAGED_REASONS != 0;
        if function.trust_class == 0 && (has_unmanaged || unowned_or_unsafe_reason) {
            diagnostics.push(Diagnostic::error(
                VFY50,
                format!(
                    "managed function @{} contains unmanaged content or reasons",
                    function.name
                ),
            ));
        }
        if function.trust_class == 2 && !has_unmanaged && record.reason_mask == 0 {
            diagnostics.push(Diagnostic::error(
                VFY50,
                format!(
                    "unmanaged function @{} has no unmanaged instruction or reason",
                    function.name
                ),
            ));
        }
        if function.trust_class == 1 && function.target != 0 {
            diagnostics.push(Diagnostic::error(
                VFY50,
                format!("GPU function @{} cannot be sandboxed", function.name),
            ));
        }
    }

    if functions.iter().any(|function| function.trust_class == 1)
        && image.header.profile == tir_target::Profile::Kernel
    {
        diagnostics.push(Diagnostic::error(
            VFY50,
            "kernel image cannot contain sandboxed functions",
        ));
    }
}

fn verify_indirect_call_types(
    bodies: &[BinaryFunctionBody],
    analysis: &AnalysisSummary,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let actual = bodies
        .iter()
        .flat_map(|body| &body.blocks)
        .flat_map(|block| &block.instructions)
        .filter(|instruction| is_indirect_call(instruction))
        .filter_map(|instruction| {
            let signature_index =
                instruction
                    .operands
                    .iter()
                    .find_map(|operand| match operand {
                        BinaryOperand::Index(index) => Some(*index),
                        _ => None,
                    })?;
            let type_tag = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::TypeTag(tag) => Some(*tag),
                    _ => None,
                })?;
            Some(IndirectCallType {
                signature_index,
                type_tag,
            })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if actual != analysis.indirect_call_types {
        diagnostics.push(Diagnostic::error(
            VFY50,
            "ANALYSIS indirect-call type table disagrees with CODE",
        ));
    }
}

fn verify_assembly_regions(
    bodies: &[BinaryFunctionBody],
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    analysis: &AnalysisSummary,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut actual = Vec::new();
    for body in bodies {
        let local_index = body.header.function_index.checked_sub(imports.functions);
        let architecture = local_index
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| functions.get(index))
            .and_then(|function| function.variant.as_ref())
            .map_or(0, |variant| variant.architecture);
        let mut instruction_ordinal = 0u64;
        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            if matches!(instruction.opcode, 0x220 | 0x221) {
                let record_index = instruction
                    .operands
                    .iter()
                    .find_map(|operand| match operand {
                        BinaryOperand::Index(index) => Some(*index),
                        _ => None,
                    });
                if let Some(assembly_record_index) = record_index {
                    actual.push(AssemblyRegionSummary {
                        function_index: body.header.function_index,
                        instruction_ordinal,
                        assembly_record_index,
                        architecture,
                        effects: ((instruction.modifiers >> 37) & 0x0f) as u8,
                    });
                }
            }
            instruction_ordinal = instruction_ordinal.saturating_add(1);
        }
    }
    if actual != analysis.assembly_regions {
        diagnostics.push(Diagnostic::error(
            VFY50,
            "ANALYSIS assembly-region inventory disagrees with CODE",
        ));
    }
}

fn require_reason(
    actual: u8,
    bit: u8,
    expected: bool,
    function: &str,
    label: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if (actual & bit != 0) != expected {
        diagnostics.push(Diagnostic::error(
            VFY50,
            format!("ANALYSIS {label} reason for @{function} disagrees with CODE"),
        ));
    }
}

fn require_reason_present(
    actual: u8,
    bit: u8,
    expected: bool,
    function: &str,
    label: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if expected && actual & bit == 0 {
        diagnostics.push(Diagnostic::error(
            VFY50,
            format!("ANALYSIS omits the {label} reason for @{function}"),
        ));
    }
}

fn is_unmanaged_instruction(instruction: &BinaryInstruction) -> bool {
    matches!(
        instruction.opcode,
        0x0aa | 0x210..=0x215 | 0x1a9 | 0x220 | 0x221 | 0x222 | 0x270..=0x273 | 0x70a
    ) || is_unmanaged_runtime(instruction)
}

fn is_unmanaged_runtime(instruction: &BinaryInstruction) -> bool {
    runtime_entry_id(instruction)
        .is_some_and(|index| matches!(index, 0x0001 | 0x0004 | 0x0005 | 0x0040 | 0x0041))
}

fn is_non_local_jump(instruction: &BinaryInstruction) -> bool {
    matches!(instruction.opcode, 0x214 | 0x215)
        || runtime_entry_id(instruction).is_some_and(|index| matches!(index, 0x0040 | 0x0041))
}

fn runtime_entry_id(instruction: &BinaryInstruction) -> Option<u64> {
    let is_runtime_call = instruction.opcode == 0x706
        || matches!(instruction.opcode, 0x1aa | 0x1e7) && (instruction.modifiers >> 48) & 0xf == 2;
    if !is_runtime_call {
        return None;
    }
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })
}

fn is_variadic_call(instruction: &BinaryInstruction) -> bool {
    instruction.opcode == 0x1a9
}

fn is_platform_c_call(
    instruction: &BinaryInstruction,
    imports: &ImportCounts,
    types: &TypeSection,
) -> bool {
    if !matches!(instruction.opcode, 0x1a0 | 0x1aa | 0x1e7)
        || matches!(instruction.opcode, 0x1aa | 0x1e7) && (instruction.modifiers >> 48) & 0xf != 0
    {
        return false;
    }
    let Some(symbol) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol(symbol) => Some(*symbol),
            _ => None,
        })
    else {
        return false;
    };
    if symbol & 3 != 0 {
        return false;
    }
    let function_index = symbol >> 2;
    let Ok(function_index) = usize::try_from(function_index) else {
        return false;
    };
    if u64::try_from(function_index).map_or(true, |index| index >= imports.functions)
        || imports.function_module_kinds.get(function_index) != Some(&2)
    {
        return false;
    }
    imports
        .function_signatures
        .get(function_index)
        .and_then(|signature| usize::try_from(*signature).ok())
        .and_then(|signature| types.signatures.get(signature))
        .is_some_and(|signature| signature.convention == 1)
}

fn is_indirect_call(instruction: &BinaryInstruction) -> bool {
    instruction.opcode == 0x1a1
        || matches!(instruction.opcode, 0x1aa | 0x1e7) && (instruction.modifiers >> 48) & 0xf == 1
}

fn is_foreign_indirect_call(instruction: &BinaryInstruction, types: &TypeSection) -> bool {
    if !is_indirect_call(instruction) || call_type_tag(instruction) != Some(0) {
        return false;
    }
    call_signature_index(instruction)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| types.signatures.get(index))
        .is_some_and(|signature| signature.convention == 1)
}

fn call_signature_index(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })
}

fn call_type_tag(instruction: &BinaryInstruction) -> Option<u32> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::TypeTag(tag) => Some(*tag),
            _ => None,
        })
}

fn verify_indirect_call_tags(
    instructions: &[&BinaryInstruction],
    types: &TypeSection,
    function: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for instruction in instructions
        .iter()
        .copied()
        .filter(|instruction| is_indirect_call(instruction))
    {
        let (Some(signature_index), Some(tag)) = (
            call_signature_index(instruction),
            call_type_tag(instruction),
        ) else {
            continue;
        };
        let convention = usize::try_from(signature_index)
            .ok()
            .and_then(|index| types.signatures.get(index))
            .map(|signature| signature.convention);
        if tag == 0 {
            if convention != Some(1) {
                diagnostics.push(Diagnostic::error(
                    VFY50,
                    format!("indirect call in @{function} omits its CFI tag without a C signature"),
                ));
            }
        } else if forbidden_type_tag(tag) {
            diagnostics.push(Diagnostic::error(
                VFY50,
                format!("indirect call in @{function} uses forbidden type tag 0x{tag:08X}"),
            ));
        }
    }
}

fn forbidden_type_tag(tag: u32) -> bool {
    tag & 0x000f_ffff == 0
        || tag & 0x0000_0fff == 0x017
        || tag == 0xfa1e_0ff3
        || tag.wrapping_neg() == 0xfa1e_0ff3
        || matches!(
            tag,
            0xd503_241f | 0xd503_245f | 0xd503_249f | 0xd503_24df | 0xd503_233f | 0xd503_237f
        )
}
