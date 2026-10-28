//! KLF Kira's elaborated types backed by the kernel session handles.

use crate::Signature;
use crate::enums::EnumTable;
use crate::structs::StructTable;
use crate::ty::FeType;
use klf_core::Name;
use klf_kir::{
    CopyPolicy, DestroyPolicy, Exclusivity, LayoutRule, Pairing, Policy, RelocatePolicy,
};
use klf_kira_syntax::ast::{DeclKind, File, Member, MemberKind};
use klf_services::{FunctionSignature, Session, TypeId, TypeKind, TypeLayout};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct TypeHandles {
    named: HashMap<Name, TypeId>,
    ids: Arc<Mutex<HashMap<FeType, TypeId>>>,
    function_types: HashMap<Name, TypeId>,
}

impl TypeHandles {
    pub(crate) fn build(
        session: &mut Session,
        structs: &StructTable,
        enums: &EnumTable,
        signatures: &HashMap<Name, Signature>,
    ) -> Self {
        let mut handles = Self {
            named: HashMap::new(),
            ids: Arc::new(Mutex::new(HashMap::new())),
            function_types: HashMap::new(),
        };
        for name in structs.keys() {
            handles.register_nominal(session, name, true);
        }
        for name in enums.keys() {
            handles.register_nominal(session, name, false);
        }

        for (name, info) in structs {
            let fields = info
                .fields
                .iter()
                .map(|(_, ty)| handles.id_for(session, ty))
                .collect();
            let type_id = handles.named[name];
            session.complete_nominal(
                type_id,
                TypeLayout::Struct {
                    fields,
                    rule: LayoutRule::Ridl,
                },
                struct_policy(),
            );
        }
        for (name, info) in enums {
            let variants = info
                .variants
                .iter()
                .map(|variant| {
                    variant
                        .stored(name)
                        .iter()
                        .map(|ty| handles.id_for(session, ty))
                        .collect()
                })
                .collect();
            let type_id = handles.named[name];
            session.complete_nominal(
                type_id,
                TypeLayout::Enum {
                    variants,
                    rule: LayoutRule::Ridl,
                },
                enum_policy(),
            );
        }

        for (name, signature) in signatures {
            let parameters = signature
                .params
                .iter()
                .map(|ty| handles.id_for(session, ty))
                .collect();
            let result = handles.id_for(session, &signature.result);
            let function = session.function_type(FunctionSignature {
                parameters,
                result,
                failure: None,
            });
            handles.function_types.insert(name.clone(), function);
        }
        handles
    }

    pub(crate) fn id_for(&self, session: &mut Session, ty: &FeType) -> TypeId {
        if let Some(id) = self
            .ids
            .lock()
            .expect("Kira type handle map is not poisoned")
            .get(ty)
            .copied()
        {
            return id;
        }
        let id = match ty {
            FeType::Void => session.void_type(),
            FeType::Int { bits, signed } => session.int_type(*bits, *signed),
            FeType::Float { bits } => session.float_type(*bits),
            FeType::Bool => session.int_type(1, false),
            FeType::Struct(name) | FeType::Enum(name) => *self
                .named
                .get(name)
                .unwrap_or_else(|| panic!("unknown Kira nominal type `{name}`")),
            FeType::Generic(name) => session.generic_type(name.clone()),
            FeType::Array { element, length } => {
                let element = self.id_for(session, element);
                session.array_type(element, length.unwrap_or(0))
            }
            FeType::List(element) => {
                let element = self.id_for(session, element);
                session.array_type(element, 0)
            }
            FeType::Address(inner) => {
                let pointee = self.id_for(session, inner);
                session.address_type(pointee)
            }
            FeType::Pointer => session.pointer_type(),
            FeType::DefinitionHandle(_) => session.int_type(64, true),
            FeType::Function { params, result } => {
                let parameters = params.iter().map(|ty| self.id_for(session, ty)).collect();
                let result = self.id_for(session, result);
                session.function_type(FunctionSignature {
                    parameters,
                    result,
                    failure: None,
                })
            }
        };
        self.ids
            .lock()
            .expect("Kira type handle map is not poisoned")
            .insert(ty.clone(), id);
        id
    }

    pub(crate) fn function_type(&self, name: &Name) -> Option<TypeId> {
        self.function_types.get(name).copied()
    }

    fn register_nominal(&mut self, session: &mut Session, name: &Name, is_struct: bool) {
        let type_id = session.opaque_nominal(name.clone());
        self.named.insert(name.clone(), type_id);
        let key = if is_struct {
            FeType::Struct(name.clone())
        } else {
            FeType::Enum(name.clone())
        };
        self.ids
            .lock()
            .expect("Kira type handle map is not poisoned")
            .insert(key, type_id);
    }
}

