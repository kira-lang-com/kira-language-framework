use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::StringTable;
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, ConstantRecord, TvisaValueTypeRecord,
    TypeSection,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    types: &TypeSection,
    value_types: &[Option<u64>],
    strings: &StringTable,
    constants: &[ConstantRecord],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        match instruction.opcode {
            0x400 => verify_builtin(instruction, types, strings, &mut diagnostics),
            0x401 => verify_specialization(instruction, types, constants, &mut diagnostics),
            0x404 => verify_vft_call(instruction, types, value_types, &mut diagnostics),
            0x406 => {
                if !has_vector_type(types, 4, |types, element| {
                    matches!(
                        types.get(element as usize),
                        Some(TvisaValueTypeRecord::Float { tag: 0x04 })
                    )
                }) {
                    diagnostics.push(Diagnostic::error(
                        VFY3,
                        "frag.load_attachment requires vec<4 x f32> in TYPES",
                    ));
                }
            }
            0x409 => {
                if !types
                    .value_types
                    .iter()
                    .any(|ty| matches!(ty, TvisaValueTypeRecord::Pointer { address_space: 8 }))
                {
                    diagnostics.push(Diagnostic::error(
                        VFY3,
                        "frag.imageblock.addr requires ptr<tile> in TYPES",
                    ));
                }
            }
            _ => {}
        }
    }
    diagnostics
}

fn verify_builtin(
    instruction: &BinaryInstruction,
    types: &TypeSection,
    strings: &StringTable,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let name_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        });
    let name = name_index
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| strings.get(index));
    let actual = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        });
    let Some((name, actual)) = name.zip(actual) else {
        diagnostics.push(Diagnostic::error(
            VFY3,
            "GPU builtin is missing its name or type",
        ));
        return;
    };
    let matches = match name {
        "thread_position_in_grid"
        | "thread_position_in_threadgroup"
        | "threadgroup_position_in_grid"
        | "threads_per_threadgroup"
        | "threadgroups_per_grid" => has_vector_type(types, 3, |types, element| {
            matches!(
                types.get(element as usize),
                Some(TvisaValueTypeRecord::Integer { bits: 32 })
            )
        }),
        "thread_index_in_threadgroup"
        | "subgroup_index_in_threadgroup"
        | "subgroups_per_threadgroup"
        | "lane_index"
        | "subgroup_size"
        | "vertex_id"
        | "instance_id"
        | "base_vertex"
        | "base_instance"
        | "draw_index"
        | "view_index"
        | "sample_index"
        | "sample_mask_in"
        | "primitive_id"
        | "layer"
        | "viewport_index" => has_integer_type(types, actual, 32),
        "front_facing" | "helper_invocation" => has_integer_type(types, actual, 1),
        "position" => has_vector_type(types, 4, |types, element| {
            matches!(
                types.get(element as usize),
                Some(TvisaValueTypeRecord::Float { tag: 0x04 })
            )
        }),
        "barycentric_perspective" | "barycentric_linear" => {
            has_vector_type(types, 3, |types, element| {
                matches!(
                    types.get(element as usize),
                    Some(TvisaValueTypeRecord::Float { tag: 0x04 })
                )
            })
        }
        "point_coord" => has_vector_type(types, 2, |types, element| {
            matches!(
                types.get(element as usize),
                Some(TvisaValueTypeRecord::Float { tag: 0x04 })
            )
        }),
        "shading_rate" => has_vector_type(types, 2, |types, element| {
            matches!(
                types.get(element as usize),
                Some(TvisaValueTypeRecord::Integer { bits: 32 })
            )
        }),
        _ => {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!("unknown GPU builtin '{name}'"),
            ));
            return;
        }
    } && actual_type_matches(types, actual, name);
    if !matches {
        diagnostics.push(Diagnostic::error(
            VFY3,
            format!("GPU builtin '{name}' has the wrong value type"),
        ));
    }
}

fn actual_type_matches(types: &TypeSection, actual: u64, name: &str) -> bool {
    let expected = match name {
        "thread_position_in_grid"
        | "thread_position_in_threadgroup"
        | "threadgroup_position_in_grid"
        | "threads_per_threadgroup"
        | "threadgroups_per_grid" => has_vector_type(types, 3, |types, element| {
            matches!(
                types.get(element as usize),
                Some(TvisaValueTypeRecord::Integer { bits: 32 })
            )
        }),
        _ => true,
    };
    expected
        && types
            .value_types
            .get(actual as usize)
            .is_some_and(|actual_type| match name {
                "thread_position_in_grid"
                | "thread_position_in_threadgroup"
                | "threadgroup_position_in_grid"
                | "threads_per_threadgroup"
                | "threadgroups_per_grid" => matches!(
                    actual_type,
                    TvisaValueTypeRecord::FixedVector { lanes: 3, element_type }
                        if matches!(types.value_types.get(*element_type as usize), Some(TvisaValueTypeRecord::Integer { bits: 32 }))
                ),
                "thread_index_in_threadgroup"
                | "subgroup_index_in_threadgroup"
                | "subgroups_per_threadgroup"
                | "lane_index"
                | "subgroup_size"
                | "vertex_id"
                | "instance_id"
                | "base_vertex"
                | "base_instance"
                | "draw_index"
                | "view_index"
                | "sample_index"
                | "sample_mask_in"
                | "primitive_id"
                | "layer"
                | "viewport_index" => matches!(actual_type, TvisaValueTypeRecord::Integer { bits: 32 }),
                "front_facing" | "helper_invocation" => {
                    matches!(actual_type, TvisaValueTypeRecord::Integer { bits: 1 })
                }
                "position" => matches!(
                    actual_type,
                    TvisaValueTypeRecord::FixedVector { lanes: 4, element_type }
                        if matches!(types.value_types.get(*element_type as usize), Some(TvisaValueTypeRecord::Float { tag: 0x04 }))
                ),
                "barycentric_perspective" | "barycentric_linear" => matches!(
                    actual_type,
                    TvisaValueTypeRecord::FixedVector { lanes: 3, element_type }
                        if matches!(types.value_types.get(*element_type as usize), Some(TvisaValueTypeRecord::Float { tag: 0x04 }))
                ),
                "point_coord" => matches!(
                    actual_type,
                    TvisaValueTypeRecord::FixedVector { lanes: 2, element_type }
                        if matches!(types.value_types.get(*element_type as usize), Some(TvisaValueTypeRecord::Float { tag: 0x04 }))
                ),
                "shading_rate" => matches!(
                    actual_type,
                    TvisaValueTypeRecord::FixedVector { lanes: 2, element_type }
                        if matches!(types.value_types.get(*element_type as usize), Some(TvisaValueTypeRecord::Integer { bits: 32 }))
                ),
                _ => false,
            })
}

