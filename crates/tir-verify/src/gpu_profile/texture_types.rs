use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind};
use tir_types::{FloatType, TextureAccess, TextureDimension, Type};

use crate::ssa::text_instruction_operands_start;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(instruction: &TextInstruction, operands: &[Type]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let opcode = instruction.opcode.as_str();
    if !is_texture_opcode(opcode) {
        return diagnostics;
    }
    let Some((dimension, component, access, multisampled)) =
        operands.first().and_then(texture_type)
    else {
        if operands.first().is_some() {
            error(
                &mut diagnostics,
                instruction,
                "texture instruction requires a texture handle",
            );
        }
        return diagnostics;
    };
    if is_sampling(opcode) {
        if operands.get(1).is_some_and(|ty| ty != &Type::Sampler) {
            error(
                &mut diagnostics,
                instruction,
                "texture sampling requires a sampler handle",
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
        if is_array(dimension) && operands.get(3).is_some_and(|ty| !is_integer(ty)) {
            error(
                &mut diagnostics,
                instruction,
                "texture array layer must be an integer",
            );
        }
    } else if matches!(opcode, "tex.read" | "tex.write" | "tex.atomic") {
        verify_coordinates(
            instruction,
            operands.get(1),
            dimension,
            false,
            &mut diagnostics,
        );
        if is_array(dimension) && operands.get(2).is_some_and(|ty| !is_integer(ty)) {
            error(
                &mut diagnostics,
                instruction,
                "texture array layer must be an integer",
            );
        }
        let allowed = match opcode {
            "tex.read" => matches!(access, TextureAccess::Read | TextureAccess::ReadWrite),
            "tex.write" => matches!(access, TextureAccess::Write | TextureAccess::ReadWrite),
            _ => access == TextureAccess::ReadWrite,
        };
        if !allowed {
            error(
                &mut diagnostics,
                instruction,
                "texture access mode does not permit this operation",
            );
        }
        if opcode == "tex.write" {
            let value_index = if is_array(dimension) { 3 } else { 2 };
            if operands.get(value_index).is_some_and(|ty| ty != component) {
                error(
                    &mut diagnostics,
                    instruction,
                    format!("tex.write value must have type {component}"),
                );
            }
        }
        if opcode == "tex.atomic" && multisampled {
            error(
                &mut diagnostics,
                instruction,
                "tex.atomic does not support multisampled textures",
            );
        }
    } else if matches!(opcode, "tex.query" | "tex.lod") {
        if opcode == "tex.lod" {
            verify_coordinates(
                instruction,
                operands.get(1),
                dimension,
                true,
                &mut diagnostics,
            );
        } else if query_type(instruction, dimension).is_none() {
            error(
                &mut diagnostics,
                instruction,
                "tex.query has an unsupported query field",
            );
        }
    }
    diagnostics
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
        "tex.lod" => Some(vector(2, Type::Float(FloatType::F32))),
        "tex.gather" => Some(gather_type(component)),
        "tex.gather_compare" => Some(vector(4, Type::Float(FloatType::F32))),
        "tex.query" => query_type(instruction, dimension),
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
    if matches!(opcode, "tex.write") {
        return Some(0);
    }
    if matches!(
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
            | "tex.atomic"
            | "tex.query"
            | "tex.lod"
    ) {
        return Some(1 + usize::from(supports_sparse(opcode) && has_sparse_modifier(instruction)));
    }
    None
}

fn is_texture_opcode(opcode: &str) -> bool {
    opcode.starts_with("tex.")
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

fn coordinate_lanes(dimension: TextureDimension) -> u32 {
    match dimension {
        TextureDimension::D1 | TextureDimension::D1Array => 1,
        TextureDimension::D2 | TextureDimension::D2Array => 2,
        TextureDimension::D3 | TextureDimension::Cube | TextureDimension::CubeArray => 3,
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

fn query_name(instruction: &TextInstruction) -> Option<&str> {
    let start = text_instruction_operands_start(&instruction.tokens);
    let comma = instruction.tokens[start..]
        .iter()
        .position(|token| is_punctuation(token, ','))?
        + start;
    atom(instruction.tokens.get(comma + 1)?)
}

fn gather_type(component: &Type) -> Type {
    match component {
        Type::FixedVector { element, .. } => vector(4, element.as_ref().clone()),
        _ => vector(4, component.clone()),
    }
}

fn has_sparse_modifier(instruction: &TextInstruction) -> bool {
    instruction
        .tokens
        .iter()
        .any(|token| matches!(&token.kind, TokenKind::Atom(atom) if atom == "sparse"))
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

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn error(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(V2, message.into()).with_primary_span(instruction.span));
}
