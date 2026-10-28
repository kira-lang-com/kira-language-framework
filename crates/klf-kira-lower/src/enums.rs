//! Enum declarations and variant metadata for the KLF Kira subset.

use crate::structs::{StructInfo, StructTable, resolve_type_with_kinds};
use crate::ty::FeType;
use klf_core::{FileId, Name};
use klf_diag::Diagnostic;
use klf_kir::{
    CopyPolicy, DestroyPolicy, EnumVariant, Exclusivity, LayoutRule, Pairing, Policy,
    RelocatePolicy, TypeDeclaration,
};
use klf_kira_syntax::ast::{CasePayload, DeclKind, EnumItem, File};
use klf_services::Session;
use klf_source::Span;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub(crate) struct EnumCase {
    pub name: Name,
    pub payload: Vec<FeType>,
}

impl EnumCase {
    /// The one value the variant carries in KIR: nothing, its single payload,
    /// or the record that holds several (see [`payload_records`]).
    pub(crate) fn stored(&self, owner: &Name) -> Option<FeType> {
        match self.payload.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            _ => Some(FeType::Struct(payload_record(owner, &self.name))),
        }
    }
}

fn payload_record(owner: &Name, case: &Name) -> Name {
    Name::new(format!("{owner}.{case}"))
}

/// A variant with several payloads carries them in a record named
/// `Enum.Case` with fields `0`, `1`, ... A KIR variant with several payloads
/// is a tuple, and `field.addr` reaches into a nominal struct, not a tuple.
pub(crate) fn payload_records(table: &EnumTable) -> StructTable {
    let mut records = StructTable::new();
    for (owner, info) in table {
        for case in &info.variants {
            if case.payload.len() < 2 {
                continue;
            }
            let fields = case
                .payload
                .iter()
                .enumerate()
                .map(|(index, ty)| (Name::new(index.to_string()), ty.clone()))
                .collect();
            records.insert(
                payload_record(owner, &case.name),
                StructInfo {
                    span: info.span,
                    fields,
                    defaults: Vec::new(),
                    wrapped: Vec::new(),
                },
            );
        }
    }
    records
}

#[derive(Clone)]
pub(crate) struct EnumInfo {
    pub span: Span,
    pub variants: Vec<EnumCase>,
}

impl EnumInfo {
    pub(crate) fn variant(&self, name: &str) -> Option<(u32, EnumCase)> {
        self.variants
            .iter()
            .enumerate()
            .find(|(_, variant)| variant.name.as_str() == name)
            .map(|(index, variant)| (u32::try_from(index).unwrap_or(0), variant.clone()))
    }
}

pub(crate) type EnumTable = HashMap<Name, EnumInfo>;

const BUILTIN_ENUM_NAMES: &[&str] = &[
    "DriverFlagValue",
    "MemoryModel",
    "DefaultConvention",
    "EntryConvention",
];

/// Names a package cannot declare: the kernel's enums and the prelude's.
const RESERVED_ENUM_NAMES: &[&str] = &[
    "DriverFlagValue",
    "MemoryModel",
    "DefaultConvention",
    "EntryConvention",
    "Optional",
    "Result",
];

/// The package's non-generic enums and the kernel's.
pub(crate) fn names(file: &File) -> HashSet<Name> {
    let mut names: HashSet<Name> = file
        .decls
        .iter()
        .filter_map(|decl| match &decl.kind {
            DeclKind::Enum(enum_decl) if !crate::generics::is_template(enum_decl) => {
                Some(enum_decl.name.name.clone())
            }
            _ => None,
        })
        .collect();
    names.extend(BUILTIN_ENUM_NAMES.iter().copied().map(Name::new));
    names
}

