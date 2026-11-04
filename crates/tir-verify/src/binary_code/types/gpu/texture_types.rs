use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable,
};
use tir_types::FloatType;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
        .filter(|instruction| (0x480..=0x48d).contains(&instruction.opcode))
    {
        let Some(values) = instruction
            .operands
            .first()
            .and_then(|operand| match operand {
                BinaryOperand::Values(values) => Some(values.as_slice()),
                _ => None,
            })
        else {
            report(
                &mut diagnostics,
                function_name,
                instruction,
                "texture instruction has no value operands",
            );
            continue;
        };
        let Some(texture_value) = values.first() else {
            continue;
        };
        let texture_type = value_type(*texture_value, value_types);
        let Some(BinaryType::Texture {
            dimension,
            component,
            access,
            multisampled,
        }) = texture_type.and_then(|index| type_at(types, index))
        else {
            if texture_type.is_some() {
                report(
                    &mut diagnostics,
                    function_name,
                    instruction,
                    "texture instruction requires a texture handle",
                );
            }
            continue;
        };
        verify_instruction(
            instruction,
            values,
            *dimension,
            *component,
            *access,
            *multisampled,
            value_types,
            types,
            function_name,
            &mut diagnostics,
        );
    }
    diagnostics
}

#[allow(clippy::too_many_arguments)]
fn verify_instruction(
    instruction: &BinaryInstruction,
    values: &[u64],
    dimension: u8,
    component: u64,
    access: u8,
    multisampled: bool,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let opcode = instruction.opcode;
    let sampler_operation = matches!(opcode, 0x480..=0x487 | 0x48c | 0x48d);
    let coordinates = match opcode {
        0x480..=0x487 | 0x48c | 0x48d => Some(2),
        0x488..=0x48a => Some(1),
        _ => None,
    };
    let array = matches!(dimension, 4..=6);
    let mut base_count = coordinates.map_or(1, |index| index + 1);
    if array && let Some(coordinate_index) = coordinates {
        let layer_index = coordinate_index + 1;
        if values
            .get(layer_index)
            .and_then(|value| value_type(*value, value_types))
            .is_some_and(|ty| !is_integer(types, ty))
        {
            report(
                diagnostics,
                function_name,
                instruction,
                "texture array layer must be an integer",
            );
        }
        base_count += 1;
    }
    if let Some(coordinate_index) = coordinates {
        let normalized = sampler_operation;
        if values
            .get(coordinate_index)
            .and_then(|value| value_type(*value, value_types))
            .is_some_and(|ty| !valid_coordinates(types, ty, dimension, normalized))
        {
            report(
                diagnostics,
                function_name,
                instruction,
                if normalized {
                    "texture coordinates must contain floating-point values with the dimension's lane count"
                } else {
                    "texture coordinates must contain integer values with the dimension's lane count"
                },
            );
        }
    }
    if sampler_operation {
        if values
            .get(1)
            .and_then(|value| value_type(*value, value_types))
            .and_then(|ty| type_at(types, ty))
            .is_some_and(|ty| !matches!(ty, BinaryType::Sampler))
        {
            report(
                diagnostics,
                function_name,
                instruction,
                "texture sampling requires a sampler handle",
            );
        }
        if !matches!(access, 0 | 3) {
            report(
                diagnostics,
                function_name,
                instruction,
                "texture access mode does not permit sampling",
            );
        }
        if multisampled {
            report(
                diagnostics,
                function_name,
                instruction,
                "sampler operations require a non-multisampled texture",
            );
        }
    }
    verify_offsets(instruction, dimension, function_name, diagnostics);
    match opcode {
        0x488 => {
            if !matches!(access, 1 | 3) {
                report(
                    diagnostics,
                    function_name,
                    instruction,
                    "texture access mode does not permit reads",
                );
            }
            if multisampled
                && values.get(base_count).is_some_and(|value| {
                    value_type(*value, value_types).is_some_and(|ty| !is_integer(types, ty))
                })
            {
                report(
                    diagnostics,
                    function_name,
                    instruction,
                    "multisampled texture index must be an integer",
                );
            }
        }
        0x489 => {
            if !matches!(access, 2 | 3) {
                report(
                    diagnostics,
                    function_name,
                    instruction,
                    "texture access mode does not permit writes",
                );
            }
            let value_index = base_count + usize::from(multisampled);
            if multisampled
                && values.get(base_count).is_some_and(|value| {
                    value_type(*value, value_types).is_some_and(|ty| !is_integer(types, ty))
                })
            {
                report(
                    diagnostics,
                    function_name,
                    instruction,
                    "multisampled texture index must be an integer",
                );
            }
            if values
                .get(value_index)
                .and_then(|value| value_type(*value, value_types))
                .is_some_and(|ty| ty != component)
            {
                report(
                    diagnostics,
                    function_name,
                    instruction,
                    "texture write value type disagrees with its component type",
                );
            }
        }
        0x48a => verify_atomic(
            instruction,
            values,
            base_count,
            component,
            access,
            multisampled,
            value_types,
            types,
            function_name,
            diagnostics,
        ),
        0x48b => {}
        0x48c => {
            if values
                .get(1)
                .and_then(|value| value_type(*value, value_types))
                .and_then(|ty| type_at(types, ty))
                .is_some_and(|ty| !matches!(ty, BinaryType::Sampler))
            {
                report(
                    diagnostics,
                    function_name,
                    instruction,
                    "tex.lod requires a sampler handle",
                );
            }
            if multisampled {
                report(
                    diagnostics,
                    function_name,
                    instruction,
                    "tex.lod requires a non-multisampled texture",
                );
            }
        }
        _ => {}
    }
    verify_operand_count(
        instruction,
        values,
        base_count,
        multisampled,
        function_name,
        diagnostics,
    );
    verify_sampling_extras(
        instruction,
        values,
        base_count,
        component,
        dimension,
        value_types,
        types,
        function_name,
        diagnostics,
    );
}

