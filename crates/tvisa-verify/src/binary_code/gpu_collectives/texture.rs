use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, TvisaValueTypeRecord, TypeSection,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

#[derive(Clone, Copy)]
struct Texture {
    dimension: u8,
    component: u64,
    access: u8,
    multisampled: bool,
}

pub(in crate::binary_code) fn infer_result_types(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Option<Vec<Option<u64>>> {
    if !is_texture_opcode(instruction.opcode) {
        return None;
    }
    let count = usize::from(instruction.result_count);
    let mut results = vec![None; count];
    if let (Some(result), Some(texture)) = (
        results.first_mut(),
        texture_info(instruction, value_types, types),
    ) {
        *result = result_type(instruction, texture, types);
    }
    if instruction.modifiers & (1 << 35) != 0 {
        if let Some(result) = results.get_mut(1) {
            *result = integer_type(types, 1);
        }
    }
    Some(results)
}

pub(super) fn infer_result_type(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Option<u64> {
    result_type(
        instruction,
        texture_info(instruction, value_types, types)?,
        types,
    )
}

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    types: &TypeSection,
    value_types: &[Option<u64>],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if !is_texture_opcode(instruction.opcode) {
            continue;
        }
        let values = value_operands(instruction);
        let texture = texture_info(instruction, value_types, types);
        if texture.is_none_or(|texture| {
            invalid_operands(instruction, &values, texture, value_types, types)
        }) {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "GPU texture opcode 0x{:03X} has invalid operands or texture type",
                    instruction.opcode
                ),
            ));
        }
        if instruction.result_count != expected_results(instruction) {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "GPU texture opcode 0x{:03X} has an invalid result count",
                    instruction.opcode
                ),
            ));
        }
    }
    diagnostics
}

