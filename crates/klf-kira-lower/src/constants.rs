//! Top-level `let` constants.
//!
//! A constant's value is a literal, inlined at every use, so it needs no
//! storage and no initialization order, and definition code still has no
//! global state (section 6.3).

use crate::structs::resolve_type_with_kinds;
use crate::ty::FeType;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{DeclKind, Expr, ExprKind, File, UnaryOp};
use klf_services::Session;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub(crate) struct Constant {
    pub value: Expr,
}

pub(crate) type ConstantTable = HashMap<Name, Constant>;

pub(crate) fn collect(
    session: &mut Session,
    file: &File,
    struct_names: &HashSet<Name>,
    enum_names: &HashSet<Name>,
) -> ConstantTable {
    let mut table = ConstantTable::new();
    for decl in &file.decls {
        let DeclKind::Constant(constant) = &decl.kind else {
            continue;
        };
        let Some(ty) = literal_type(&constant.value) else {
            session.report(
                Diagnostic::error("KLFK-E0206", "a constant's value must be a literal")
                    .with_label(constant.value.span, "not a literal"),
            );
            continue;
        };
        if let Some(annotation) = &constant.ty
            && resolve_type_with_kinds(session, annotation, struct_names, enum_names)
                .is_some_and(|declared| declared != ty)
        {
            session.report(
                Diagnostic::error(
                    "KLFK-E0201",
                    format!(
                        "constant `{}` is a {} literal",
                        constant.name.name,
                        ty.display()
                    ),
                )
                .with_label(annotation.span, "annotation disagrees with the value"),
            );
        }
        table.insert(
            constant.name.name.clone(),
            Constant {
                value: constant.value.clone(),
            },
        );
    }
    table
}

fn literal_type(value: &Expr) -> Option<FeType> {
    match &value.kind {
        ExprKind::Int(_) => FeType::from_name("Int"),
        ExprKind::Float(_) => FeType::from_name("Float"),
        ExprKind::Bool(_) => Some(FeType::Bool),
        ExprKind::Str(_) => Some(FeType::Struct(Name::new("String"))),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } if matches!(operand.kind, ExprKind::Int(_) | ExprKind::Float(_)) => literal_type(operand),
        _ => None,
    }
}
