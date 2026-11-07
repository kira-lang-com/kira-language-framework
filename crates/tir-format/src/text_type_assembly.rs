mod declarations;
mod lower;
mod members;
mod policies;
mod syntax;

use crate::types::{
    BinaryLayout, BinaryNominalDeclaration, BinaryNominalKind, BinaryType, BinaryTypeTable,
};
use crate::{TextItem, TextItemKind, TextModule, TextParseError};
use std::collections::HashMap;
use tir_diag::Span;
use tir_types::Type;

pub(crate) fn enum_tag_widths(module: &TextModule) -> Result<HashMap<String, u16>, TextParseError> {
    let mut widths = HashMap::new();
    for item in module
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Type)
    {
        let declaration = declarations::parse_declaration(module, *item)?;
        if declaration.declaration.kind != BinaryNominalKind::Enum
            || !declaration.declaration.generic_parameters.is_empty()
        {
            continue;
        }
        let count = syntax::split_ranges(
            &declaration.tokens,
            declaration.body_start + 1,
            declaration.body_end,
        )
        .into_iter()
        .filter(|(start, end)| {
            let (start, end) = syntax::trim_range(&declaration.tokens, *start, *end);
            start != end
        })
        .count();
        let width = match declaration.layout {
            BinaryLayout::C => 32,
            BinaryLayout::Ridl | BinaryLayout::Packed(_) if count <= 256 => 8,
            BinaryLayout::Ridl | BinaryLayout::Packed(_) if count <= 65_536 => 16,
            BinaryLayout::Ridl | BinaryLayout::Packed(_) => 32,
            BinaryLayout::Explicit { .. } => continue,
        };
        widths.insert(declaration.declaration.name, width);
    }
    Ok(widths)
}

pub(crate) fn trivial_struct_fields(
    module: &TextModule,
) -> Result<HashMap<String, Vec<Type>>, TextParseError> {
    trivial_nominal_fields(module, BinaryNominalKind::Struct)
}

pub(crate) fn trivial_union_fields(
    module: &TextModule,
) -> Result<HashMap<String, Vec<Type>>, TextParseError> {
    trivial_nominal_fields(module, BinaryNominalKind::Explicit)
}

fn trivial_nominal_fields(
    module: &TextModule,
    kind: BinaryNominalKind,
) -> Result<HashMap<String, Vec<Type>>, TextParseError> {
    let (types, _, _, _) = parse_text_type_table_with_roots(module, &[], &[], &[])?;
    let signatures = crate::types::BinarySignatureTable::default();
    let mut fields = HashMap::new();
    for definition in &types.definitions {
        let Some(declaration) = types.declarations.get(definition.declaration as usize) else {
            continue;
        };
        if declaration.kind != kind || !declaration.generic_parameters.is_empty() {
            continue;
        }
        let values = definition
            .fields
            .iter()
            .map(|field| {
                if !(kind == BinaryNominalKind::Explicit
                    && trivial_union_member(&types, field.ty, 0))
                    && !matches!(
                        types.entries.get(field.ty as usize),
                        Some(
                            BinaryType::Integer(_) | BinaryType::Float(_) | BinaryType::Pointer(_)
                        )
                    )
                {
                    return None;
                }
                crate::text_type_disassembly::format_type_reference(&types, field.ty, &signatures)
                    .ok()
                    .and_then(|text| crate::parse_type(&text).ok())
            })
            .collect::<Option<Vec<_>>>();
        if let Some(values) = values {
            fields.insert(declaration.name.clone(), values);
        }
    }
    Ok(fields)
}

fn trivial_union_member(types: &BinaryTypeTable, ty: u64, depth: usize) -> bool {
    if depth >= 64 {
        return false;
    }
    match types.entries.get(ty as usize) {
        Some(BinaryType::Integer(_) | BinaryType::Float(_) | BinaryType::Pointer(_)) => true,
        Some(BinaryType::Tuple(fields)) if !fields.is_empty() => fields
            .iter()
            .all(|field| trivial_union_member(types, *field, depth + 1)),
        Some(BinaryType::Array { count, element }) if *count > 0 => {
            trivial_union_member(types, *element, depth + 1)
        }
        _ => false,
    }
}