fn verify_operand_count(
    instruction: &BinaryInstruction,
    values: &[u64],
    base_count: usize,
    multisampled: bool,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let multisample_index = usize::from(multisampled);
    let (minimum, maximum) = match instruction.opcode {
        0x480 | 0x486 | 0x48d => (0, 1),
        0x481 | 0x483 | 0x484 | 0x487 => (1, 2),
        0x482 | 0x485 => (2, 3),
        0x488 => (multisample_index, multisample_index),
        0x489 => (multisample_index + 1, multisample_index + 1),
        0x48a => (1, 1),
        0x48b | 0x48c => (0, 0),
        _ => (0, usize::MAX),
    };
    let actual = values.len().saturating_sub(base_count);
    if actual < minimum || actual > maximum {
        report(
            diagnostics,
            function_name,
            instruction,
            "texture instruction has an invalid number of value operands",
        );
    }
}

fn verify_sampling_extras(
    instruction: &BinaryInstruction,
    values: &[u64],
    base_count: usize,
    component: u64,
    dimension: u8,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let opcode = instruction.opcode;
    let extra_types: &[u8] = match opcode {
        0x481 | 0x483 | 0x484 => &[0],
        0x482 => &[1, 1],
        0x485 => &[0, 0],
        0x487 => &[0],
        _ => &[],
    };
    for (offset, kind) in extra_types.iter().enumerate() {
        if values
            .get(base_count + offset)
            .and_then(|value| value_type(*value, value_types))
            .is_some_and(|ty| match kind {
                0 => !is_f32(types, ty),
                _ => !valid_f32_coordinates(types, ty, coordinate_lanes(dimension)),
            })
        {
            report(
                diagnostics,
                function_name,
                instruction,
                "texture sampling operand has an invalid type",
            );
        }
    }
    let min_lod_index = match opcode {
        0x480 | 0x483 | 0x484 | 0x486 | 0x48d => Some(base_count),
        0x481 => Some(base_count + 1),
        0x482 => Some(base_count + 2),
        0x485 | 0x487 => Some(base_count + 1),
        _ => None,
    };
    if min_lod_index.is_some_and(|index| {
        values
            .get(index)
            .and_then(|value| value_type(*value, value_types))
            .is_some_and(|ty| !is_f32(types, ty))
    }) {
        report(
            diagnostics,
            function_name,
            instruction,
            "texture min_lod must be f32",
        );
    }
    if matches!(opcode, 0x484 | 0x485 | 0x487) && !is_f32(types, component) {
        report(
            diagnostics,
            function_name,
            instruction,
            "comparison texture operations require an f32 component",
        );
    }
    if opcode == 0x484 || opcode == 0x485 {
        let reference_index = base_count;
        if values
            .get(reference_index)
            .and_then(|value| value_type(*value, value_types))
            .is_some_and(|ty| !is_f32(types, ty))
        {
            report(
                diagnostics,
                function_name,
                instruction,
                "texture compare reference must be f32",
            );
        }
    }
    if opcode == 0x487
        && values
            .get(base_count)
            .and_then(|value| value_type(*value, value_types))
            .is_some_and(|ty| !is_f32(types, ty))
    {
        report(
            diagnostics,
            function_name,
            instruction,
            "texture gather compare reference must be f32",
        );
    }
}

