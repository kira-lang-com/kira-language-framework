//! The optional `fileModes` member of a section-7 `Language` declaration.

use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{Expr, ExprKind};
use klf_services::{DefaultConvention, FileMode, MemoryModel, Session};
use klf_source::Span;

/// Reads `fileModes` if the declaration has it. A package without per-file
/// exceptions leaves the member out, which is the `[]` default of section 7.2.
pub(crate) fn optional_file_modes(
    session: &mut Session,
    value: Option<&Expr>,
) -> Option<Vec<FileMode>> {
    let Some(value) = value else {
        return Some(Vec::new());
    };
    let ExprKind::ArrayLit(items) = &value.kind else {
        report(session, value.span, "must be an array of FileMode values");
        return None;
    };
    let mut modes = Vec::with_capacity(items.len());
    for item in items {
        modes.push(file_mode(session, item)?);
    }
    Some(modes)
}

fn file_mode(session: &mut Session, value: &Expr) -> Option<FileMode> {
    let ExprKind::StructLit { ty, fields } = &value.kind else {
        report(session, value.span, "entries must be FileMode literals");
        return None;
    };
    if !type_named(ty, "FileMode") {
        report(session, value.span, "entries must be FileMode literals");
        return None;
    }
    let mut name = None;
    let mut pragma = None;
    let mut flag = None;
    let mut extensions = Vec::new();
    let mut memory = None;
    let mut convention = None;
    for (field, init) in fields {
        match field.name.as_str() {
            "name" => name = Some(string(session, init)?),
            "pragma" => pragma = Some(string(session, init)?),
            "flag" => flag = Some(string(session, init)?),
            "extensions" => extensions = strings(session, init)?,
            "memory" => memory = optional_case(session, init, memory_model).ok()?,
            "convention" => convention = optional_case(session, init, convention_kind).ok()?,
            other => {
                report(
                    session,
                    field.span,
                    &format!("unknown FileMode field `{other}`"),
                );
                return None;
            }
        }
    }
    let (Some(name), Some(pragma), Some(flag)) = (name, pragma, flag) else {
        report(session, value.span, "FileMode needs name, pragma and flag");
        return None;
    };
    Some(FileMode {
        name,
        pragma,
        flag,
        extensions,
        memory,
        convention,
    })
}

/// The value was not a well-formed `Optional<T>`; the diagnostic is reported.
struct Invalid;

/// Reads an `Optional<T>` metadata value written as `.None` or `.Some(.Case)`.
fn optional_case<T>(
    session: &mut Session,
    value: &Expr,
    parse: fn(&str) -> Option<T>,
) -> Result<Option<T>, Invalid> {
    let ExprKind::EnumLit { name, args } = &value.kind else {
        report(session, value.span, "expected `.None` or `.Some(.Case)`");
        return Err(Invalid);
    };
    match (name.name.as_str(), args.as_deref()) {
        ("None", None) => Ok(None),
        ("Some", Some([argument])) => {
            let ExprKind::EnumLit {
                name: inner,
                args: None,
            } = &argument.value.kind
            else {
                report(session, argument.value.span, "expected an enum case");
                return Err(Invalid);
            };
            let Some(parsed) = parse(inner.name.as_str()) else {
                report(
                    session,
                    argument.value.span,
                    &format!("unknown case `.{}`", inner.name),
                );
                return Err(Invalid);
            };
            Ok(Some(parsed))
        }
        _ => {
            report(session, value.span, "expected `.None` or `.Some(.Case)`");
            Err(Invalid)
        }
    }
}

fn memory_model(name: &str) -> Option<MemoryModel> {
    match name {
        "Unmanaged" => Some(MemoryModel::Unmanaged),
        "Counted" => Some(MemoryModel::Counted),
        "Affine" => Some(MemoryModel::Affine),
        _ => None,
    }
}

fn convention_kind(name: &str) -> Option<DefaultConvention> {
    match name {
        "Ridl" => Some(DefaultConvention::Ridl),
        "Platform" => Some(DefaultConvention::Platform),
        _ => None,
    }
}

fn string(session: &mut Session, value: &Expr) -> Option<String> {
    if let ExprKind::Str(text) = &value.kind {
        return Some(text.clone());
    }
    report(session, value.span, "expected a string literal");
    None
}

fn strings(session: &mut Session, value: &Expr) -> Option<Vec<String>> {
    let ExprKind::ArrayLit(items) = &value.kind else {
        report(session, value.span, "expected an array of string literals");
        return None;
    };
    items.iter().map(|item| string(session, item)).collect()
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
            format!("Language metadata field `fileModes` {message}"),
        )
        .with_label(span, message.to_owned()),
    );
}