pub(crate) fn trivial_enum_payloads(
    module: &TextModule,
) -> Result<HashMap<String, Vec<(String, Type)>>, TextParseError> {
    let (types, _, _, _) = parse_text_type_table_with_roots(module, &[], &[], &[])?;
    let signatures = crate::types::BinarySignatureTable::default();
    let mut payloads = HashMap::new();
    for definition in &types.definitions {
        let Some(declaration) = types.declarations.get(definition.declaration as usize) else {
            continue;
        };
        if declaration.kind != BinaryNominalKind::Enum || !declaration.generic_parameters.is_empty()
        {
            continue;
        }
        let variants = definition
            .variants
            .iter()
            .map(|variant| {
                let fields = variant
                    .payload
                    .iter()
                    .map(|index| {
                        if !matches!(
                            types.entries.get(*index as usize),
                            Some(
                                BinaryType::Integer(_)
                                    | BinaryType::Float(_)
                                    | BinaryType::Pointer(_)
                            )
                        ) {
                            return None;
                        }
                        crate::text_type_disassembly::format_type_reference(
                            &types,
                            *index,
                            &signatures,
                        )
                        .ok()
                        .and_then(|text| crate::parse_type(&text).ok())
                    })
                    .collect::<Option<Vec<_>>>()?;
                let result = match fields.as_slice() {
                    [] => Type::Unit,
                    [one] => one.clone(),
                    _ => Type::Tuple(fields),
                };
                Some((variant.name.clone(), result))
            })
            .collect::<Option<Vec<_>>>();
        if let Some(variants) = variants {
            payloads.insert(declaration.name.clone(), variants);
        }
    }
    Ok(payloads)
}

pub(crate) fn parse_text_type_table_with_roots(
    module: &TextModule,
    leading_roots: &[(Type, TextItem)],
    middle_roots: &[(Type, TextItem)],
    trailing_roots: &[(Type, TextItem)],
) -> Result<(BinaryTypeTable, Vec<u64>, Vec<u64>, Vec<u64>), TextParseError> {
    let function_indices = lower::function_indices(module)?;
    let mut parsed = Vec::new();
    for item in module
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Type)
    {
        parsed.push(declarations::parse_declaration(module, *item)?);
    }
    let mut names = HashMap::new();
    for (index, declaration) in parsed.iter().enumerate() {
        if names
            .insert(declaration.declaration.name.clone(), index as u64)
            .is_some()
        {
            return Err(syntax::item_error(
                declaration.item,
                Span::new(0, 0),
                "duplicate nominal declaration",
            ));
        }
    }
    let declarations = parsed
        .iter()
        .map(|declaration| declaration.declaration.clone())
        .collect::<Vec<BinaryNominalDeclaration>>();
    let mut builder = lower::TypeTableBuilder::new(&declarations, &names, &function_indices);
    let leading_indices = leading_roots
        .iter()
        .map(|(ty, item)| builder.intern_external_type(ty, *item))
        .collect::<Result<Vec<_>, _>>()?;
    let mut definitions = Vec::new();
    for (index, declaration) in parsed.iter().enumerate() {
        definitions.push(members::parse_definition(
            module,
            declaration,
            index as u64,
            &mut builder,
        )?);
    }
    let middle_indices = middle_roots
        .iter()
        .map(|(ty, item)| builder.intern_external_type(ty, *item))
        .collect::<Result<Vec<_>, _>>()?;
    let trailing_indices = trailing_roots
        .iter()
        .map(|(ty, item)| builder.intern_external_type(ty, *item))
        .collect::<Result<Vec<_>, _>>()?;
    let entries = builder.into_entries();
    Ok((
        BinaryTypeTable {
            declarations,
            entries,
            definitions,
        },
        leading_indices,
        middle_indices,
        trailing_indices,
    ))
}
