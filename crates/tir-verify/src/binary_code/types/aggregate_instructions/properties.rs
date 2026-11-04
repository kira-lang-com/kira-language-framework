use std::collections::HashSet;

use tir_format::{
    BinaryCopyPolicy, BinaryLayout, BinaryNominalDefinition, BinaryNominalKind, BinaryType,
    BinaryTypeTable,
};

pub(super) fn nominal_definition<'a>(
    types: &'a BinaryTypeTable,
    ty: u64,
    expected_kind: BinaryNominalKind,
) -> Option<(&'a BinaryNominalDefinition, &'a [u64])> {
    let BinaryType::Nominal {
        declaration,
        arguments,
    } = type_entry(types, ty)?
    else {
        return None;
    };
    let declaration_id = *declaration;
    let declaration_record = types
        .declarations
        .get(usize::try_from(declaration_id).ok()?)?;
    if declaration_record.kind != expected_kind
        || declaration_record.generic_parameters.len() != arguments.len()
    {
        return None;
    }
    let definition = types
        .definitions
        .iter()
        .find(|definition| definition.declaration == declaration_id)?;
    Some((definition, arguments))
}

pub(super) fn substitute(types: &BinaryTypeTable, ty: u64, arguments: &[u64]) -> Option<u64> {
    super::super::address::substitute_type(types, ty, arguments)
}

pub(super) fn enum_tag_type(
    types: &BinaryTypeTable,
    definition: &BinaryNominalDefinition,
) -> Option<u64> {
    let width = match definition.layout {
        BinaryLayout::C => 32,
        BinaryLayout::Ridl | BinaryLayout::Packed(_) => match definition.variants.len() {
            0..=256 => 8,
            257..=65_536 => 16,
            _ => 32,
        },
        BinaryLayout::Explicit { .. } => return None,
    };
    types
        .entries
        .iter()
        .position(|ty| matches!(ty, BinaryType::Integer(bits) if *bits == width))
        .and_then(|index| u64::try_from(index).ok())
}

pub(super) fn is_trivial(types: &BinaryTypeTable, ty: u64, depth: usize) -> bool {
    if depth >= 128 {
        return false;
    }
    match type_entry(types, ty) {
        Some(
            BinaryType::Integer(_)
            | BinaryType::Float(_)
            | BinaryType::Pointer(_)
            | BinaryType::Unit
            | BinaryType::Sampler
            | BinaryType::AccelerationStructure
            | BinaryType::Texture { .. }
            | BinaryType::VisibleFunctionTable { .. }
            | BinaryType::Tensor { .. }
            | BinaryType::RayQuery
            | BinaryType::HitObject
            | BinaryType::CooperativeMatrix { .. }
            | BinaryType::Entry { .. },
        ) => true,
        Some(BinaryType::FixedVector { element, .. }) => is_trivial(types, *element, depth + 1),
        Some(BinaryType::Tuple(fields)) => fields
            .iter()
            .all(|field| is_trivial(types, *field, depth + 1)),
        Some(BinaryType::Array { element, .. }) => is_trivial(types, *element, depth + 1),
        Some(BinaryType::Nominal { declaration, .. }) => types
            .definitions
            .iter()
            .find(|definition| definition.declaration == *declaration)
            .is_some_and(|definition| {
                matches!(
                    definition.policy.copy,
                    tir_format::BinaryCopyPolicy::Trivial
                ) && matches!(
                    definition.policy.destroy,
                    tir_format::BinaryDestroyPolicy::Trivial
                )
            }),
        _ => false,
    }
}

