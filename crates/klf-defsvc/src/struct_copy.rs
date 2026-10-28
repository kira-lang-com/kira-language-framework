//! Whether a struct can be copied field by field, and the flat plan that does
//! it. A copy is only expressible when every leaf of the struct is a scalar,
//! so the plan is a list of paths down to those leaves.

use klf_kir::{StoreMode, Type};

#[derive(Clone, Eq, PartialEq)]
pub(crate) enum CopyStep {
    Field(u32),
    Elem(u64),
    EnumPayload(u32),
    Value,
}

use crate::type_table::TypeTable;

pub(crate) fn copyable_struct(
    types: &TypeTable,
    module: u64,
    declaration: &klf_kir::TypeDeclaration,
) -> bool {
    let mut visited = Vec::new();
    shape_struct(types, module, declaration, &mut visited)
}

pub(crate) fn struct_copy_plan_with_variants(
    types: &TypeTable,
    module: u64,
    declaration: &klf_kir::TypeDeclaration,
    variants: &[(Vec<CopyStep>, u32)],
) -> Option<Vec<Vec<CopyStep>>> {
    let mut leaves = Vec::new();
    plan_struct(
        types,
        module,
        declaration,
        &mut Vec::new(),
        &mut Vec::new(),
        &mut leaves,
        variants,
    )
    .then_some(leaves)
}

fn shape_struct(
    types: &TypeTable,
    module: u64,
    declaration: &klf_kir::TypeDeclaration,
    visited: &mut Vec<usize>,
) -> bool {
    if !declaration.variants.is_empty() {
        return false;
    }
    declaration
        .fields
        .iter()
        .all(|field| shape_type(types, module, &field.ty, visited))
}

fn shape_type(types: &TypeTable, module: u64, ty: &Type, visited: &mut Vec<usize>) -> bool {
    match ty {
        Type::Integer(32 | 64) => true,
        Type::Array(_, element) => matches!(element.as_ref(), Type::Integer(32 | 64)),
        Type::Nominal(name, arguments) if arguments.is_empty() => {
            let Some(index) = types.find(module, name.as_str()) else {
                return false;
            };
            let declaration = &types[index].1;
            if visited.contains(&index) {
                return false;
            }
            visited.push(index);
            let valid = if declaration.variants.is_empty() {
                shape_struct(types, module, declaration, visited)
            } else {
                declaration.variants.iter().all(|variant| {
                    variant.payload.len() <= 1
                        && variant
                            .payload
                            .first()
                            .is_none_or(|payload| shape_type(types, module, payload, visited))
                })
            };
            visited.pop();
            valid
        }
        _ => false,
    }
}

fn plan_struct(
    types: &TypeTable,
    module: u64,
    declaration: &klf_kir::TypeDeclaration,
    prefix: &mut Vec<CopyStep>,
    visited: &mut Vec<usize>,
    leaves: &mut Vec<Vec<CopyStep>>,
    variants: &[(Vec<CopyStep>, u32)],
) -> bool {
    if !declaration.variants.is_empty() {
        return false;
    }
    for (index, field) in declaration.fields.iter().enumerate() {
        let Ok(field_id) = u32::try_from(index) else {
            return false;
        };
        prefix.push(CopyStep::Field(field_id));
        let ok = plan_type(types, module, &field.ty, prefix, visited, leaves, variants);
        prefix.pop();
        if !ok {
            return false;
        }
    }
    true
}

fn plan_type(
    types: &TypeTable,
    module: u64,
    ty: &Type,
    prefix: &mut Vec<CopyStep>,
    visited: &mut Vec<usize>,
    leaves: &mut Vec<Vec<CopyStep>>,
    variants: &[(Vec<CopyStep>, u32)],
) -> bool {
    match ty {
        Type::Integer(32 | 64) => {
            prefix.push(CopyStep::Value);
            leaves.push(prefix.clone());
            prefix.pop();
            true
        }
        Type::Array(length, element) if matches!(element.as_ref(), Type::Integer(32 | 64)) => {
            for index in 0..*length {
                prefix.push(CopyStep::Elem(index));
                prefix.push(CopyStep::Value);
                leaves.push(prefix.clone());
                prefix.pop();
                prefix.pop();
            }
            true
        }
        Type::Nominal(name, arguments) if arguments.is_empty() => {
            let Some(index) = types.find(module, name.as_str()) else {
                return false;
            };
            let declaration = &types[index].1;
            if visited.contains(&index) {
                return false;
            }
            visited.push(index);
            let ok = if declaration.variants.is_empty() {
                plan_struct(
                    types,
                    module,
                    declaration,
                    prefix,
                    visited,
                    leaves,
                    variants,
                )
            } else {
                let Some((_, variant)) = variants.iter().find(|(path, _)| path == prefix) else {
                    visited.pop();
                    return false;
                };
                let Some(variant_declaration) = declaration.variants.get(*variant as usize) else {
                    visited.pop();
                    return false;
                };
                prefix.push(CopyStep::EnumPayload(*variant));
                let before = leaves.len();
                let ok = match variant_declaration.payload.as_slice() {
                    [] => {
                        leaves.push(prefix.clone());
                        true
                    }
                    [payload] => {
                        plan_type(types, module, payload, prefix, visited, leaves, variants)
                    }
                    _ => false,
                };
                if ok && leaves.len() == before {
                    leaves.push(prefix.clone());
                }
                prefix.pop();
                ok
            };
            visited.pop();
            ok
        }
        _ => false,
    }
}

pub(crate) fn copy_struct_fields(
    builder: &mut klf_services::FunctionBuilder,
    leaves: &[Vec<CopyStep>],
    source: &klf_core::Name,
    destination: &klf_core::Name,
) {
    let mut injected: Vec<Vec<CopyStep>> = Vec::new();
    for leaf in leaves {
        let mut source_address = source.clone();
        let mut destination_address = destination.clone();
        let mut address_path = Vec::new();
        let mut copy_value = false;
        for step in leaf {
            match step {
                CopyStep::Field(field) => {
                    source_address = builder.field_addr(source_address, *field);
                    destination_address = builder.field_addr(destination_address, *field);
                    address_path.push(CopyStep::Field(*field));
                }
                CopyStep::Elem(index) => {
                    let index_name = builder.const_int(Type::Integer(64), i128::from(*index));
                    source_address = builder.elem_addr(source_address, index_name.clone());
                    destination_address = builder.elem_addr(destination_address, index_name);
                    address_path.push(CopyStep::Elem(*index));
                }
                CopyStep::EnumPayload(variant) => {
                    let mut injection_path = address_path.clone();
                    injection_path.push(CopyStep::EnumPayload(*variant));
                    if !injected.contains(&injection_path) {
                        builder.enum_inject(destination_address.clone(), *variant);
                        injected.push(injection_path);
                    }
                    source_address = builder.enum_payload_addr(source_address, *variant);
                    destination_address = builder.enum_payload_addr(destination_address, *variant);
                    address_path.push(CopyStep::EnumPayload(*variant));
                }
                CopyStep::Value => copy_value = true,
            }
        }
        if copy_value {
            let value = builder.load(source_address);
            builder.store(StoreMode::Init, value, destination_address);
        }
    }
}

pub(crate) fn field_index(value: Option<&u64>) -> Option<usize> {
    value
        .copied()
        .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
        .and_then(|value| usize::try_from(value).ok())
}
