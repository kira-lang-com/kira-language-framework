//! The members only a `ShaderLanguage` declares (`KLF-KSL.md` section 5.2):
//! `layouts`, `targets` and `libraryRoot`.

use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{Expr, ExprKind};
use klf_services::{Session, ShaderFamily};
use klf_shader::builder::LayoutDefaults;
use klf_shader::{Capability, LayoutRule, Target};
use std::collections::HashMap;

fn report(session: &mut Session, value: &Expr, message: String) {
    session.report(
        Diagnostic::error_at("KLF-DEF-0002", value.span, message)
            .with_label(value.span, "in the ShaderLanguage metadata"),
    );
}

/// The case name of a `.Case` literal.
fn case<'a>(session: &mut Session, value: &'a Expr, field: &str) -> Option<&'a str> {
    if let ExprKind::EnumLit { name, args: None } = &value.kind {
        return Some(name.name.as_str());
    }
    report(session, value, format!("`{field}` holds `.Case` values"));
    None
}

fn rule(session: &mut Session, value: &Expr, field: &str) -> Option<LayoutRule> {
    let rule = match case(session, value, field)? {
        "Std140" => LayoutRule::Std140,
        "Std430" => LayoutRule::Std430,
        "Scalar" => LayoutRule::Scalar,
        "Msl" => LayoutRule::Msl,
        "Explicit" => LayoutRule::Explicit,
        other => {
            report(session, value, format!("unknown LayoutRule `.{other}`"));
            return None;
        }
    };
    Some(rule)
}

/// `LayoutDefaults { uniform = .R, storage = .R, immediate = .R }`.
fn layouts(session: &mut Session, value: &Expr) -> Option<LayoutDefaults> {
    let ExprKind::StructLit { fields, .. } = &value.kind else {
        report(
            session,
            value,
            "`layouts` is a `LayoutDefaults { ... }`".to_owned(),
        );
        return None;
    };
    let mut found = [None; 3];
    for (name, field) in fields {
        let Some(slot) = ["uniform", "storage", "immediate"]
            .iter()
            .position(|known| *known == name.name.as_str())
        else {
            report(
                session,
                field,
                format!("`LayoutDefaults` has no `{}`", name.name),
            );
            return None;
        };
        found[slot] = Some(rule(session, field, name.name.as_str())?);
    }
    let [Some(uniform), Some(storage), Some(immediate)] = found else {
        report(
            session,
            value,
            "`layouts` names uniform, storage and immediate".to_owned(),
        );
        return None;
    };
    Some(LayoutDefaults {
        uniform,
        storage,
        immediate,
    })
}

fn targets(session: &mut Session, value: Option<&Expr>) -> Option<Vec<Target>> {
    let Some(value) = value else {
        return Some(Target::ALL.to_vec());
    };
    let ExprKind::ArrayLit(items) = &value.kind else {
        report(
            session,
            value,
            "`targets` is an array of `ShaderTarget`".to_owned(),
        );
        return None;
    };
    let mut targets = Vec::new();
    for item in items {
        let target = match case(session, item, "targets")? {
            "Msl" => Target::Msl,
            "Spirv" => Target::Spirv,
            "Hlsl" => Target::Hlsl,
            "Glsl430" => Target::Glsl,
            "Wgsl" => Target::Wgsl,
            "Ksl" => Target::Ksl,
            other => {
                report(session, item, format!("unknown ShaderTarget `.{other}`"));
                return None;
            }
        };
        targets.push(target);
    }
    Some(targets)
}

/// Reads the shader members from a `ShaderLanguage`'s fields.
pub(crate) fn shader_family(
    session: &mut Session,
    fields: &HashMap<&str, &Expr>,
) -> Option<ShaderFamily> {
    let Some(layouts_value) = fields.get("layouts").copied() else {
        session.report(Diagnostic::error(
            "KLF-DEF-0002",
            "ShaderLanguage metadata is missing required field `layouts`",
        ));
        return None;
    };
    let layouts = layouts(session, layouts_value)?;
    let targets = targets(session, fields.get("targets").copied())?;
    let library_root = match fields.get("libraryRoot").copied() {
        None => String::new(),
        Some(Expr {
            kind: ExprKind::Str(root),
            ..
        }) => root.clone(),
        Some(other) => {
            report(session, other, "`libraryRoot` is a string".to_owned());
            return None;
        }
    };
    let keys = keys(session, fields.get("keys").copied())?;
    let capabilities = capabilities(session, fields.get("capabilities").copied())?;
    Some(ShaderFamily {
        layouts,
        targets,
        library_root,
        keys,
        capabilities,
    })
}

/// `[.StorageImageReadWrite, ...]`: each case is a capability's name in
/// upper camel case. Absent, the language declares none.
fn capabilities(session: &mut Session, value: Option<&Expr>) -> Option<Vec<Capability>> {
    let Some(value) = value else {
        return Some(Vec::new());
    };
    let ExprKind::ArrayLit(items) = &value.kind else {
        report(
            session,
            value,
            "`capabilities` is an array of `ShaderCapability`".to_owned(),
        );
        return None;
    };
    let mut capabilities = Vec::new();
    for item in items {
        let written = case(session, item, "capabilities")?;
        let Some(capability) = Capability::ALL
            .into_iter()
            .find(|capability| camel_case(capability.name()) == written)
        else {
            report(
                session,
                item,
                format!("unknown ShaderCapability `.{written}`"),
            );
            return None;
        };
        capabilities.push(capability);
    }
    Some(capabilities)
}

/// `storage-image-read-write` as `StorageImageReadWrite`.
fn camel_case(name: &str) -> String {
    name.split('-')
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + letters.as_str()
            })
        })
        .collect()
}

/// `[DiagnosticKey { key = "shader.uniformity", code = 2019 }, ...]`, which
/// must map every kernel key (section 16.4).
fn keys(session: &mut Session, value: Option<&Expr>) -> Option<Vec<(String, i64)>> {
    let Some(value) = value else {
        session.report(Diagnostic::error(
            "KLF-DEF-0002",
            "ShaderLanguage metadata is missing required field `keys`",
        ));
        return None;
    };
    let ExprKind::ArrayLit(items) = &value.kind else {
        report(
            session,
            value,
            "`keys` is an array of `DiagnosticKey`".to_owned(),
        );
        return None;
    };
    let mut keys = Vec::new();
    for item in items {
        let ExprKind::StructLit { fields, .. } = &item.kind else {
            report(session, item, "a `DiagnosticKey { key, code }`".to_owned());
            return None;
        };
        let mut key = None;
        let mut code = None;
        for (name, field) in fields {
            match (name.name.as_str(), &field.kind) {
                ("key", ExprKind::Str(text)) => key = Some(text.clone()),
                ("code", ExprKind::Int(text)) => code = text.as_str().parse::<i64>().ok(),
                _ => {
                    report(
                        session,
                        field,
                        format!("`DiagnosticKey.{}` is not this", name.name),
                    );
                    return None;
                }
            }
        }
        let (Some(key), Some(code)) = (key, code) else {
            report(
                session,
                item,
                "a `DiagnosticKey` names a key and a code".to_owned(),
            );
            return None;
        };
        if !klf_services::SHADER_KEYS.contains(&key.as_str()) {
            report(
                session,
                item,
                format!("`{key}` is not a shader diagnostic key"),
            );
            return None;
        }
        keys.push((key, code));
    }
    for key in klf_services::SHADER_KEYS {
        if !keys.iter().any(|(known, _)| known == key) {
            report(session, value, format!("`keys` does not map `{key}`"));
            return None;
        }
    }
    Some(keys)
}