fn invalid_operands(
    instruction: &BinaryInstruction,
    values: &[u64],
    texture: Texture,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> bool {
    let opcode = instruction.opcode;
    let dimension = texture.dimension;
    let array = is_array(dimension);
    let sparse = instruction.modifiers & (1 << 35) != 0;
    let selector = (instruction.modifiers >> 48) & 0xf;
    let offset = offset_count(instruction);
    if dimension > 6
        || texture.access > 3
        || !component_type_supported(types, texture.component)
        || (sparse && !supports_sparse(opcode))
        || (opcode == 0x486 && selector > 3)
        || (opcode == 0x48b && selector > 6)
        || (!matches!(opcode, 0x486 | 0x48b) && selector != 0)
        || offset.is_none_or(|count| {
            count > 0 && (!supports_offset(opcode) || count != offset_dimensions(dimension))
        })
    {
        return true;
    }

    let type_at = |index: usize| value_record(values, index, value_types, types);
    let access_allowed = match opcode {
        0x480..=0x487 | 0x48c | 0x48d => matches!(texture.access, 0 | 3),
        0x488 => matches!(texture.access, 1 | 3),
        0x489 => matches!(texture.access, 2 | 3),
        0x48a => texture.access == 3,
        _ => true,
    };
    if !access_allowed {
        return true;
    }

    let core = core_value_count(opcode, texture);
    let has_min_lod = values.len() == core + 1 && supports_min_lod(opcode);
    if values.len() != core && !has_min_lod {
        return true;
    }

    match opcode {
        0x480..=0x487 | 0x48d => {
            if texture.multisampled
                || !type_at(1).is_some_and(is_sampler)
                || !type_at(2).is_some_and(|ty| coordinate_type(ty, dimension, true, types))
                || (array && !type_at(3).is_some_and(is_integer))
            {
                return true;
            }
            let tail = 3 + usize::from(array);
            let invalid = match opcode {
                0x480 | 0x486 | 0x48d => false,
                0x481 | 0x483 => !type_at(tail).is_some_and(is_f32),
                0x482 => {
                    !gradient_type(type_at(tail), dimension, types)
                        || !gradient_type(type_at(tail + 1), dimension, types)
                }
                0x484 => !type_at(tail).is_some_and(is_f32),
                0x485 => {
                    !type_at(tail).is_some_and(is_f32) || !type_at(tail + 1).is_some_and(is_f32)
                }
                0x487 => !type_at(tail).is_some_and(is_f32),
                _ => true,
            };
            invalid
                || (has_min_lod && !type_at(values.len() - 1).is_some_and(is_f32))
                || (matches!(opcode, 0x484 | 0x485 | 0x487)
                    && !component_is_f32(types, texture.component))
                || (opcode == 0x486 && !gather_component_supported(types, texture.component))
        }
        0x488 | 0x489 | 0x48a => {
            let coordinate =
                type_at(1).is_some_and(|ty| coordinate_type(ty, dimension, false, types));
            let layer = !array || type_at(2).is_some_and(is_integer);
            if !coordinate || !layer {
                return true;
            }
            let sample_index = 2 + usize::from(array);
            if texture.multisampled
                && (!matches!(opcode, 0x488 | 0x489)
                    || !type_at(sample_index).is_some_and(is_integer))
            {
                return true;
            }
            match opcode {
                0x488 => false,
                0x489 => {
                    let value_index = sample_index + usize::from(texture.multisampled);
                    value_type_index(values[value_index], value_types) != Some(texture.component)
                }
                0x48a => {
                    let operation = atomic_operation(instruction);
                    let value = value_type_index(values[sample_index], value_types);
                    !operation.is_some_and(|operation| operation <= 14)
                        || !atomic_component(types, texture.component, operation.unwrap_or(0))
                        || value != Some(texture.component)
                }
                _ => true,
            }
        }
        0x48b => query_selector(instruction).is_none(),
        0x48c => {
            texture.multisampled
                || !type_at(1).is_some_and(is_sampler)
                || !type_at(2).is_some_and(|ty| coordinate_type(ty, dimension, true, types))
                || (array && !type_at(3).is_some_and(is_integer))
        }
        _ => true,
    }
}

fn core_value_count(opcode: u64, texture: Texture) -> usize {
    let array = usize::from(is_array(texture.dimension));
    let sample = usize::from(texture.multisampled);
    match opcode {
        0x480 | 0x486 => 3 + array,
        0x481 | 0x483 | 0x484 | 0x487 => 4 + array,
        0x482 | 0x485 => 5 + array,
        0x488 => 2 + array + sample,
        0x489 => 3 + array + sample,
        0x48a => 3 + array,
        0x48b => 1,
        0x48c | 0x48d => 3 + array,
        _ => 0,
    }
}

fn result_type(
    instruction: &BinaryInstruction,
    texture: Texture,
    types: &TypeSection,
) -> Option<u64> {
    match instruction.opcode {
        0x484 | 0x485 | 0x487 => float_type(types),
        0x486 => gather_type(types, texture.component),
        0x48b => query_type(types, texture, query_selector(instruction)?),
        0x48c => vector_type(types, 2, float_type(types)?),
        _ => Some(texture.component),
    }
}

fn expected_results(instruction: &BinaryInstruction) -> u8 {
    if instruction.opcode == 0x489 {
        0
    } else {
        1 + u8::from(instruction.modifiers & (1 << 35) != 0)
    }
}

fn supports_sparse(opcode: u64) -> bool {
    matches!(opcode, 0x480..=0x488 | 0x48d)
}

fn supports_offset(opcode: u64) -> bool {
    matches!(opcode, 0x480..=0x487 | 0x48d)
}

fn supports_min_lod(opcode: u64) -> bool {
    supports_offset(opcode)
}

fn query_type(types: &TypeSection, texture: Texture, query: u64) -> Option<u64> {
    if query != 3 {
        return integer_type(types, 32);
    }
    match texture.dimension {
        0 => integer_type(types, 32),
        1 | 3 | 4 => vector_type(types, 2, integer_type(types, 32)?),
        2 | 5 | 6 => vector_type(types, 3, integer_type(types, 32)?),
        _ => None,
    }
}

fn gather_type(types: &TypeSection, component: u64) -> Option<u64> {
    let element = match types.value_types.get(component as usize)? {
        TvisaValueTypeRecord::FixedVector {
            lanes: 4,
            element_type,
        } => *element_type,
        _ => component,
    };
    vector_type(types, 4, element)
}

fn texture_info(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Option<Texture> {
    let value = value_operands(instruction).first().copied()?;
    let type_index = value_type_index(value, value_types)?;
    match types.value_types.get(type_index as usize)? {
        TvisaValueTypeRecord::Texture {
            dimension,
            component_type,
            access,
            multisampled,
        } => Some(Texture {
            dimension: *dimension,
            component: *component_type,
            access: *access,
            multisampled: *multisampled,
        }),
        _ => None,
    }
}

fn coordinate_type(
    ty: &TvisaValueTypeRecord,
    dimension: u8,
    normalized: bool,
    types: &TypeSection,
) -> bool {
    let lanes = spatial_dimensions(dimension);
    let element_valid = |element: &TvisaValueTypeRecord| {
        if normalized {
            matches!(element, TvisaValueTypeRecord::Float { tag: 0x02..=0x05 })
        } else {
            is_integer(element)
        }
    };
    if lanes == 1 {
        return element_valid(ty);
    }
    matches!(ty, TvisaValueTypeRecord::FixedVector { lanes: actual, element_type }
        if *actual == lanes as u64
            && types.value_types.get(*element_type as usize).is_some_and(element_valid))
}

fn gradient_type(ty: Option<&TvisaValueTypeRecord>, dimension: u8, types: &TypeSection) -> bool {
    let Some(ty) = ty else {
        return false;
    };
    match ty {
        TvisaValueTypeRecord::Float { tag: 0x04 } => spatial_dimensions(dimension) == 1,
        TvisaValueTypeRecord::FixedVector {
            lanes,
            element_type,
        } => {
            *lanes == spatial_dimensions(dimension) as u64
                && types
                    .value_types
                    .get(*element_type as usize)
                    .is_some_and(is_f32)
        }
        _ => false,
    }
}

fn component_type_supported(types: &TypeSection, component: u64) -> bool {
    match types.value_types.get(component as usize) {
        Some(TvisaValueTypeRecord::FixedVector {
            lanes: 4,
            element_type,
        }) => scalar_component_supported(types, *element_type),
        _ => scalar_component_supported(types, component),
    }
}

fn scalar_component_supported(types: &TypeSection, component: u64) -> bool {
    matches!(
        types.value_types.get(component as usize),
        Some(TvisaValueTypeRecord::Integer { bits: 32 })
            | Some(TvisaValueTypeRecord::Float { tag: 0x02 | 0x04 })
    )
}

fn gather_component_supported(types: &TypeSection, component: u64) -> bool {
    let scalar = match types.value_types.get(component as usize) {
        Some(TvisaValueTypeRecord::FixedVector {
            lanes: 4,
            element_type,
        }) => *element_type,
        _ => component,
    };
    scalar_component_supported(types, scalar)
}

fn component_is_f32(types: &TypeSection, component: u64) -> bool {
    matches!(
        types.value_types.get(component as usize),
        Some(TvisaValueTypeRecord::Float { tag: 0x04 })
    )
}

fn atomic_component(types: &TypeSection, component: u64, operation: u64) -> bool {
    match (operation, types.value_types.get(component as usize)) {
        (11..=14, Some(TvisaValueTypeRecord::Float { tag: 0x04 })) => true,
        (0..=10, Some(TvisaValueTypeRecord::Integer { bits: 32 })) => true,
        _ => false,
    }
}

fn atomic_operation(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .rev()
        .find_map(|operand| match operand {
            BinaryOperand::Index(operation) => Some(*operation),
            _ => None,
        })
}

fn offset_count(instruction: &BinaryInstruction) -> Option<usize> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Integers(offsets) => Some(offsets.len()),
            _ => None,
        })
}

