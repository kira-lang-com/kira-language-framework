//! Methods: functions declared in a struct or enum body, or in an `extend`
//! block of any type the definition can name.
//!
//! A method is an ordinary module function named `Owner.method`. One declared
//! with a `self` receiver takes the receiver as its first parameter and is
//! called as `value.method(...)`; one without is called as `Owner.method(...)`.
//! An aggregate receiver arrives as the address of the caller's storage, so a
//! `borrow mut self` method writes through to it like any `borrow mut`
//! parameter.

use crate::ty::FeType;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{DeclKind, EnumItem, File, FunctionDecl, Ident, MemberKind};
use klf_services::Session;
use std::collections::HashSet;

#[derive(Clone)]
pub(crate) struct Method {
    pub owner: FeType,
    /// The declaration, renamed to its qualified `Owner.method` name.
    pub decl: FunctionDecl,
}

/// The module function a method of `owner` lowers to.
pub(crate) fn qualified(owner: &FeType, method: &str) -> Name {
    Name::new(format!("{}.{method}", owner.display()))
}

pub(crate) fn collect(
    session: &mut Session,
    file: &File,
    struct_names: &HashSet<Name>,
    enum_names: &HashSet<Name>,
) -> Vec<Method> {
    let mut methods = Vec::new();
    for decl in &file.decls {
        match &decl.kind {
            DeclKind::Struct(structure) => {
                let owner = FeType::Struct(structure.name.name.clone());
                for member in &structure.members {
                    if let MemberKind::Method(function) = &member.kind {
                        methods.push(method(&owner, function));
                    }
                }
            }
            DeclKind::Enum(enum_decl) => {
                let owner = FeType::Enum(enum_decl.name.name.clone());
                for item in &enum_decl.items {
                    if let EnumItem::Method(function) = item {
                        methods.push(method(&owner, function));
                    }
                }
            }
            DeclKind::Extend(extend) => {
                let Some(owner) = crate::structs::resolve_type_with_kinds(
                    session,
                    &extend.ty,
                    struct_names,
                    enum_names,
                ) else {
                    continue;
                };
                for member in &extend.items {
                    match &member.kind {
                        MemberKind::Method(function) => methods.push(method(&owner, function)),
                        MemberKind::Field { name, .. } => session.report(
                            Diagnostic::error("KLFK-E0206", "`extend` cannot add stored fields")
                                .with_label(name.span, "declare the field on the type itself"),
                        ),
                        MemberKind::Init { .. } => session.report(
                            Diagnostic::error("KLFK-E0206", "initializers are outside KLF Kira")
                                .with_label(member.span, "use a static method instead"),
                        ),
                    }
                }
            }
            _ => {}
        }
    }
    methods
}

pub(crate) fn method(owner: &FeType, function: &FunctionDecl) -> Method {
    let mut decl = function.clone();
    decl.name = Ident {
        name: qualified(owner, function.name.name.as_str()),
        span: function.name.span,
    };
    Method {
        owner: owner.clone(),
        decl,
    }
}