/// The package's non-generic enums, the kernel's, and the generic `instances`.
/// `enum_names` covers all three, so a payload may name any of them.
pub(crate) fn collect(
    session: &mut Session,
    file: &File,
    struct_names: &HashSet<Name>,
    enum_names: &HashSet<Name>,
    instances: EnumTable,
) -> EnumTable {
    let mut table = instances;
    for decl in &file.decls {
        let DeclKind::Enum(enum_decl) = &decl.kind else {
            continue;
        };
        if RESERVED_ENUM_NAMES.contains(&enum_decl.name.name.as_str()) {
            session.report(
                Diagnostic::error(
                    "KLFK-E0200",
                    format!(
                        "`{}` is a reserved kernel-provided enum",
                        enum_decl.name.name
                    ),
                )
                .with_label(enum_decl.name.span, "reserved type"),
            );
            continue;
        }
        if crate::generics::is_template(enum_decl) {
            continue;
        }
        if !enum_decl.conforms.is_empty() {
            session.report(
                Diagnostic::error(
                    "KLFK-E0206",
                    "conforming enums are outside the current subset",
                )
                .with_label(enum_decl.span, "not supported yet"),
            );
        }
        let mut variants = Vec::new();
        for item in &enum_decl.items {
            let EnumItem::Case(case) = item else {
                continue;
            };
            if variants
                .iter()
                .any(|variant: &EnumCase| variant.name == case.name.name)
            {
                session.report(
                    Diagnostic::error(
                        "KLFK-E0206",
                        format!("duplicate enum case `{}`", case.name.name),
                    )
                    .with_label(case.span, "already declared"),
                );
            }
            if let CasePayload::Typed {
                default: Some(_), ..
            } = &case.payload
            {
                session.report(
                    Diagnostic::error("KLFK-E0206", "enum case defaults are outside KLF Kira")
                        .with_label(case.span, "not supported"),
                );
            }
            let payload = crate::generics::payload_types(&case.payload)
                .into_iter()
                .map(|ty| {
                    resolve_type_with_kinds(session, ty, struct_names, enum_names)
                        .unwrap_or(FeType::Void)
                })
                .collect();
            variants.push(EnumCase {
                name: case.name.name.clone(),
                payload,
            });
        }
        table.insert(
            enum_decl.name.name.clone(),
            EnumInfo {
                span: enum_decl.span,
                variants,
            },
        );
    }
    add_builtin_enums(&mut table);
    table
}

fn add_builtin_enums(table: &mut EnumTable) {
    table.insert(
        Name::new("DriverFlagValue"),
        EnumInfo {
            span: Span::empty(FileId::from_u32(0), 0),
            variants: vec![
                EnumCase {
                    name: Name::new("Boolean"),
                    payload: vec![FeType::Bool],
                },
                EnumCase {
                    name: Name::new("Integer"),
                    payload: vec![FeType::Int {
                        bits: 64,
                        signed: true,
                    }],
                },
                EnumCase {
                    name: Name::new("String"),
                    payload: vec![FeType::Struct(Name::new("String"))],
                },
            ],
        },
    );
    for (name, variants) in [
        ("MemoryModel", &["Unmanaged", "Counted", "Affine"][..]),
        ("DefaultConvention", &["Ridl", "Platform"][..]),
        (
            "EntryConvention",
            &["Outcome", "Process", "ProcessExit"][..],
        ),
    ] {
        table.insert(
            Name::new(name),
            EnumInfo {
                span: Span::empty(FileId::from_u32(0), 0),
                variants: variants
                    .iter()
                    .map(|variant| EnumCase {
                        name: Name::new(variant),
                        payload: Vec::new(),
                    })
                    .collect(),
            },
        );
    }
}

pub(crate) fn type_declarations(table: &EnumTable) -> Vec<TypeDeclaration> {
    let mut declarations: Vec<TypeDeclaration> = table
        .iter()
        .map(|(name, info)| TypeDeclaration {
            open: false,
            name: name.clone(),
            generics: Vec::new(),
            kind: klf_kir::TypeKind::Enum,
            layout: LayoutRule::Ridl,
            size: None,
            align: None,
            fields: Vec::new(),
            variants: info
                .variants
                .iter()
                .map(|variant| EnumVariant {
                    name: variant.name.clone(),
                    payload: variant.stored(name).iter().map(FeType::to_kir).collect(),
                })
                .collect(),
            policy: Policy {
                copy: CopyPolicy::None,
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
