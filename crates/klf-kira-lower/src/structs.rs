//! Struct declarations in KLF Kira: collecting them, mapping them to KIR nominal
//! type declarations, and the field table field access lowers against. A struct
//! is a loadable nominal with trivial copy/destroy for now (no `Drop` types
//! yet); a struct value is the address of its storage.

use crate::resolve_type;
use crate::ty::FeType;
use crate::wrappers::{Wrapped, WrappedTable};
use klf_core::{FileId, Name};
use klf_kir::{
    CopyPolicy, DestroyPolicy, Exclusivity, Field, LayoutRule, Pairing, Policy, RelocatePolicy,
    TypeDeclaration,
};
use klf_kira_syntax::ast::{DeclKind, Expr, FieldInit, File, MemberKind};
use klf_services::Session;
use klf_source::Span;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub(crate) struct StructInfo {
    pub span: Span,
    pub fields: Vec<(Name, FeType)>,
    /// The value a field is declared with, which a literal leaving the field
    /// out stores.
    pub defaults: Vec<(Name, Expr)>,
    /// Properties stored through a wrapper (`wrappers.rs`).
    pub wrapped: Vec<Wrapped>,
}

impl StructInfo {
    pub(crate) fn field(&self, name: &str) -> Option<(u32, &FeType)> {
        self.fields
            .iter()
            .enumerate()
            .find(|(_, (field_name, _))| field_name.as_str() == name)
            .map(|(index, (_, ty))| (u32::try_from(index).unwrap_or(0), ty))
    }

    pub(crate) fn wrapped(&self, name: &str) -> Option<&Wrapped> {
        self.wrapped
            .iter()
            .find(|wrapped| wrapped.property.as_str() == name)
    }
}

pub(crate) type StructTable = HashMap<Name, StructInfo>;

/// Where member `name` of struct `owner` lives: the field steps from the
/// struct to it, one for a field and two for a wrapped property, which is its
/// backing field's `wrappedValue`.
pub(crate) fn member(table: &StructTable, owner: &Name, name: &str) -> Option<Vec<(u32, FeType)>> {
    let info = table.get(owner)?;
    if let Some((index, ty)) = info.field(name) {
        return Some(vec![(index, ty.clone())]);
    }
    let wrapped = info.wrapped(name)?;
    let (backing, backing_ty) = info.field(wrapped.backing.as_str())?;
    let FeType::Struct(wrapper_type) = backing_ty else {
        return None;
    };
    let (value, value_ty) = table.get(wrapper_type)?.field("wrappedValue")?;
    Some(vec![
        (backing, backing_ty.clone()),
        (value, value_ty.clone()),
    ])
}

/// Collect every struct declaration into a field table. Two passes: gather the
/// names first so a field can reference another struct, then resolve field
/// types.
pub(crate) fn collect(
    session: &mut Session,
    file: &File,
    enum_names: &HashSet<Name>,
    wrapped: &WrappedTable,
) -> StructTable {
    let names = names(file);
    let mut table = StructTable::new();
    for decl in &file.decls {
        let DeclKind::Struct(structure) = &decl.kind else {
            continue;
        };
        let mut fields = Vec::new();
        let mut defaults = Vec::new();
        for member in &structure.members {
            match &member.kind {
                MemberKind::Field { name, ty, init, .. } => {
                    let field_ty = resolve_type_with_kinds(session, ty, &names, enum_names)
                        .unwrap_or(FeType::Void);
                    fields.push((name.name.clone(), field_ty));
                    if let Some(FieldInit::Expr(value)) = init {
                        defaults.push((name.name.clone(), value.clone()));
                    }
                }
                MemberKind::Method(_) => {}
                MemberKind::Init { .. } => {
                    session.report(
                        klf_diag::Diagnostic::error(
                            "KLFK-E0206",
                            "initializers are outside KLF Kira",
                        )
                        .with_label(member.span, "use a static method instead"),
                    );
                }
            }
        }
        table.insert(
            structure.name.name.clone(),
            StructInfo {
                span: structure.span,
                fields,
                defaults,
                wrapped: wrapped
                    .get(&structure.name.name)
                    .cloned()
                    .unwrap_or_default(),
            },
        );
    }
    add_builtin_structs(&mut table);
    table
}

/// The declared structs and the kernel-provided `String` and `DriverRequest`.
pub(crate) fn names(file: &File) -> HashSet<Name> {
    let mut names: HashSet<Name> = file
        .decls
        .iter()
        .filter_map(|decl| match &decl.kind {
            DeclKind::Struct(structure) => Some(structure.name.name.clone()),
            _ => None,
        })
        .collect();
    names.insert(Name::new("String"));
    names.insert(Name::new(DRIVER_REQUEST));
    names
}