fn has_integer_type(types: &TypeSection, actual: u64, bits: u64) -> bool {
    matches!(
        types.value_types.get(actual as usize),
        Some(TvisaValueTypeRecord::Integer { bits: actual_bits }) if *actual_bits == bits
    )
}

fn has_vector_type(
    types: &TypeSection,
    lanes: u64,
    element_matches: impl Fn(&[TvisaValueTypeRecord], u64) -> bool,
) -> bool {
    types.value_types.iter().any(|ty| {
        matches!(ty, TvisaValueTypeRecord::FixedVector { lanes: actual, element_type } if *actual == lanes && element_matches(&types.value_types, *element_type))
    })
}

fn verify_specialization(
    instruction: &BinaryInstruction,
    types: &TypeSection,
    constants: &[ConstantRecord],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut indexes = instruction
        .operands
        .iter()
        .filter_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        });
    let id = indexes.next();
    let default_index = indexes.next();
    let type_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        });
    let valid_type = type_index.is_some_and(|index| specialization_type(types, index));
    if id.is_none_or(|id| id > u64::from(u32::MAX)) || !valid_type {
        diagnostics.push(Diagnostic::error(
            VFY3,
            "gpu.spec_const has an invalid ID or unsupported value type",
        ));
    }
    let default = default_index
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| constants.get(index));
    if default.is_none_or(|constant| Some(constant.type_index) != type_index) {
        diagnostics.push(Diagnostic::error(
            VFY3,
            "gpu.spec_const default constant has an invalid index or mismatched type",
        ));
    }
}

fn specialization_type(types: &TypeSection, type_index: u64) -> bool {
    match types.value_types.get(type_index as usize) {
        Some(TvisaValueTypeRecord::Integer { bits }) => (1..=64).contains(bits),
        Some(TvisaValueTypeRecord::Float { tag }) => matches!(tag, 0x02..=0x05),
        Some(TvisaValueTypeRecord::FixedVector {
            lanes,
            element_type,
        }) if matches!(lanes, 2 | 3 | 4) => {
            matches!(
                types.value_types.get(*element_type as usize),
                Some(TvisaValueTypeRecord::Integer { bits }) if *bits <= 64
            ) || matches!(
                types.value_types.get(*element_type as usize),
                Some(TvisaValueTypeRecord::Float { tag }) if matches!(tag, 0x02..=0x0c)
            )
        }
        _ => false,
    }
}

fn verify_vft_call(
    instruction: &BinaryInstruction,
    types: &TypeSection,
    value_types: &[Option<u64>],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let values = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Values(values) => Some(values.as_slice()),
            _ => None,
        });
    let signature_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        });
    let Some([table, index]) = values else {
        diagnostics.push(Diagnostic::error(
            VFY3,
            "gpu.call.vft requires a table and index value",
        ));
        return;
    };
    let actual_table = usize::try_from(*table)
        .ok()
        .and_then(|value| value_types.get(value))
        .copied()
        .flatten()
        .and_then(|type_index| types.value_types.get(type_index as usize));
    let index_type = usize::try_from(*index)
        .ok()
        .and_then(|value| value_types.get(value))
        .copied()
        .flatten()
        .and_then(|type_index| types.value_types.get(type_index as usize));
    let matches_signature = matches!(
        (actual_table, signature_index),
        (Some(TvisaValueTypeRecord::VisibleFunctionTable { signature: table_signature }), Some(call_signature))
            if *table_signature == call_signature
    );
    if !matches_signature {
        diagnostics.push(Diagnostic::error(
            VFY3,
            "gpu.call.vft signature does not match its visible function table",
        ));
    }
    if !matches!(index_type, Some(TvisaValueTypeRecord::Integer { .. })) {
        diagnostics.push(Diagnostic::error(
            VFY3,
            "gpu.call.vft table index must be an integer",
        ));
    }
    if signature_index
        .and_then(|index| types.signatures.get(index as usize))
        .is_none_or(|signature| signature.convention != 3 || signature.flags & 3 != 0)
    {
        diagnostics.push(Diagnostic::error(
            VFY3,
            "gpu.call.vft requires a non-failing, non-variadic GPU signature",
        ));
    }
}
