use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_target::Profile;
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, FunctionRecordSummary,
    TvisaValueTypeRecord, TypeSection,
};

const VFY14: Code = Code::new(Dialect::Tvisa, Area::Verifier, 14);
const VFY15: Code = Code::new(Dialect::Tvisa, Area::Verifier, 15);
const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);
const VFY17: Code = Code::new(Dialect::Tvisa, Area::Verifier, 17);
const VFY21: Code = Code::new(Dialect::Tvisa, Area::Verifier, 21);
const VFY40: Code = Code::new(Dialect::Tvisa, Area::Verifier, 40);

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    function: &FunctionRecordSummary,
    types: &TypeSection,
    value_types: &[Option<u64>],
    profile: Profile,
) -> Vec<Diagnostic> {
    let gpu = function.target != 0;
    let kernel_fp = function.attributes & (1 << 5) != 0;
    let no_fp_regs = function.attributes & (1 << 6) != 0;
    let mut diagnostics = Vec::new();
    if profile == Profile::Kernel && !kernel_fp && !no_fp_regs {
        diagnostics.push(Diagnostic::error(
            VFY14,
            format!(
                "kernel function @{} without kernel_fp must carry no_fp_regs",
                function.name
            ),
        ));
    } else if profile != Profile::Kernel && kernel_fp {
        diagnostics.push(Diagnostic::error(
            VFY14,
            format!(
                "kernel_fp is only valid in the kernel profile (@{})",
                function.name
            ),
        ));
    }

    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if (0x020..=0x045).contains(&instruction.opcode) && !valid_integer_modifiers(instruction) {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "integer instruction opcode 0x{:03X} has invalid modifiers",
                    instruction.opcode
                ),
            ));
        }
        if (0x060..=0x092).contains(&instruction.opcode) && !valid_float_modifiers(instruction) {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "floating-point instruction opcode 0x{:03X} has invalid modifiers",
                    instruction.opcode
                ),
            ));
        }
        if (0x0a0..=0x0ac).contains(&instruction.opcode) && !valid_conversion_modifiers(instruction)
        {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "conversion instruction opcode 0x{:03X} has invalid modifiers",
                    instruction.opcode
                ),
            ));
        }
        if instruction.opcode == 0x230
            && let Some(id) = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Index(id) => Some(*id),
                    _ => None,
                })
            && !valid_portable_intrinsic(instruction, id, types, value_types)
        {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!("portable intrinsic {id} operands do not match its signature"),
            ));
        }
        if gpu
            && instruction.opcode == 0x230
            && !instruction
                .operands
                .iter()
                .any(|operand| matches!(operand, BinaryOperand::Index(15 | 16)))
        {
            diagnostics.push(Diagnostic::error(
                VFY21,
                "GPU code permits only portable intrinsic IDs 15 and 16",
            ));
        }
        if instruction.opcode == 0x70b && !matches!(profile, Profile::System | Profile::Kernel) {
            diagnostics.push(Diagnostic::error(
                VFY40,
                "code.fromaddr is only valid in system and kernel profiles",
            ));
        }
        if !gpu && is_gpu_instruction(instruction.opcode) {
            diagnostics.push(Diagnostic::error(
                VFY14,
                format!(
                    "GPU opcode 0x{:X} appears in CPU function @{}",
                    instruction.opcode, function.name
                ),
            ));
        }
        if !gpu && instruction.opcode == 0x011 {
            diagnostics.push(Diagnostic::error(
                VFY14,
                format!("const.sampler appears in CPU function @{}", function.name),
            ));
        }
        if gpu && is_cpu_only_tvisa_instruction(instruction) {
            diagnostics.push(Diagnostic::error(
                VFY21,
                format!(
                    "TVISA opcode 0x{:03X} is forbidden in GPU function @{}",
                    instruction.opcode, function.name
                ),
            ));
        }
        if instruction.opcode == 0x011
            && !types
                .value_types
                .iter()
                .any(|ty| matches!(ty, TvisaValueTypeRecord::Sampler))
        {
            diagnostics.push(Diagnostic::error(
                VFY3,
                "const.sampler requires the sampler value type in TYPES",
            ));
        }
        if profile == Profile::Kernel && is_allocation_runtime_call(instruction) {
            diagnostics.push(Diagnostic::error(
                VFY14,
                format!(
                    "kernel function @{} calls a runtime allocation entry",
                    function.name
                ),
            ));
        }
        if function.semantics == 0 && is_permissive_only(instruction) {
            diagnostics.push(Diagnostic::error(
                VFY15,
                format!(
                    "opcode 0x{:X} uses a permissive-only modifier in defined function @{}",
                    instruction.opcode, function.name
                ),
            ));
        }
        if matches!(instruction.opcode, 0x1e9 | 0x707) && !valid_trap_code(instruction) {
            diagnostics.push(Diagnostic::error(
                VFY17,
                format!(
                    "trap code in opcode 0x{:X} must be a 16-bit integer",
                    instruction.opcode
                ),
            ));
        }
        if profile == Profile::Kernel && !kernel_fp && uses_float_or_vector(instruction, types) {
            diagnostics.push(Diagnostic::error(
                VFY14,
                format!(
                    "kernel function @{} without kernel_fp uses a floating-point or vector instruction",
                    function.name
                ),
            ));
        }
    }

    if profile == Profile::Kernel && !kernel_fp {
        let signature_uses_fp_or_vector = types
            .signatures
            .get(function.signature_index as usize)
            .is_some_and(|signature| {
                signature
                    .parameters
                    .iter()
                    .flat_map(parameter_type_indices)
                    .chain(signature.results.iter().flat_map(result_type_indices))
                    .any(|type_index| type_is_float_or_vector(types, type_index))
            });
        let body_uses_fp_or_vector = body.blocks.iter().any(|block| {
            block
                .parameter_types
                .iter()
                .any(|type_index| type_is_float_or_vector(types, *type_index))
        });
        if signature_uses_fp_or_vector || body_uses_fp_or_vector {
            diagnostics.push(Diagnostic::error(
                VFY14,
                format!(
                    "kernel function @{} without kernel_fp has floating-point or vector values",
                    function.name
                ),
            ));
        }
    }
    diagnostics
}