pub(crate) fn register_conformances(
    session: &mut Session,
    file: &File,
    types: &TypeHandles,
    traits: &HashMap<Name, klf_services::TraitId>,
    structs: &StructTable,
    enums: &EnumTable,
) {
    let struct_names = structs.keys().cloned().collect();
    let enum_names = enums.keys().cloned().collect();
    for decl in &file.decls {
        let (target, conforms, witnesses) = match &decl.kind {
            DeclKind::Struct(structure) => (
                Some(FeType::Struct(structure.name.name.clone())),
                structure.conforms.clone(),
                Vec::new(),
            ),
            DeclKind::Enum(enum_decl) => (
                Some(FeType::Enum(enum_decl.name.name.clone())),
                enum_decl.conforms.clone(),
                Vec::new(),
            ),
            DeclKind::Extend(extension) => (
                crate::structs::resolve_type_with_kinds(
                    session,
                    &extension.ty,
                    &struct_names,
                    &enum_names,
                ),
                extension.conforms.clone(),
                member_witnesses(&extension.items),
            ),
            _ => (None, Vec::new(), Vec::new()),
        };
        let Some(target) = target else {
            continue;
        };
        let type_id = types.id_for(session, &target);
        for bound in &conforms {
            let Some(trait_id) = crate::trait_id_for(session, bound, traits) else {
                continue;
            };
            session.conformance(type_id, trait_id, witnesses.clone(), Vec::new());
        }
    }
}

pub(crate) fn witness_declarations(session: &Session) -> Vec<klf_kir::WitnessDeclaration> {
    session
        .conformance_records()
        .into_iter()
        .filter_map(|(type_id, trait_id, conformance)| {
            let trait_name = session.trait_info(trait_id)?.name.clone();
            let ty = kir_type(session, type_id, &mut HashSet::new())?;
            let associated = conformance
                .associated
                .iter()
                .map(|type_id| kir_type(session, *type_id, &mut HashSet::new()))
                .collect::<Option<Vec<_>>>()?;
            Some(klf_kir::WitnessDeclaration {
                ty,
                trait_name,
                witnesses: conformance.witnesses,
                associated,
            })
        })
        .collect()
}

fn kir_type(
    session: &Session,
    type_id: TypeId,
    visiting: &mut HashSet<TypeId>,
) -> Option<klf_kir::Type> {
    if !visiting.insert(type_id) {
        return None;
    }
    let result = match &session.type_info(type_id)?.kind {
        TypeKind::Void => Some(klf_kir::Type::Unit),
        TypeKind::Integer { bits, .. } => Some(klf_kir::Type::Integer(*bits)),
        TypeKind::Float { bits } => Some(klf_kir::Type::Float(*bits)),
        TypeKind::Pointer => Some(klf_kir::Type::Pointer),
        TypeKind::Address(pointee) => Some(klf_kir::Type::Address(Box::new(kir_type(
            session, *pointee, visiting,
        )?))),
        TypeKind::Vector { element, count } => Some(klf_kir::Type::Vector(
            *count,
            Box::new(kir_type(session, *element, visiting)?),
        )),
        TypeKind::Tuple(elements) => Some(klf_kir::Type::Tuple(
            elements
                .iter()
                .map(|element| kir_type(session, *element, visiting))
                .collect::<Option<Vec<_>>>()?,
        )),
        TypeKind::Array { element, count } => Some(klf_kir::Type::Array(
            *count,
            Box::new(kir_type(session, *element, visiting)?),
        )),
        TypeKind::Function(signature) => Some(klf_kir::Type::Function(klf_kir::FunctionType {
            abi: klf_kir::FunctionAbi::Ridl,
            parameters: signature
                .parameters
                .iter()
                .map(|parameter| {
                    Some(klf_kir::ParameterType {
                        convention: None,
                        context: false,
                        ty: kir_type(session, *parameter, visiting)?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
            result: Box::new(kir_type(session, signature.result, visiting)?),
            failure: signature
                .failure
                .and_then(|failure| kir_type(session, failure, visiting).map(Box::new)),
        })),
        TypeKind::Nominal(name) => Some(klf_kir::Type::Nominal(name.clone(), Vec::new())),
        TypeKind::Generic(name) => Some(klf_kir::Type::Generic(name.clone())),
    };
    visiting.remove(&type_id);
    result
}

fn member_witnesses(members: &[Member]) -> Vec<Name> {
    members
        .iter()
        .filter_map(|member| match &member.kind {
            MemberKind::Method(function) => Some(function.name.name.clone()),
            MemberKind::Field { .. } | MemberKind::Init { .. } => None,
        })
        .collect()
}

fn struct_policy() -> Policy {
    Policy {
        copy: CopyPolicy::Trivial,
        destroy: DestroyPolicy::Trivial,
        relocate: RelocatePolicy::Bitwise,
        exclusivity: Exclusivity::None,
        pairing: Pairing::None,
    }
}

fn enum_policy() -> Policy {
    Policy {
        copy: CopyPolicy::None,
        destroy: DestroyPolicy::Trivial,
        relocate: RelocatePolicy::Bitwise,
        exclusivity: Exclusivity::None,
        pairing: Pairing::None,
    }
}
