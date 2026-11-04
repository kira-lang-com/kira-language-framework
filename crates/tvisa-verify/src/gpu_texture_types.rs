use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_types::{FloatType, TextureAccess, TextureDimension, Type};

mod syntax;

use syntax::{
    has_keyword, has_offset_marker, has_sparse_modifier, keyword_atom, keyword_value,
    offset_components, query_name,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify(instruction: &TextInstruction, operands: &[Type]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let opcode = instruction.opcode.as_str();
    if !is_texture_opcode(opcode) {
        return diagnostics;
    }
    let Some((dimension, component, access, multisampled)) =
        operands.first().and_then(texture_type)
    else {
        error(
            &mut diagnostics,
            instruction,
            "texture instruction requires a texture handle",
        );
        return diagnostics;
    };

    verify_operand_layout(
        instruction,
        operands,
        dimension,
        multisampled,
        &mut diagnostics,
    );
    let array = is_array(dimension);
    if is_sampling(opcode) || opcode == "tex.lod" {
        if operands.get(1).is_some_and(|ty| ty != &Type::Sampler) {
            error(
                &mut diagnostics,
                instruction,
                "texture operation requires a sampler handle",
            );
        }
        if !matches!(access, TextureAccess::Sample | TextureAccess::ReadWrite) {
            error(
                &mut diagnostics,
                instruction,
                "texture access mode does not permit sampling",
            );
        }
        verify_coordinates(
            instruction,
            operands.get(2),
            dimension,
            true,
            &mut diagnostics,
        );
        if array && operands.get(3).is_some_and(|ty| !is_integer(ty)) {
            error(
                &mut diagnostics,
                instruction,
                "texture array layer must be an integer",
            );
        }
        if multisampled {
            error(
                &mut diagnostics,
                instruction,
                "sampler operations do not support multisampled textures",
            );
        }
        verify_sampling_operands(
            instruction,
            operands,
            dimension,
            component,
            array,
            &mut diagnostics,
        );
    } else if matches!(opcode, "tex.read" | "tex.write" | "tex.atomic") {
        verify_coordinates(
            instruction,
            operands.get(1),
            dimension,
            false,
            &mut diagnostics,
        );
        if array && operands.get(2).is_some_and(|ty| !is_integer(ty)) {
            error(
                &mut diagnostics,
                instruction,
                "texture array layer must be an integer",
            );
        }
        let access_allowed = match opcode {
            "tex.read" => matches!(access, TextureAccess::Read | TextureAccess::ReadWrite),
            "tex.write" => matches!(access, TextureAccess::Write | TextureAccess::ReadWrite),
            _ => access == TextureAccess::ReadWrite,
        };
        if !access_allowed {
            error(
                &mut diagnostics,
                instruction,
                "texture access mode does not permit this operation",
            );
        }
        verify_texel_operands(
            instruction,
            operands,
            component,
            array,
            multisampled,
            &mut diagnostics,
        );
    } else if opcode == "tex.query" {
        if query_type(instruction, dimension).is_none() {
            error(
                &mut diagnostics,
                instruction,
                "tex.query has an unsupported query field",
            );
        }
    }

    if has_sparse_modifier(instruction) && !supports_sparse(opcode) {
        error(
            &mut diagnostics,
            instruction,
            "sparse modifier is not supported by this texture instruction",
        );
    }
    if let Some(offsets) = offset_components(instruction) {
        if !supports_offset(opcode) || offsets.len() != offset_dimensions(dimension) {
            error(
                &mut diagnostics,
                instruction,
                "texture offset has the wrong number of components",
            );
        }
        if offsets.iter().any(|value| !(-8..=7).contains(value)) {
            error(
                &mut diagnostics,
                instruction,
                "texture offset components must be in −8…7",
            );
        }
    } else if has_offset_marker(instruction) {
        error(
            &mut diagnostics,
            instruction,
            "texture offset must be an integer literal vector",
        );
    }
    diagnostics
}

fn verify_operand_layout(
    instruction: &TextInstruction,
    operands: &[Type],
    dimension: TextureDimension,
    multisampled: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let opcode = instruction.opcode.as_str();
    let array = usize::from(is_array(dimension));
    let sample = usize::from(multisampled);
    let core = match opcode {
        "tex.sample" | "tex.gather" => 3 + array,
        "tex.sample_level" | "tex.sample_bias" | "tex.sample_compare" | "tex.gather_compare" => {
            4 + array
        }
        "tex.sample_grad" | "tex.sample_compare_level" => 5 + array,
        "tex.read" => 2 + array + sample,
        "tex.write" => 3 + array + sample,
        "tex.atomic" => 3 + array,
        "tex.query" => 1,
        "tex.lod" | "tex.feedback" => 3 + array,
        _ => return,
    };
    let has_min_lod = has_keyword(instruction, "min_lod");
    let expected = core + usize::from(has_min_lod);
    if operands.len() != expected {
        error(
            diagnostics,
            instruction,
            format!(
                "{} has {} value operands; expected {expected}",
                opcode,
                operands.len()
            ),
        );
    }

    let required_fields: &[&str] = match opcode {
        "tex.sample_level" => &["lod"],
        "tex.sample_grad" => &["grad"],
        "tex.sample_bias" => &["bias"],
        "tex.sample_compare" => &["compare"],
        "tex.sample_compare_level" => &["compare", "lod"],
        _ => &[],
    };
    for field in required_fields {
        if !has_keyword(instruction, field) {
            error(
                diagnostics,
                instruction,
                format!("{} requires '{field}'", opcode),
            );
        }
    }
    if has_min_lod && !supports_min_lod(opcode) {
        error(
            diagnostics,
            instruction,
            "min_lod is not supported by this texture instruction",
        );
    }
    if opcode == "tex.gather"
        && let Some(component) = keyword_value(instruction, "component")
        && !tir_format::numeric_literals::parse_unsigned_integer(component)
            .is_some_and(|value| value <= 3)
    {
        error(
            diagnostics,
            instruction,
            "tex.gather component must be in 0…3",
        );
    }
    if opcode == "tex.query" && query_name(instruction).is_none() {
        error(
            diagnostics,
            instruction,
            "tex.query requires one query field",
        );
    }
}

fn verify_sampling_operands(
    instruction: &TextInstruction,
    operands: &[Type],
    dimension: TextureDimension,
    component: &Type,
    array: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let opcode = instruction.opcode.as_str();
    let tail = 3 + usize::from(array);
    let f32_type = Type::Float(FloatType::F32);
    let invalid = match opcode {
        "tex.sample_level" | "tex.sample_bias" | "tex.sample_compare" => {
            operands.get(tail) != Some(&f32_type)
        }
        "tex.sample_grad" => {
            !operands
                .get(tail)
                .is_some_and(|ty| gradient_type(ty, dimension))
                || !operands
                    .get(tail + 1)
                    .is_some_and(|ty| gradient_type(ty, dimension))
        }
        "tex.sample_compare_level" => {
            operands.get(tail) != Some(&f32_type) || operands.get(tail + 1) != Some(&f32_type)
        }
        "tex.gather_compare" => operands.get(tail) != Some(&f32_type),
        _ => false,
    };
    if invalid {
        error(
            diagnostics,
            instruction,
            format!("{} has a mistyped sampling operand", opcode),
        );
    }
    if matches!(
        opcode,
        "tex.sample_compare" | "tex.sample_compare_level" | "tex.gather_compare"
    ) && component != &f32_type
    {
        error(
            diagnostics,
            instruction,
            "comparison sampling requires an f32 texture component",
        );
    }
    if has_keyword(instruction, "min_lod") && operands.last() != Some(&f32_type) {
        error(diagnostics, instruction, "min_lod value must have type f32");
    }
}

fn verify_texel_operands(
    instruction: &TextInstruction,
    operands: &[Type],
    component: &Type,
    array: bool,
    multisampled: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let opcode = instruction.opcode.as_str();
    let sample_index = 2 + usize::from(array);
    if opcode == "tex.atomic" && multisampled {
        error(
            diagnostics,
            instruction,
            "tex.atomic does not support multisampled textures",
        );
    }
    if multisampled
        && matches!(opcode, "tex.read" | "tex.write")
        && operands.get(sample_index).is_some_and(|ty| !is_integer(ty))
    {
        error(
            diagnostics,
            instruction,
            "multisample index must be an integer",
        );
    }
    match opcode {
        "tex.write" => {
            if operands.last().is_some_and(|ty| ty != component) {
                error(
                    diagnostics,
                    instruction,
                    format!("tex.write value must have type {component}"),
                );
            }
        }
        "tex.atomic" => {
            let operation = keyword_atom(
                instruction,
                &[
                    "xchg", "add", "sub", "and", "nand", "or", "xor", "max", "min", "umax", "umin",
                    "fadd", "fsub", "fmax", "fmin",
                ],
            );
            let integer_operation = matches!(
                operation,
                Some(
                    "xchg"
                        | "add"
                        | "sub"
                        | "and"
                        | "nand"
                        | "or"
                        | "xor"
                        | "max"
                        | "min"
                        | "umax"
                        | "umin"
                )
            );
            let expected = if integer_operation {
                Type::Integer(32)
            } else {
                Type::Float(FloatType::F32)
            };
            if operation.is_none() || component != &expected || operands.last() != Some(component) {
                error(
                    diagnostics,
                    instruction,
                    "tex.atomic operation and value must match an i32 or f32 component",
                );
            }
        }
        _ => {}
    }
}

pub(super) fn infer_result_types(instruction: &TextInstruction, operands: &[Type]) -> Vec<Type> {
    let opcode = instruction.opcode.as_str();
    if opcode == "tex.write" {
        return Vec::new();
    }
    let Some((dimension, component, _, _)) = operands.first().and_then(texture_type) else {
        return Vec::new();
    };
    let Some(result) = (match opcode {
        "tex.sample" | "tex.sample_level" | "tex.sample_grad" | "tex.sample_bias" | "tex.read"
        | "tex.atomic" | "tex.feedback" => Some(component.clone()),
        "tex.sample_compare" | "tex.sample_compare_level" => Some(Type::Float(FloatType::F32)),
        "tex.gather" => Some(gather_type(component)),
        "tex.gather_compare" => Some(vector(4, Type::Float(FloatType::F32))),
        "tex.query" => query_type(instruction, dimension),
        "tex.lod" => Some(vector(2, Type::Float(FloatType::F32))),
        _ => None,
    }) else {
        return Vec::new();
    };
    let mut results = vec![result];
    if supports_sparse(opcode) && has_sparse_modifier(instruction) {
        results.push(Type::Integer(1));
    }
    results
}

pub(super) fn result_arity(instruction: &TextInstruction) -> Option<usize> {
    let opcode = instruction.opcode.as_str();
    if opcode == "tex.write" {
        return Some(0);
    }
    if is_texture_opcode(opcode) {
        return Some(1 + usize::from(supports_sparse(opcode) && has_sparse_modifier(instruction)));
    }
    None
}

fn is_texture_opcode(opcode: &str) -> bool {
    matches!(
        opcode,
        "tex.sample"
            | "tex.sample_level"
            | "tex.sample_grad"
            | "tex.sample_bias"
            | "tex.sample_compare"
            | "tex.sample_compare_level"
            | "tex.gather"
            | "tex.gather_compare"
            | "tex.read"
            | "tex.write"
            | "tex.atomic"
            | "tex.query"
            | "tex.lod"
            | "tex.feedback"
    )
}

fn is_sampling(opcode: &str) -> bool {
    matches!(
        opcode,
        "tex.sample"
            | "tex.sample_level"
            | "tex.sample_grad"
            | "tex.sample_bias"
            | "tex.sample_compare"
            | "tex.sample_compare_level"
            | "tex.gather"
            | "tex.gather_compare"
            | "tex.feedback"
    )
}

fn supports_sparse(opcode: &str) -> bool {
    matches!(
        opcode,
        "tex.sample"
            | "tex.sample_level"
            | "tex.sample_grad"
            | "tex.sample_bias"
            | "tex.sample_compare"
            | "tex.sample_compare_level"
            | "tex.gather"
            | "tex.gather_compare"
            | "tex.read"
            | "tex.feedback"
    )
}

fn supports_offset(opcode: &str) -> bool {
    matches!(
        opcode,
        "tex.sample"
            | "tex.sample_level"
            | "tex.sample_grad"
            | "tex.sample_bias"
            | "tex.sample_compare"
            | "tex.sample_compare_level"
            | "tex.gather"
            | "tex.gather_compare"
            | "tex.feedback"
    )
}

fn supports_min_lod(opcode: &str) -> bool {
    supports_offset(opcode)
}

fn texture_type(ty: &Type) -> Option<(TextureDimension, &Type, TextureAccess, bool)> {
    if let Type::Texture {
        dimension,
        component,
        access,
        multisampled,
    } = ty
    {
        Some((*dimension, component, *access, *multisampled))
    } else {
        None
    }
}

fn verify_coordinates(
    instruction: &TextInstruction,
    coordinate: Option<&Type>,
    dimension: TextureDimension,
    normalized: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(coordinate) = coordinate else {
        return;
    };
    let expected_lanes = coordinate_lanes(dimension);
    let element_is_valid = |ty: &Type| {
        if normalized {
            matches!(
                ty,
                Type::Float(FloatType::F16 | FloatType::F32 | FloatType::F64)
            )
        } else {
            is_integer(ty)
        }
    };
    let valid = match coordinate {
        Type::FixedVector { lanes, element } => {
            *lanes == expected_lanes && element_is_valid(element)
        }
        scalar if expected_lanes == 1 => element_is_valid(scalar),
        _ => false,
    };
    if !valid {
        let family = if normalized {
            "floating-point"
        } else {
            "integer"
        };
        error(
            diagnostics,
            instruction,
            format!(
                "{} coordinates must contain {expected_lanes} {family} values",
                instruction.opcode
            ),
        );
    }
}

fn gradient_type(ty: &Type, dimension: TextureDimension) -> bool {
    let expected_lanes = coordinate_lanes(dimension);
    match ty {
        Type::Float(FloatType::F32) => expected_lanes == 1,
        Type::FixedVector { lanes, element } => {
            *lanes == expected_lanes && matches!(element.as_ref(), Type::Float(FloatType::F32))
        }
        _ => false,
    }
}

fn coordinate_lanes(dimension: TextureDimension) -> u32 {
    match dimension {
        TextureDimension::D1 | TextureDimension::D1Array => 1,
        TextureDimension::D2 | TextureDimension::D2Array => 2,
        TextureDimension::D3 | TextureDimension::Cube | TextureDimension::CubeArray => 3,
    }
}

fn offset_dimensions(dimension: TextureDimension) -> usize {
    match dimension {
        TextureDimension::D1 | TextureDimension::D1Array => 1,
        TextureDimension::D2
        | TextureDimension::D2Array
        | TextureDimension::Cube
        | TextureDimension::CubeArray => 2,
        TextureDimension::D3 => 3,
    }
}

fn is_array(dimension: TextureDimension) -> bool {
    matches!(
        dimension,
        TextureDimension::D1Array | TextureDimension::D2Array | TextureDimension::CubeArray
    )
}

fn query_type(instruction: &TextInstruction, dimension: TextureDimension) -> Option<Type> {
    let query = query_name(instruction)?;
    if matches!(
        query,
        "width" | "height" | "depth" | "layers" | "levels" | "samples"
    ) {
        return Some(Type::Integer(32));
    }
    if query != "size" {
        return None;
    }
    let lanes = match dimension {
        TextureDimension::D1 => return Some(Type::Integer(32)),
        TextureDimension::D1Array | TextureDimension::D2 | TextureDimension::Cube => 2,
        TextureDimension::D2Array | TextureDimension::D3 | TextureDimension::CubeArray => 3,
    };
    Some(vector(lanes, Type::Integer(32)))
}

fn gather_type(component: &Type) -> Type {
    match component {
        Type::FixedVector { element, .. } => vector(4, element.as_ref().clone()),
        _ => vector(4, component.clone()),
    }
}

fn is_integer(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_))
}

fn vector(lanes: u32, element: Type) -> Type {
    Type::FixedVector {
        lanes,
        element: Box::new(element),
    }
}

fn error(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(VFY3, message.into()).with_primary_span(instruction.span));
}