fn valid_portable_intrinsic(
    instruction: &BinaryInstruction,
    id: u64,
    types: &TypeSection,
    value_types: &[Option<u64>],
) -> bool {
    let values = instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        });
    let value_types = values
        .map(|value| {
            usize::try_from(*value)
                .ok()
                .and_then(|index| value_types.get(index))
                .copied()
                .flatten()
                .and_then(|index| types.value_types.get(index as usize))
        })
        .collect::<Vec<_>>();
    let integer_width = |index: usize| match value_types.get(index).copied().flatten() {
        Some(TvisaValueTypeRecord::Integer { bits }) => Some(*bits),
        _ => None,
    };
    let generic_pointer = |index: usize| {
        matches!(
            value_types.get(index).copied().flatten(),
            Some(TvisaValueTypeRecord::Pointer { address_space: 0 })
        )
    };
    let aes_vector = |index: usize| {
        matches!(
            value_types.get(index).copied().flatten(),
            Some(TvisaValueTypeRecord::FixedVector {
                lanes: 16,
                element_type,
            }) if matches!(types.value_types.get(*element_type as usize), Some(TvisaValueTypeRecord::Integer { bits: 8 }))
        )
    };
    match id {
        0 | 1 => integer_width(0) == Some(32) && matches!(integer_width(1), Some(8 | 16 | 32 | 64)),
        2..=5 => value_types.len() == 2 && aes_vector(0) && value_types[0] == value_types[1],
        6 => value_types.len() == 1 && aes_vector(0),
        7..=9 => value_types.len() == 2 && generic_pointer(0) && generic_pointer(1),
        10 => value_types.len() == 1 && generic_pointer(0),
        11..=14 => value_types.is_empty(),
        15 => {
            integer_width(0) == Some(1)
                && value_types.len() == 3
                && value_types[1] == value_types[2]
        }
        16 => {
            value_types.len() == 3
                && generic_pointer(0)
                && generic_pointer(1)
                && integer_width(2) == Some(64)
        }
        _ => false,
    }
}

fn parameter_type_indices(parameter: &tvisa_format::ParameterItemSummary) -> Vec<u64> {
    match parameter {
        tvisa_format::ParameterItemSummary::Leaf { type_index } => vec![*type_index],
        tvisa_format::ParameterItemSummary::Group(group) => group
            .members
            .iter()
            .map(|member| member.type_index)
            .collect(),
        tvisa_format::ParameterItemSummary::ByRef { .. }
        | tvisa_format::ParameterItemSummary::SRet { .. }
        | tvisa_format::ParameterItemSummary::Context
        | tvisa_format::ParameterItemSummary::CAggregate { .. } => Vec::new(),
    }
}

fn result_type_indices(result: &tvisa_format::ResultItemSummary) -> Vec<u64> {
    match result {
        tvisa_format::ResultItemSummary::Leaf { type_index } => vec![*type_index],
        tvisa_format::ResultItemSummary::Group(group) => group
            .members
            .iter()
            .map(|member| member.type_index)
            .collect(),
    }
}

fn type_is_float_or_vector(types: &TypeSection, type_index: u64) -> bool {
    matches!(
        types.value_types.get(type_index as usize),
        Some(
            TvisaValueTypeRecord::Float { .. }
                | TvisaValueTypeRecord::FixedVector { .. }
                | TvisaValueTypeRecord::ScalableVector { .. }
        )
    )
}

fn uses_float_or_vector(instruction: &BinaryInstruction, types: &TypeSection) -> bool {
    matches!(instruction.opcode, 0x060..=0x092 | 0x0c0..=0x0d7)
        || instruction.operands.iter().any(|operand| match operand {
            BinaryOperand::Type(type_index) => type_is_float_or_vector(types, *type_index),
            _ => false,
        })
}

