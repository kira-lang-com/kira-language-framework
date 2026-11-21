//! Type lookups the value-type inference reads: a function value's type and
//! the members, fields, tags and payloads of aggregates.

use super::ModuleIndex;
use crate::{Function, FunctionAbi, FunctionType, Linkage, Operation, ParameterType, Type};
use klf_core::Name;
use rustc_hash::FxHashMap as HashMap;

/// The type of a function value naming `function`.
pub(super) fn function_type(function: &Function) -> Type {
    let abi = if function.foreign && function.abi != FunctionAbi::Ridl {
        FunctionAbi::Platform
    } else if function.abi == FunctionAbi::Ridl
        || matches!(function.linkage, Linkage::Exported)
        || function.failure.is_some()
        || function
            .parameters
            .iter()
            .any(|parameter| parameter.context)
    {
        FunctionAbi::Ridl
    } else {
        FunctionAbi::Platform
    };
    Type::Function(FunctionType {
        parameters: function
            .parameters
            .iter()
            .map(|parameter| ParameterType {
                convention: parameter.convention,
                context: parameter.context,
                ty: parameter.ty.clone(),
            })
            .collect(),
        result: Box::new(function.result.clone()),
        failure: function.failure.clone().map(Box::new),
        abi,
    })
}

/// The pointee of `value` wrapped back up in another pointer-like type.
pub(super) fn pointee_wrapped(
    value: &Name,
    values: &HashMap<Name, Type>,
    wrap: impl FnOnce(Type) -> Type,
) -> Option<Type> {
    values
        .get(value)
        .and_then(pointee)
        .map(|inner| wrap(inner.clone()))
}

/// The member types the destructuring operations yield, in order.
pub(super) fn member_types(
    operation: &Operation,
    values: &HashMap<Name, Type>,
    index: &ModuleIndex<'_>,
) -> Option<Vec<Type>> {
    match operation {
        Operation::StructDestructure(value) => match values.get(value)? {
            Type::Nominal(name, _) => Some(
                index
                    .types
                    .get(name.as_str())?
                    .fields
                    .iter()
                    .map(|field| field.ty.clone())
                    .collect(),
            ),
            _ => None,
        },
        Operation::TupleDestructure(value) => match values.get(value)? {
            Type::Tuple(elements) => Some(elements.clone()),
            _ => None,
        },
        Operation::EnumTake { value, variant } | Operation::EnumProject { value, variant } => {
            let ty = values.get(value)?;
            let Type::Nominal(name, _) = pointee(ty).unwrap_or(ty) else {
                return None;
            };
            let declaration = index.types.get(name.as_str())?;
            Some(declaration.variants.get(*variant as usize)?.payload.clone())
        }
        _ => None,
    }
}

pub(super) fn struct_field(
    aggregate: &Name,
    field: u32,
    values: &HashMap<Name, Type>,
    index: &ModuleIndex<'_>,
) -> Option<Type> {
    let Some(Type::Nominal(name, _)) = values.get(aggregate) else {
        return None;
    };
    let declaration = index.types.get(name.as_str())?;
    declaration
        .fields
        .get(field as usize)
        .map(|field| field.ty.clone())
}

pub(super) fn enum_tag_type(
    value: &Name,
    values: &HashMap<Name, Type>,
    index: &ModuleIndex<'_>,
) -> Option<Type> {
    let Type::Nominal(name, _) = values.get(value).and_then(pointee)? else {
        return None;
    };
    let declaration = index.types.get(name.as_str())?;
    if declaration.variants.is_empty() {
        return None;
    }
    Some(Type::Integer(if declaration.open {
        32
    } else if declaration.variants.len() <= 256 {
        8
    } else if declaration.variants.len() <= 65_536 {
        16
    } else {
        32
    }))
}

pub(super) fn enum_payload_type(
    base: &Name,
    variant: u32,
    values: &HashMap<Name, Type>,
    index: &ModuleIndex<'_>,
) -> Option<Type> {
    let Type::Nominal(name, _) = values.get(base).and_then(pointee)? else {
        return None;
    };
    let declaration = index.types.get(name.as_str())?;
    let payload = declaration
        .variants
        .get(variant as usize)?
        .payload
        .as_slice();
    let payload = match payload {
        [] => Type::Unit,
        [one] => one.clone(),
        many => Type::Tuple(many.to_vec()),
    };
    Some(Type::Address(Box::new(payload)))
}

pub(super) fn pointee(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Address(inner) | Type::Reference(inner) | Type::Unique(inner) | Type::Weak(inner) => {
            Some(inner)
        }
        _ => None,
    }
}
