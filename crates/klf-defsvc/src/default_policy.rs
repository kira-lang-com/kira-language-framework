//! Default lifecycle policies for the types a definition builds (section 11.2).
//!
//! Unmanaged aggregates are trivial. Counted and Affine aggregates are
//! memberwise: when every member is trivial the type is too; otherwise the
//! kernel synthesizes `destroy(obj: ptr)` and, when every member can be
//! copied, `copy(dst: ptr, src: ptr)` over the members (see `lifecycle`).

use crate::lifecycle::{self, Member};
use klf_kir::{
    CopyPolicy, DestroyPolicy, Exclusivity, Function, Pairing, Policy, RelocatePolicy, Type,
    TypeDeclaration,
};
use klf_services::MemoryModel;
use std::collections::HashMap;

/// What a declaration's members need at copy and destroy.
#[derive(Clone, Copy)]
pub(crate) struct Lifecycle {
    pub trivial: bool,
    pub copyable: bool,
}

/// Derives every declaration's policy and returns the functions the
/// memberwise policies name.
pub(crate) fn derive_all(
    types: &[(u64, TypeDeclaration)],
    module: u64,
    memory: MemoryModel,
    exclusivity: Exclusivity,
) -> Result<(Vec<Policy>, Vec<Function>), String> {
    let mut named: HashMap<&klf_core::Name, Vec<usize>> = HashMap::new();
    for (index, (owner, declaration)) in types.iter().enumerate() {
        if *owner == module {
            named.entry(&declaration.name).or_default().push(index);
        }
    }
    let mut deriver = Deriver {
        types,
        named,
        lifecycles: vec![None; types.len()],
        visiting: Vec::new(),
    };
    let type_names: Vec<klf_core::Name> = types
        .iter()
        .map(|(_, declaration)| declaration.name.clone())
        .collect();
    let declarations: Vec<&TypeDeclaration> =
        types.iter().map(|(_, declaration)| declaration).collect();
    let mut policies = Vec::with_capacity(types.len());
    let mut functions = Vec::new();
    for (index, (_, declaration)) in types.iter().enumerate() {
        if memory == MemoryModel::Unmanaged {
            policies.push(policy(
                CopyPolicy::Trivial,
                DestroyPolicy::Trivial,
                exclusivity,
            ));
            continue;
        }
        let members = deriver.members(declaration)?;
        let shape = deriver.lifecycle(index)?;
        if shape.trivial {
            let copy = match memory {
                MemoryModel::Affine => CopyPolicy::None,
                MemoryModel::Unmanaged | MemoryModel::Counted => CopyPolicy::Trivial,
            };
            policies.push(policy(copy, DestroyPolicy::Trivial, exclusivity));
            continue;
        }
        let destroy = lifecycle::destroy_name(&declaration.name);
        functions.push(lifecycle::destroy_function(
            declaration,
            &members,
            &type_names,
        )?);
        let copy = if memory == MemoryModel::Counted && shape.copyable {
            let copy = lifecycle::copy_name(&declaration.name);
            functions.push(lifecycle::copy_function(
                declaration,
                &members,
                &type_names,
                &declarations,
            )?);
            CopyPolicy::Implicit(copy)
        } else {
            CopyPolicy::None
        };
        policies.push(policy(copy, DestroyPolicy::Function(destroy), exclusivity));
    }
    Ok((policies, functions))
}

fn policy(copy: CopyPolicy, destroy: DestroyPolicy, exclusivity: Exclusivity) -> Policy {
    Policy {
        copy,
        destroy,
        relocate: RelocatePolicy::Bitwise,
        exclusivity,
        pairing: Pairing::None,
    }
}

struct Deriver<'a> {
    types: &'a [(u64, TypeDeclaration)],
    /// The module's own declarations by name.
    named: HashMap<&'a klf_core::Name, Vec<usize>>,
    lifecycles: Vec<Option<Lifecycle>>,
    visiting: Vec<usize>,
}