fn is_gpu_instruction(opcode: u64) -> bool {
    (0x400..=0x5bf).contains(&opcode) || (0x5c0..=0x5c5).contains(&opcode)
}

fn is_cpu_only_tvisa_instruction(instruction: &BinaryInstruction) -> bool {
    let opcode = instruction.opcode;
    matches!(
        opcode,
        0x00f | 0x1a1 | 0x1a9 | 0x210..=0x215 | 0x220..=0x224 | 0x703..=0x706 | 0x708..=0x70f
    ) || (opcode == 0x1e7 && matches!((instruction.modifiers >> 48) & 0xf, 1 | 2))
}

fn is_allocation_runtime_call(instruction: &BinaryInstruction) -> bool {
    instruction.opcode == 0x706
        && instruction
            .operands
            .iter()
            .any(|operand| matches!(operand, BinaryOperand::Index(0x0001..=0x0004 | 0x0020)))
}

fn is_permissive_only(instruction: &BinaryInstruction) -> bool {
    let modifiers = instruction.modifiers;
    let opcode = instruction.opcode;
    matches!(opcode, 0x005 | 0x243)
        || (opcode == 0x1e8 && (modifiers >> 48) & 0xf == 1)
        || (matches!(opcode, 0x020 | 0x021 | 0x022 | 0x031 | 0x02f | 0x030) && modifiers & 3 == 3)
        || (opcode == 0x02c && modifiers & (1 << 4) != 0)
        || (matches!(opcode, 0x027 | 0x028 | 0x032 | 0x033) && modifiers & (1 << 3) != 0)
        || (matches!(opcode, 0x03e | 0x03f | 0x0a7 | 0x0a8) && modifiers & (1 << 47) != 0)
        || (opcode == 0x148 && modifiers & (1 << 5) != 0)
        || (opcode == 0x14a && modifiers & (1 << 31) != 0)
        || ((0x060..=0x092).contains(&opcode) && modifiers & ((1 << 6) | (1 << 7)) != 0)
}

fn valid_trap_code(instruction: &BinaryInstruction) -> bool {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Integer(value) => {
                Some(!value.negative && value.magnitude <= u128::from(u16::MAX))
            }
            _ => None,
        })
        == Some(true)
}

fn valid_integer_modifiers(instruction: &BinaryInstruction) -> bool {
    let modifiers = instruction.modifiers;
    let selector = (modifiers >> 48) & 0xf;
    let allowed = match instruction.opcode {
        0x020..=0x022 => 0x7,
        0x023..=0x026 => 1 << 2,
        0x027 => 0x3 | (1 << 3),
        0x028 => 1 << 3,
        0x029 | 0x02f | 0x030 => 0x3,
        0x02c => 1 << 4,
        0x031 => 0x7,
        0x032 | 0x033 => 0x3 | (1 << 3),
        0x03c => 0xf << 48,
        0x03e | 0x03f => 1 << 47,
        _ => 0,
    };
    if modifiers & !allowed != 0 {
        return false;
    }
    match instruction.opcode {
        0x020..=0x022 | 0x031 => modifiers & 0x3 != 0 || modifiers & (1 << 2) == 0,
        0x027 | 0x029 | 0x032 | 0x033 => modifiers & 0x3 <= 1,
        0x02f | 0x030 => modifiers & 0x3 != 2,
        0x03c => selector <= 9,
        _ => true,
    }
}

fn valid_float_modifiers(instruction: &BinaryInstruction) -> bool {
    let modifiers = instruction.modifiers;
    let selector = (modifiers >> 48) & 0xf;
    let allowed = (0x7f << 6)
        | (0x7 << 13)
        | (1 << 16)
        | (1 << 17)
        | if instruction.opcode == 0x074 {
            0xf << 48
        } else {
            0
        };
    modifiers & !allowed == 0
        && (modifiers >> 13) & 0x7 <= 5
        && (instruction.opcode == 0x074 || selector == 0)
        && (!matches!(instruction.opcode, 0x078 | 0x079) || modifiers == 0)
}

fn valid_conversion_modifiers(instruction: &BinaryInstruction) -> bool {
    let modifiers = instruction.modifiers;
    let rounding = (modifiers >> 13) & 0x7;
    let selector = (modifiers >> 48) & 0xf;
    match instruction.opcode {
        0x0a3 | 0x0a5 | 0x0a6 => modifiers & !(0x7 << 13) == 0 && rounding <= 5,
        0x0a7 | 0x0a8 => {
            let mode = modifiers & 0x3;
            let poison = modifiers & (1 << 47) != 0;
            modifiers & !(0x3 | (1 << 47)) == 0
                && if poison {
                    mode == 0
                } else {
                    matches!(mode, 1 | 2)
                }
        }
        0x0ac => modifiers & !((0x7 << 13) | (0xf << 48)) == 0 && rounding <= 5 && selector <= 2,
        _ => modifiers == 0,
    }
}
