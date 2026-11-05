use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_types::{AddressSpace, Type};

use crate::Instruction;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const V19: Code = Code::new(Dialect::Tir, Area::Verifier, 19);

#[derive(Clone, Copy, Eq, PartialEq)]
enum Shape {
    Scalar,
    Fixed(u32),
    Scalable,
}

pub(super) fn verify_instruction(instruction: &Instruction) -> Option<Vec<Diagnostic>> {
    let opcode = instruction.opcode.as_str();
    let mut diagnostics = Vec::new();
    let Some(rule) = conversion_rule(opcode) else {
        return None;
    };
    if instruction.operands.len() != 1 || instruction.results.len() != 1 {
        diagnostics.push(
            Diagnostic::error(
                V2,
                format!(
                    "{opcode} has {} operands and {} results; expected one of each",
                    instruction.operands.len(),
                    instruction.results.len()
                ),
            )
            .with_primary_span(instruction.span),
        );
        return Some(diagnostics);
    }

    let source = &instruction.operands[0].ty;
    let destination = &instruction.results[0].ty;
    let valid = match rule {
        Conversion::IntegerNarrow => integer_conversion(source, destination, |a, b| a > b),
        Conversion::IntegerWiden => integer_conversion(source, destination, |a, b| a < b),
        Conversion::FloatNarrow => float_conversion(source, destination, |a, b| a > b),
        Conversion::FloatWiden => float_conversion(source, destination, |a, b| a < b),
        Conversion::IntegerToFloat => {
            same_shape(source, destination)
                && is_integer_element(source)
                && is_float_element(destination)
        }
        Conversion::FloatToInteger => {
            same_shape(source, destination)
                && is_float_element(source)
                && is_integer_element(destination)
        }
        Conversion::FloatConvert => {
            same_shape(source, destination)
                && is_float_element(source)
                && is_float_element(destination)
        }
        Conversion::PointerToInteger => {
            matches!(source, Type::Pointer(_)) && matches!(destination, Type::Integer(64))
        }
        Conversion::IntegerToPointer => {
            matches!(source, Type::Integer(64))
                && matches!(
                    destination,
                    Type::Pointer(
                        AddressSpace::Generic | AddressSpace::Device | AddressSpace::Constant
                    )
                )
        }
        Conversion::Bitcast => bitcast_valid(source, destination).unwrap_or(true),
    };
    if !valid {
        let code = if matches!(rule, Conversion::Bitcast)
            && function_bitcast_valid(source, destination).is_some_and(|valid| !valid)
        {
            V19
        } else {
            V2
        };
        diagnostics.push(
            Diagnostic::error(
                code,
                format!("{opcode} cannot convert {} to {}", source, destination),
            )
            .with_primary_span(instruction.span),
        );
    }
    Some(diagnostics)
}

#[derive(Clone, Copy)]
enum Conversion {
    IntegerNarrow,
    IntegerWiden,
    FloatNarrow,
    FloatWiden,
    IntegerToFloat,
    FloatToInteger,
    FloatConvert,
    PointerToInteger,
    IntegerToPointer,
    Bitcast,
}

fn conversion_rule(opcode: &str) -> Option<Conversion> {
    Some(match opcode {
        "trunc" => Conversion::IntegerNarrow,
        "zext" | "sext" => Conversion::IntegerWiden,
        "fptrunc" => Conversion::FloatNarrow,
        "fpext" => Conversion::FloatWiden,
        "sitofp" | "uitofp" => Conversion::IntegerToFloat,
        "fptosi" | "fptoui" => Conversion::FloatToInteger,
        "fcvt" => Conversion::FloatConvert,
        "ptrtoint" => Conversion::PointerToInteger,
        "inttoptr" => Conversion::IntegerToPointer,
        "bitcast" => Conversion::Bitcast,
        _ => return None,
    })
}

fn integer_conversion(source: &Type, destination: &Type, compare: fn(u16, u16) -> bool) -> bool {
    same_shape(source, destination)
        && integer_width(source)
            .zip(integer_width(destination))
            .is_some_and(|(from, to)| compare(from, to))
}

fn float_conversion(source: &Type, destination: &Type, compare: fn(u16, u16) -> bool) -> bool {
    same_shape(source, destination)
        && float_width(source)
            .zip(float_width(destination))
            .is_some_and(|(from, to)| compare(from, to))
}

fn integer_width(ty: &Type) -> Option<u16> {
    match element_type(ty) {
        Type::Integer(width) => Some(*width),
        _ => None,
    }
}

fn float_width(ty: &Type) -> Option<u16> {
    match element_type(ty) {
        Type::Float(float) => Some(float.bits()),
        _ => None,
    }
}

fn is_integer_element(ty: &Type) -> bool {
    integer_width(ty).is_some()
}

fn is_float_element(ty: &Type) -> bool {
    float_width(ty).is_some()
}

fn same_shape(left: &Type, right: &Type) -> bool {
    shape(left) == shape(right)
}

fn shape(ty: &Type) -> Shape {
    match ty {
        Type::FixedVector { lanes, .. } => Shape::Fixed(*lanes),
        Type::ScalableVector(_) => Shape::Scalable,
        _ => Shape::Scalar,
    }
}

fn element_type(ty: &Type) -> &Type {
    match ty {
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => element,
        _ => ty,
    }
}

fn bitcast_valid(source: &Type, destination: &Type) -> Option<bool> {
    if let Some(valid) = function_bitcast_valid(source, destination) {
        return Some(valid);
    }
    if matches!(source, Type::Function(_) | Type::Closure(_))
        || matches!(destination, Type::Function(_) | Type::Closure(_))
    {
        return Some(false);
    }
    match (bit_size(source), bit_size(destination)) {
        (Some(source), Some(destination)) => Some(source == destination),
        _ => None,
    }
}

fn function_bitcast_valid(source: &Type, destination: &Type) -> Option<bool> {
    match (source, destination) {
        (Type::Function(left), Type::Function(right)) => Some(left == right),
        (Type::Function(function), Type::Pointer(AddressSpace::Generic))
        | (Type::Pointer(AddressSpace::Generic), Type::Function(function)) => {
            Some(function.foreign)
        }
        (Type::Function(_), Type::Pointer(_)) | (Type::Pointer(_), Type::Function(_)) => {
            Some(false)
        }
        _ => None,
    }
}

fn bit_size(ty: &Type) -> Option<u64> {
    match ty {
        Type::Integer(bits) => Some(u64::from(*bits)),
        Type::Float(float) => Some(u64::from(float.bits())),
        Type::Pointer(_) => Some(64),
        Type::FixedVector { lanes, element } => u64::from(*lanes).checked_mul(bit_size(element)?),
        _ => None,
    }
}
