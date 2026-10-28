//! The optional `flags` member of a section-7 `Language` declaration.

use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{Expr, ExprKind, UnaryOp};
use klf_services::{DriverFlag, DriverFlagValue, Session};
use klf_source::Span;

/// Reads declared driver flags. An omitted member has the section-7 `[]` default.
pub(crate) fn optional_flags(
    session: &mut Session,
    value: Option<&Expr>,
) -> Option<Vec<DriverFlag>> {
    let Some(value) = value else {
        return Some(Vec::new());
    };
    let ExprKind::ArrayLit(items) = &value.kind else {
        report(session, value.span, "must be an array of DriverFlag values");
        return None;
    };
    items
        .iter()
        .map(|item| driver_flag(session, item))
        .collect()
}

fn driver_flag(session: &mut Session, value: &Expr) -> Option<DriverFlag> {
    let ExprKind::StructLit { ty, fields } = &value.kind else {
        report(session, value.span, "entries must be DriverFlag literals");
        return None;
    };
    if !type_named(ty, "DriverFlag") {
        report(session, value.span, "entries must be DriverFlag literals");
        return None;
    }
    let mut name = None;
    let mut default_value = None;
    for (field, init) in fields {
        match field.name.as_str() {
            "name" => name = Some(string(session, init)?),
            "default_value" => default_value = Some(flag_value(session, init)?),
            other => {
                report(
                    session,
                    field.span,
                    &format!("unknown DriverFlag field `{other}`"),
                );
                return None;
            }
        }
    }
    let (Some(name), Some(default_value)) = (name, default_value) else {
        report(
            session,
            value.span,
            "DriverFlag needs name and default_value",
        );
        return None;
    };
    Some(DriverFlag {
        name,
        default_value,
    })
}

fn flag_value(session: &mut Session, value: &Expr) -> Option<DriverFlagValue> {
    let ExprKind::EnumLit { name, args } = &value.kind else {
        report(
            session,
            value.span,
            "default_value must be .Boolean, .Integer or .String with one scalar value",
        );
        return None;
    };
    let Some([argument]) = args.as_deref() else {
        report(
            session,
            value.span,
            "default_value case needs one scalar value",
        );
        return None;
    };
    match (name.name.as_str(), &argument.value.kind) {
        ("Boolean", ExprKind::Bool(value)) => Some(DriverFlagValue::Boolean(*value)),
        ("Integer", value) => integer_value(session, &argument.value, value),
        ("String", ExprKind::Str(value)) => Some(DriverFlagValue::String(value.clone())),
        ("Boolean" | "String", _) => {
            report(
                session,
                argument.value.span,
                "default_value scalar does not match its declared case",
            );
            None
        }
        other => {
            report(
                session,
                value.span,
                &format!("unknown DriverFlag default case `.{}`", other.0),
            );
            None
        }
    }
}

fn integer_value(
    session: &mut Session,
    expression: &Expr,
    value: &ExprKind,
) -> Option<DriverFlagValue> {
    let (text, negative) = match value {
        ExprKind::Int(text) => (text.as_str(), false),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => {
            let ExprKind::Int(text) = &operand.kind else {
                report(
                    session,
                    expression.span,
                    "Integer default_value must be an integer literal",
                );
                return None;
            };
            (text.as_str(), true)
        }
        _ => {
            report(
                session,
                expression.span,
                "Integer default_value must be an integer literal",
            );
            return None;
        }
    };
    let parsed = text
        .replace('_', "")
        .parse::<i128>()
        .ok()
        .and_then(|value| i64::try_from(if negative { -value } else { value }).ok());
    if let Some(value) = parsed {
        Some(DriverFlagValue::Integer(value))
    } else {
        report(
            session,
            expression.span,
            "Integer default_value is outside the i64 range",
        );
        None
    }
}

fn string(session: &mut Session, value: &Expr) -> Option<String> {
    if let ExprKind::Str(value) = &value.kind {
        return Some(value.clone());
    }
    report(session, value.span, "expected a string literal");
    None
}

fn type_named(ty: &klf_kira_syntax::ast::Type, expected: &str) -> bool {
    matches!(
        &ty.kind,
        klf_kira_syntax::ast::TypeKind::Named { path, args }
            if args.is_empty()
                && path.segments.len() == 1
                && path.segments[0].name.as_str() == expected
    )
}

fn report(session: &mut Session, span: Span, message: &str) {
    session.report(
        Diagnostic::error_at(
            "KLF-DEF-0002",
            span,
            format!("Language metadata field `flags` {message}"),
        )
        .with_label(span, message.to_owned()),
    );
}