fn query_selector(instruction: &BinaryInstruction) -> Option<u64> {
    (instruction.opcode == 0x48b)
        .then_some((instruction.modifiers >> 48) & 0xf)
        .filter(|selector| *selector <= 6)
}

fn spatial_dimensions(dimension: u8) -> usize {
    match dimension {
        0 | 4 => 1,
        1 | 5 => 2,
        2 | 3 | 6 => 3,
        _ => 0,
    }
}

fn offset_dimensions(dimension: u8) -> usize {
    match dimension {
        0 | 4 => 1,
        1 | 3 | 5 | 6 => 2,
        2 => 3,
        _ => 0,
    }
}

fn is_array(dimension: u8) -> bool {
    matches!(dimension, 4..=6)
}

fn is_texture_opcode(opcode: u64) -> bool {
    (0x480..=0x48d).contains(&opcode)
}

fn is_sampler(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::Sampler)
}

fn is_integer(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::Integer { .. })
}

fn is_f32(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::Float { tag: 0x04 })
}

fn float_type(types: &TypeSection) -> Option<u64> {
    types
        .value_types
        .iter()
        .position(is_f32)
        .map(|index| index as u64)
}

fn integer_type(types: &TypeSection, bits: u64) -> Option<u64> {
    types
        .value_types
        .iter()
        .position(
            |ty| matches!(ty, TvisaValueTypeRecord::Integer { bits: width } if *width == bits),
        )
        .map(|index| index as u64)
}

fn vector_type(types: &TypeSection, lanes: u64, element_type: u64) -> Option<u64> {
    types
        .value_types
        .iter()
        .position(|ty| {
            matches!(ty, TvisaValueTypeRecord::FixedVector { lanes: actual, element_type: element }
            if *actual == lanes && *element == element_type)
        })
        .map(|index| index as u64)
}

fn value_operands(instruction: &BinaryInstruction) -> Vec<u64> {
    instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        })
        .copied()
        .collect()
}

fn value_type_index(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn value_record<'a>(
    values: &[u64],
    index: usize,
    value_types: &[Option<u64>],
    types: &'a TypeSection,
) -> Option<&'a TvisaValueTypeRecord> {
    values
        .get(index)
        .and_then(|value| value_type_index(*value, value_types))
        .and_then(|index| types.value_types.get(index as usize))
}