fn verify_atomic(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_index: usize,
    component: u64,
    access: u8,
    multisampled: bool,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if access != 3 {
        report(
            diagnostics,
            function_name,
            instruction,
            "texture access mode does not permit atomics",
        );
    }
    if multisampled {
        report(
            diagnostics,
            function_name,
            instruction,
            "texture atomics do not support multisampled textures",
        );
    }
    let operation = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        });
    let float_operation = operation.is_some_and(|operation| (11..=14).contains(&operation));
    let valid_component = if float_operation {
        is_f32(types, component)
    } else {
        is_i32(types, component)
    };
    if !valid_component {
        report(
            diagnostics,
            function_name,
            instruction,
            "texture atomic operation requires a matching scalar i32 or f32 component",
        );
    }
    if values
        .get(value_index)
        .and_then(|value| value_type(*value, value_types))
        .is_some_and(|ty| ty != component)
    {
        report(
            diagnostics,
            function_name,
            instruction,
            "texture atomic value type disagrees with its component type",
        );
    }
}

fn verify_offsets(
    instruction: &BinaryInstruction,
    dimension: u8,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(offsets) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Integers(offsets) => Some(offsets),
            _ => None,
        })
    else {
        return;
    };
    if !offsets.is_empty() {
        let expected = if matches!(dimension, 3 | 6) {
            2
        } else {
            coordinate_lanes(dimension)
        };
        if offsets.len() != expected {
            report(
                diagnostics,
                function_name,
                instruction,
                "texture offset width does not match its spatial dimension",
            );
        }
    }
}

fn valid_coordinates(
    types: &BinaryTypeTable,
    type_index: u64,
    dimension: u8,
    normalized: bool,
) -> bool {
    let Some(ty) = type_at(types, type_index) else {
        return false;
    };
    let lanes = coordinate_lanes(dimension);
    match ty {
        BinaryType::FixedVector {
            lanes: actual,
            element,
        } => *actual == lanes as u64 && coordinate_element_valid(types, *element, normalized),
        _ if lanes == 1 => coordinate_element_valid(types, type_index, normalized),
        _ => false,
    }
}

fn valid_f32_coordinates(types: &BinaryTypeTable, type_index: u64, lanes: usize) -> bool {
    let Some(ty) = type_at(types, type_index) else {
        return false;
    };
    matches!(
        ty,
        BinaryType::FixedVector {
            lanes: actual,
            element
        } if *actual as usize == lanes && is_f32(types, *element)
    )
}

fn coordinate_element_valid(types: &BinaryTypeTable, type_index: u64, normalized: bool) -> bool {
    if normalized {
        matches!(
            type_at(types, type_index),
            Some(BinaryType::Float(
                FloatType::F16 | FloatType::F32 | FloatType::F64
            ))
        )
    } else {
        is_integer(types, type_index)
    }
}

fn coordinate_lanes(dimension: u8) -> usize {
    match dimension {
        0 | 4 => 1,
        1 | 5 => 2,
        _ => 3,
    }
}

fn value_type(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn type_at(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    usize::try_from(index)
        .ok()
        .and_then(|index| types.entries.get(index))
}

fn is_integer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Integer(_)))
}

fn is_i32(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Integer(32)))
}

fn is_f32(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        type_at(types, index),
        Some(BinaryType::Float(FloatType::F32))
    )
}

fn report(
    diagnostics: &mut Vec<Diagnostic>,
    function_name: &str,
    instruction: &BinaryInstruction,
    message: &str,
) {
    diagnostics.push(Diagnostic::error(
        V2,
        format!(
            "function @{function_name} CODE offset {}: {message}",
            instruction.offset
        ),
    ));
}