fn add_builtin_structs(table: &mut StructTable) {
    table
        .entry(Name::new("String"))
        .or_insert_with(|| StructInfo {
            span: Span::empty(FileId::from_u32(0), 0),
            defaults: Vec::new(),
            wrapped: Vec::new(),
            fields: vec![
                (
                    Name::new("data"),
                    FeType::Address(Box::new(FeType::Array {
                        element: Box::new(FeType::Int {
                            bits: 8,
                            signed: false,
                        }),
                        length: Some(0),
                    })),
                ),
                (
                    Name::new("len"),
                    FeType::Int {
                        bits: 64,
                        signed: true,
                    },
                ),
                (
                    Name::new("cap"),
                    FeType::Int {
                        bits: 64,
                        signed: true,
                    },
                ),
                (Name::new("ops"), FeType::Pointer),
            ],
        });
    table
        .entry(Name::new("List"))
        .or_insert_with(|| StructInfo {
            span: Span::empty(FileId::from_u32(0), 0),
            defaults: Vec::new(),
            wrapped: Vec::new(),
            fields: vec![
                (
                    Name::new("count"),
                    FeType::Int {
                        bits: 64,
                        signed: true,
                    },
                ),
                (
                    Name::new("cap"),
                    FeType::Int {
                        bits: 64,
                        signed: true,
                    },
                ),
                (Name::new("data"), FeType::Pointer),
            ],
        });
    table
        .entry(Name::new(crate::ty::CLOSURE_RECORD))
        .or_insert_with(|| StructInfo {
            span: Span::empty(FileId::from_u32(0), 0),
            defaults: Vec::new(),
            wrapped: Vec::new(),
            fields: vec![
                (Name::new("code"), FeType::Pointer),
                (Name::new("ctx"), FeType::Pointer),
            ],
        });
    table
        .entry(Name::new(DRIVER_REQUEST))
        .or_insert_with(|| StructInfo {
            span: Span::empty(FileId::from_u32(0), 0),
            defaults: Vec::new(),
            wrapped: Vec::new(),
            fields: vec![(
                Name::new("arguments"),
                FeType::List(Box::new(FeType::Struct(Name::new("String")))),
            )],
        });
}

/// The kernel-provided `struct DriverRequest { let arguments: [String] }`.
pub(crate) const DRIVER_REQUEST: &str = "DriverRequest";

pub(crate) fn resolve_type_with_kinds(
    session: &mut Session,
    ty: &klf_kira_syntax::ast::Type,
    struct_names: &HashSet<Name>,
    enum_names: &HashSet<Name>,
) -> Option<FeType> {
    resolve_type_with_context(session, ty, struct_names, enum_names, &HashSet::new())
}

pub(crate) fn resolve_type_with_context(
    session: &mut Session,
    ty: &klf_kira_syntax::ast::Type,
    struct_names: &HashSet<Name>,
    enum_names: &HashSet<Name>,
    generic_names: &HashSet<Name>,
) -> Option<FeType> {
    use klf_kira_syntax::ast::TypeKind;
    if let TypeKind::Function { params, ret } = &ty.kind {
        let mut resolve =
            |ty| resolve_type_with_context(session, ty, struct_names, enum_names, generic_names);
        let params = params
            .iter()
            .map(&mut resolve)
            .collect::<Option<Vec<_>>>()?;
        let result = Box::new(resolve(ret)?);
        return Some(FeType::Function { params, result });
    }
    if let TypeKind::Array(element) = &ty.kind {
        return Some(FeType::List(Box::new(resolve_type_with_context(
            session,
            element,
            struct_names,
            enum_names,
            generic_names,
        )?)));
    }
    if let TypeKind::Named { path, args } = &ty.kind
        && args.is_empty()
        && path.segments.len() == 1
    {
        let name = &path.segments[0].name;
        if generic_names.contains(name) {
            return Some(FeType::Generic(name.clone()));
        }
        if enum_names.contains(name) {
            return Some(FeType::Enum(name.clone()));
        }
        if struct_names.contains(name) {
            return Some(FeType::Struct(name.clone()));
        }
    }
    if let TypeKind::Named { path, args } = &ty.kind
        && !args.is_empty()
        && let [base] = path.segments.as_slice()
    {
        let arguments = args
            .iter()
            .map(|argument| {
                resolve_type_with_context(
                    session,
                    argument,
                    struct_names,
                    enum_names,
                    generic_names,
                )
            })
            .collect::<Option<Vec<FeType>>>()?;
        let instance = crate::generics::instance_name(base.name.as_str(), &arguments);
        if enum_names.contains(&instance) {
            return Some(FeType::Enum(instance));
        }
        session.report(
            klf_diag::Diagnostic::error(
                "KLFK-E0200",
                format!("unknown generic type `{}`", base.name),
            )
            .with_label(ty.span, "not a generic enum"),
        );
        return None;
    }
    resolve_type(session, ty)
}

/// The KIR nominal type declarations for the collected structs.
pub(crate) fn type_declarations(table: &StructTable) -> Vec<TypeDeclaration> {
    let mut declarations: Vec<TypeDeclaration> = table
        .iter()
        .map(|(name, info)| TypeDeclaration {
            open: false,
            name: name.clone(),
            generics: Vec::new(),
            kind: klf_kir::TypeKind::Struct,
            layout: LayoutRule::Ridl,
            size: None,
            align: None,
            fields: info
                .fields
                .iter()
                .map(|(field_name, ty)| Field {
                    name: field_name.clone(),
                    ty: ty.to_kir(),
                    offset: None,
                })
                .collect(),
            variants: Vec::new(),
            policy: Policy {
                copy: CopyPolicy::Trivial,
                destroy: DestroyPolicy::Trivial,
                relocate: RelocatePolicy::Bitwise,
                exclusivity: Exclusivity::None,
                pairing: Pairing::None,
            },
            span: info.span,
        })
        .collect();
    declarations.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    declarations
}
