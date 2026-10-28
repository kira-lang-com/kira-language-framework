//! The optional `diagnostics` member of a section-7 `Language` declaration.
//!
//! Section 31.4 requires a definition to declare every code it reports, so the
//! kernel can render a number a phase raises with the text that belongs to it.

use klf_diag::{Diagnostic, Severity};
use klf_kira_syntax::ast::{Expr, ExprKind};
use klf_services::{DiagnosticSpec, Session};
use klf_source::Span;

/// Reads `diagnostics` if the declaration has it. A definition that reports no
/// numeric codes leaves the member out, which is the `[]` default.
pub(crate) fn optional_diagnostics(
    session: &mut Session,
    value: Option<&Expr>,
) -> Option<Vec<DiagnosticSpec>> {
    let Some(value) = value else {
        return Some(Vec::new());
    };
    let ExprKind::ArrayLit(items) = &value.kind else {
        report(
            session,
            value.span,
            "must be an array of DiagnosticSpec values",
        );
        return None;
    };
    let mut specs = Vec::with_capacity(items.len());
    for item in items {
        specs.push(spec(session, item)?);
    }
    Some(specs)
}

fn spec(session: &mut Session, value: &Expr) -> Option<DiagnosticSpec> {
    let ExprKind::StructLit { ty, fields } = &value.kind else {
        report(
            session,
            value.span,
            "entries must be DiagnosticSpec literals",
        );
        return None;
    };
    if !type_named(ty, "DiagnosticSpec") {
        report(
            session,
            value.span,
            "entries must be DiagnosticSpec literals",
        );
        return None;
    }
    let mut code = None;
    let mut severity = None;
    let mut title = None;
    let mut explanation = None;
    let mut help = None;
    for (field, init) in fields {
        match field.name.as_str() {
            "code" => code = Some(integer(session, init)?),
            "severity" => severity = Some(severity_case(session, init)?),
            "title" => title = Some(string(session, init)?),
            "explanation" => explanation = Some(string(session, init)?),
            "help" => help = Some(string(session, init)?),
            other => {
                report(
                    session,
                    field.span,
                    &format!("unknown DiagnosticSpec field `{other}`"),
                );
                return None;
            }
        }
    }
    let (Some(code), Some(title), Some(explanation)) = (code, title, explanation) else {
        report(
            session,
            value.span,
            "DiagnosticSpec needs code, title and explanation",
        );
        return None;
    };
    Some(
        DiagnosticSpec::new(code, severity.unwrap_or(Severity::Error), title, explanation)
            .with_help(help.unwrap_or_default()),
    )
}

fn severity_case(session: &mut Session, value: &Expr) -> Option<Severity> {
    let ExprKind::EnumLit { name, args: None } = &value.kind else {
        report(session, value.span, "severity must be an enum case");
        return None;
    };
    match name.name.as_str() {
        "Error" => Some(Severity::Error),
        "Warning" => Some(Severity::Warning),
        "Note" => Some(Severity::Note),
        "Remark" => Some(Severity::Remark),
        other => {
            report(session, value.span, &format!("unknown severity `.{other}`"));
            None
        }
    }
}

fn integer(session: &mut Session, value: &Expr) -> Option<i64> {
    if let ExprKind::Int(text) = &value.kind
        && let Ok(parsed) = text.as_str().replace('_', "").parse::<i64>()
    {
        return Some(parsed);
    }
    report(session, value.span, "expected an integer literal");
    None
}

fn string(session: &mut Session, value: &Expr) -> Option<String> {
    if let ExprKind::Str(text) = &value.kind {
        return Some(text.clone());
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
            format!("Language metadata field `diagnostics` {message}"),
        )
        .with_label(span, message.to_owned()),
    );
}
