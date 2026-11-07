//! Canonical text formatting for nongeneric struct and enum CODE.
use std::fmt::Write as _;

use crate::FormatError;
use crate::text_nominal_construction::{definition, enum_tag_width, malformed, trivial};
use crate::types::{
    BinaryInstruction, BinaryNominalKind, BinaryOperand, BinaryType, BinaryTypeTable,
};

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed(
            "nominal construction/insertion requires one result, zero modifiers",
        ));
    }
    let get = |index: u64| {
        values
            .get(index as usize)
            .map(|(name, ty)| (name.as_str(), *ty))
            .ok_or_else(|| malformed("nominal SSA operand index invalid"))
    };
    let (code, result_ty) = match instruction.opcode {
        0x100 | 0x10a => {
            let (ty, variant, args) = match instruction.operands.as_slice() {
                [BinaryOperand::Type(ty), BinaryOperand::Values(args)]
                    if instruction.opcode == 0x100 =>
                {
                    (*ty, None, args)
                }
                [
                    BinaryOperand::Type(ty),
                    BinaryOperand::Index(variant),
                    BinaryOperand::Values(args),
                ] if instruction.opcode == 0x10a => (*ty, Some(*variant), args),
                _ => return Err(malformed("nominal constructor has wrong operand kinds")),
            };
            let kind = if variant.is_none() {
                BinaryNominalKind::Struct
            } else {
                BinaryNominalKind::Enum
            };
            let declaration = definition(types, ty, kind)?;
            let expected = if let Some(variant) = variant {
                declaration
                    .variants
                    .get(variant as usize)
                    .ok_or_else(|| malformed("enum.new variant index out of range"))?
                    .payload
                    .clone()
            } else {
                declaration.fields.iter().map(|field| field.ty).collect()
            };
            if expected.len() != args.len() || expected.iter().any(|ty| !trivial(types, *ty)) {
                return Err(malformed(
                    "nominal constructor payload shape is unsupported",
                ));
            }
            let mut names = Vec::new();
            for (id, expected) in args.iter().zip(&expected) {
                let (name, actual) = get(*id)?;
                if actual != *expected {
                    return Err(malformed("nominal constructor operand types disagree"));
                }
                names.push(format!("%{name}"));
            }
            let ty_name = crate::text_type_disassembly::format_type_reference(
                types,
                ty,
                &crate::types::BinarySignatureTable::default(),
            )?;
            let code = if let Some(variant) = variant {
                let variant = &declaration.variants[variant as usize];
                format!("enum.new {ty_name}, {}({})", variant.name, names.join(", "))
            } else {
                format!("struct.new {ty_name}({})", names.join(", "))
            };
            (code, ty)
        }
        0x101 => {
            let [BinaryOperand::Value(source), BinaryOperand::Index(field)] =
                instruction.operands.as_slice()
            else {
                return Err(malformed(
                    "struct.extract requires source value and field index",
                ));
            };
            let (source_name, ty) = get(*source)?;
            let declaration = definition(types, ty, BinaryNominalKind::Struct)?;
            let field_type = declaration
                .fields
                .get(*field as usize)
                .ok_or_else(|| malformed("struct.extract field index is out of range"))?
                .ty;
            if !trivial(types, field_type) {
                return Err(malformed("struct.extract requires a trivial field type"));
            }
            (
                format!("struct.extract %{source_name}, {field}"),
                field_type,
            )
        }
        0x102 => {
            let [
                BinaryOperand::Value(source),
                BinaryOperand::Index(field),
                BinaryOperand::Value(value),
            ] = instruction.operands.as_slice()
            else {
                return Err(malformed("struct.insert needs value, field index, value"));
            };
            let (source_name, ty) = get(*source)?;
            let declaration = definition(types, ty, BinaryNominalKind::Struct)?;
            let field_ty = declaration
                .fields
                .get(*field as usize)
                .ok_or_else(|| malformed("struct.insert field index out of range"))?
                .ty;
            if !trivial(types, field_ty)
                || declaration
                    .fields
                    .iter()
                    .any(|field| !trivial(types, field.ty))
            {
                return Err(malformed("struct.insert requires trivial nominal struct"));
            }
            let (value_name, actual) = get(*value)?;
            if actual != field_ty {
                return Err(malformed("struct.insert replacement type is incompatible"));
            }
            (
                format!("struct.insert %{source_name}, {field}, %{value_name}"),
                ty,
            )
        }
        0x10b => {
            let [BinaryOperand::Value(source)] = instruction.operands.as_slice() else {
                return Err(malformed("enum.tag requires exactly one SSA operand"));
            };
            let (source_name, ty) = get(*source)?;
            let definition = definition(types, ty, BinaryNominalKind::Enum)?;
            let width = enum_tag_width(definition)?;
            let tag = types
                .entries
                .iter()
                .position(|ty| matches!(ty, BinaryType::Integer(bits) if *bits == width))
                .ok_or_else(|| malformed("enum.tag result type is missing from TYPES"))?;
            (format!("enum.tag %{source_name}"), tag as u64)
        }
        _ => return Err(malformed("unsupported nominal constructor opcode")),
    };
    writeln!(text, "    %v{} = {code}", values.len())
        .map_err(|issue| malformed(issue.to_string()))?;
    values.push((format!("v{}", values.len()), result_ty));
    Ok(())
}
