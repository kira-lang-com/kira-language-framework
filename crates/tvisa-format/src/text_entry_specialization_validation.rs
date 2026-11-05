use tir_format::numeric_literals::{float_literal_is_exactly_representable, integer_literal_fits};
use tir_types::{FloatType, Type};

use crate::text::error;
use crate::text_entries::{TextEntryField, TextEntryValue, TextEntryValueKind};
use crate::text_entry_validation::check_member_names;
use crate::text_entry_value::{array, atom, member, object, unsigned};

pub(super) fn validate_specialization_constants(
    field: &TextEntryField,
) -> Result<(), crate::text::TextParseError> {
    let values = array(&field.value, "specialization_constants")?;
    let mut ids = Vec::new();
    for value in values {
        let fields = object(value, "specialization constant")?;
        check_member_names(
            fields,
            &["id", "type", "default"],
            value.span,
            "specialization constant",
        )?;
        let id = unsigned(member(fields, "id")?, "specialization constant id", 32)?;
        if ids.contains(&id) {
            return Err(error(
                member(fields, "id")?.span,
                "duplicate specialization constant id",
            ));
        }
        ids.push(id);
        let type_value = member(fields, "type")?;
        let TextEntryValueKind::Type(ty) = &type_value.kind else {
            return Err(error(type_value.span, "expected TVISA type"));
        };
        validate_default(ty, member(fields, "default")?)?;
    }
    Ok(())
}

fn validate_default(ty: &Type, value: &TextEntryValue) -> Result<(), crate::text::TextParseError> {
    match ty {
        Type::FixedVector { lanes, element } => {
            if !(2..=4).contains(lanes) || !is_gpu_numeric_scalar(element) {
                return Err(error(value.span, "unsupported specialization vector type"));
            }
            let values = array(value, "vector specialization default")?;
            if values.len() != *lanes as usize {
                return Err(error(
                    value.span,
                    "vector default lane count does not match its type",
                ));
            }
            for lane in values {
                validate_scalar_default(element, lane)?;
            }
            Ok(())
        }
        _ => validate_scalar_default(ty, value),
    }
}

fn validate_scalar_default(
    ty: &Type,
    value: &TextEntryValue,
) -> Result<(), crate::text::TextParseError> {
    let literal = atom(value)?;
    match ty {
        Type::Integer(1) if matches!(literal, "true" | "false") => Ok(()),
        Type::Integer(bits @ 2..=64) if integer_literal_fits(literal, *bits) => Ok(()),
        Type::Float(
            float @ (FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64),
        ) if float_literal_is_exactly_representable(literal, *float) => Ok(()),
        Type::Integer(1) => Err(error(
            value.span,
            "i1 specialization default must be true or false",
        )),
        Type::Integer(_) => Err(error(
            value.span,
            "integer specialization default is out of range",
        )),
        Type::Float(_) => Err(error(
            value.span,
            "floating-point specialization default is not exactly representable",
        )),
        _ => Err(error(
            value.span,
            "specialization default requires a scalar numeric type",
        )),
    }
}

fn is_gpu_numeric_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Integer(8 | 16 | 32 | 64))
        || matches!(
            ty,
            Type::Float(FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64)
        )
}