impl Deriver<'_> {
    fn lifecycle(&mut self, index: usize) -> Result<Lifecycle, String> {
        if let Some(known) = self.lifecycles[index] {
            return Ok(known);
        }
        let declaration = &self.types[index].1;
        if self.visiting.contains(&index) {
            return Err(format!(
                "default lifecycle policy for `%{}` cannot be derived: the type contains itself",
                declaration.name
            ));
        }
        self.visiting.push(index);
        let members = self.members(declaration);
        self.visiting.pop();
        let mut shape = Lifecycle {
            trivial: true,
            copyable: true,
        };
        for member in members?.iter().flatten() {
            match member {
                Member::Trivial => {}
                Member::Counted => shape.trivial = false,
                Member::Unique => {
                    shape.trivial = false;
                    shape.copyable = false;
                }
                Member::Nested(inner) => {
                    let inner = self.lifecycle(*inner)?;
                    shape.trivial &= inner.trivial;
                    shape.copyable &= inner.copyable;
                }
            }
        }
        self.lifecycles[index] = Some(shape);
        Ok(shape)
    }

    /// Each field's member kind, or each variant's payload member kinds.
    fn members(&mut self, declaration: &TypeDeclaration) -> Result<Vec<Vec<Member>>, String> {
        if !declaration.generics.is_empty() {
            return Err(unsupported(declaration, "generic types need descriptors"));
        }
        if declaration.variants.is_empty() {
            return declaration
                .fields
                .iter()
                .map(|field| Ok(vec![self.member(declaration, &field.ty)?]))
                .collect();
        }
        declaration
            .variants
            .iter()
            .map(|variant| {
                if variant.payload.len() > 1 {
                    return Err(unsupported(declaration, "multi-value payloads"));
                }
                let payload = variant
                    .payload
                    .iter()
                    .map(|ty| self.member(declaration, ty))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(payload)
            })
            .collect()
    }

    fn member(&mut self, owner: &TypeDeclaration, ty: &Type) -> Result<Member, String> {
        match ty {
            Type::Integer(_)
            | Type::Float(_)
            | Type::Pointer
            | Type::Unit
            | Type::Never
            | Type::Address(_)
            | Type::Function(_) => Ok(Member::Trivial),
            Type::Reference(_) => Ok(Member::Counted),
            Type::Unique(_) => Ok(Member::Unique),
            Type::Vector(_, element) | Type::Array(_, element) => {
                match self.member(owner, element)? {
                    Member::Trivial => Ok(Member::Trivial),
                    _ => Err(unsupported(owner, "arrays of managed members")),
                }
            }
            Type::Tuple(elements) => {
                for element in elements {
                    if !matches!(self.member(owner, element)?, Member::Trivial) {
                        return Err(unsupported(owner, "tuples of managed members"));
                    }
                }
                Ok(Member::Trivial)
            }
            Type::Nominal(name, arguments) if arguments.is_empty() => {
                let index = match self.named.get(name).map(Vec::as_slice) {
                    None | Some([]) => {
                        return Err(unsupported(owner, "members of undeclared types"));
                    }
                    Some([index]) => *index,
                    Some(_) => {
                        let what = format!("the member type `%{name}`, declared more than once");
                        return Err(unsupported(owner, &what));
                    }
                };
                if self.lifecycle(index)?.trivial {
                    Ok(Member::Trivial)
                } else {
                    Ok(Member::Nested(index))
                }
            }
            Type::Weak(_) => Err(unsupported(owner, "weak members")),
            Type::Nominal(_, _) | Type::Generic(_) => {
                Err(unsupported(owner, "generic members need descriptors"))
            }
        }
    }
}

fn unsupported(declaration: &TypeDeclaration, what: &str) -> String {
    format!(
        "default lifecycle policy for `%{}` cannot be derived: memberwise synthesis does not cover {what}",
        declaration.name
    )
}