pub(super) fn is_loadable(
    types: &BinaryTypeTable,
    ty: u64,
    visiting: &mut HashSet<u64>,
    depth: usize,
) -> bool {
    if depth >= 128 || !visiting.insert(ty) {
        return false;
    }
    let loadable = match type_entry(types, ty) {
        Some(
            BinaryType::Generic { .. }
            | BinaryType::ScalableVector { .. }
            | BinaryType::Frame { .. }
            | BinaryType::Address { .. },
        ) => false,
        Some(BinaryType::FixedVector { element, .. }) | Some(BinaryType::Array { element, .. }) => {
            is_loadable(types, *element, visiting, depth + 1)
        }
        Some(BinaryType::Tuple(fields)) => fields
            .iter()
            .all(|field| is_loadable(types, *field, visiting, depth + 1)),
        Some(BinaryType::Nominal {
            declaration,
            arguments,
        }) => {
            let declaration_record = usize::try_from(*declaration)
                .ok()
                .and_then(|index| types.declarations.get(index));
            let definition = types
                .definitions
                .iter()
                .find(|definition| definition.declaration == *declaration);
            declaration_record
                .is_some_and(|record| record.generic_parameters.len() == arguments.len())
                && definition.is_some_and(|definition| {
                    matches!(
                        definition.policy.relocate,
                        tir_format::BinaryRelocatePolicy::Bitwise
                    ) && definition.fields.iter().all(|field| {
                        substitute(types, field.ty, arguments)
                            .is_some_and(|field| is_loadable(types, field, visiting, depth + 1))
                    }) && definition.variants.iter().all(|variant| {
                        variant.payload.iter().all(|payload| {
                            substitute(types, *payload, arguments).is_some_and(|payload| {
                                is_loadable(types, payload, visiting, depth + 1)
                            })
                        })
                    }) && arguments
                        .iter()
                        .all(|argument| is_loadable(types, *argument, visiting, depth + 1))
                })
        }
        Some(
            BinaryType::Function(_)
            | BinaryType::Reference { .. }
            | BinaryType::Unique { .. }
            | BinaryType::Weak { .. }
            | BinaryType::Closure { .. }
            | BinaryType::Coroutine { .. }
            | BinaryType::Texture { .. }
            | BinaryType::Sampler
            | BinaryType::AccelerationStructure
            | BinaryType::VisibleFunctionTable { .. }
            | BinaryType::Tensor { .. }
            | BinaryType::RayQuery
            | BinaryType::HitObject
            | BinaryType::CooperativeMatrix { .. }
            | BinaryType::Entry { .. }
            | BinaryType::Integer(_)
            | BinaryType::Float(_)
            | BinaryType::Pointer(_)
            | BinaryType::Unit
            | BinaryType::Never,
        ) => true,
        None => false,
    };
    visiting.remove(&ty);
    loadable
}

fn type_entry(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}

pub(super) fn is_copyable(types: &BinaryTypeTable, ty: u64, depth: usize) -> bool {
    if depth >= 128 {
        return false;
    }
    if is_trivial(types, ty, 0) {
        return true;
    }
    match type_entry(types, ty) {
        Some(BinaryType::Reference { .. } | BinaryType::Weak { .. }) => true,
        Some(
            BinaryType::Unique { .. }
            | BinaryType::Closure { .. }
            | BinaryType::Coroutine { .. }
            | BinaryType::Generic { .. }
            | BinaryType::Frame { .. },
        )
        | None => false,
        Some(BinaryType::Tuple(fields)) => fields
            .iter()
            .all(|field| is_copyable(types, *field, depth + 1)),
        Some(BinaryType::Array { element, .. }) => is_copyable(types, *element, depth + 1),
        Some(BinaryType::Nominal {
            declaration,
            arguments,
        }) => {
            let declaration_record = usize::try_from(*declaration)
                .ok()
                .and_then(|index| types.declarations.get(index));
            let definition = types
                .definitions
                .iter()
                .find(|definition| definition.declaration == *declaration);
            declaration_record
                .is_some_and(|record| record.generic_parameters.len() == arguments.len())
                && definition.is_some_and(|definition| {
                    matches!(
                        definition.policy.copy,
                        BinaryCopyPolicy::Trivial
                            | BinaryCopyPolicy::Implicit(_)
                            | BinaryCopyPolicy::Explicit(_)
                    )
                })
        }
        Some(_) => true,
    }
}
