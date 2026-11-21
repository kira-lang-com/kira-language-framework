//! Traits as Kira construct families and the structs that declare them
//! (KLF-RIDL section 27.7).

use std::fmt::Write as _;

use super::{function, is_trait, parameters, spell};
use crate::module::{Convention, ExtendBody, FunctionDecl, ItemKind, Module, SelfParam, TraitDecl};
use crate::types::{Scalar, StructDecl, Type};

/// A trait as the construct family it is in Kira (KLF-RIDL section 27.7).
/// A method taking nothing but `borrow self` and answering a value is a
/// value member; a method the trait's module exports a default for has no
/// body here, the default being the module's, and the rest are required.
pub(super) fn family(out: &mut String, module: &Module, t: &TraitDecl) {
    if !t.generics.is_empty() || !t.assoc.is_empty() || !t.parents.is_empty() {
        let _ = writeln!(
            out,
            "// skipped trait `{}`: only a trait with methods alone is a family\n",
            t.name
        );
        return;
    }
    let defaulted: Vec<&str> = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Extend(extend) if is_trait(module, &extend.target) => {
                match (&extend.target, &extend.body) {
                    (
                        Type::Nominal { name, .. } | Type::TraitObject(name),
                        ExtendBody::Methods(methods),
                    ) if *name == t.name => Some(
                        methods
                            .iter()
                            .map(|method| method.name.as_str())
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                }
            }
            _ => None,
        })
        .flatten()
        .collect();
    let mut lines = Vec::new();
    for method in &t.methods {
        match family_member(
            &module.name,
            method,
            defaulted.contains(&method.name.as_str()),
        ) {
            Ok(line) => lines.push(line),
            Err(reason) => {
                let _ = writeln!(
                    out,
                    "// skipped trait `{}`: method `{}`: {reason}\n",
                    t.name, method.name
                );
                return;
            }
        }
    }
    let _ = writeln!(out, "construct {} {{\n{}\n}}\n", t.name, lines.join("\n"));
}

fn family_member(module: &str, method: &FunctionDecl, defaulted: bool) -> Result<String, String> {
    if !method.generics.is_empty() {
        return Err("generic methods are not imported".to_owned());
    }
    let result = match &method.result {
        None | Some(Type::Scalar(Scalar::Void)) => None,
        Some(ty) => Some(spell(ty)?),
    };
    match (method.self_param, method.params.is_empty(), &result) {
        (Some(SelfParam::Borrow), true, Some(result)) if !defaulted => {
            return Ok(format!("    @Required let {}: {result}", method.name));
        }
        (None, _, _) => return Err("a trait method takes `self`".to_owned()),
        _ => {}
    }
    // A member that changes its value says so (KLF-RIDL section 27.7); the
    // others borrow it, which Kira leaves unwritten.
    let mut params = Vec::new();
    if method.self_param == Some(SelfParam::BorrowMut) {
        params.push("borrow mut self".to_owned());
    }
    for param in &method.params {
        let text = spell(&param.ty)?;
        params.push(match param.convention {
            Convention::Owned => format!("{}: {text}", param.label),
            Convention::Borrow => format!("{}: borrow {text}", param.label),
            Convention::BorrowMut => format!("{}: borrow mut {text}", param.label),
        });
    }
    let mut marks = Vec::new();
    if defaulted {
        marks.push(format!("@Ridl(\"{module}\")"));
    } else {
        marks.push("@Required".to_owned());
    }
    if method.self_param == Some(SelfParam::Owned) {
        marks.push("@Consuming".to_owned());
    }
    let result = result
        .map(|result| format!(" -> {result}"))
        .unwrap_or_default();
    Ok(format!(
        "    {} function {}({}){result}",
        marks.join(" "),
        method.name,
        params.join(", ")
    ))
}

/// The methods of `extend Family` that are not the trait's own members, as
/// an `extend` of the family whose receiver is implicit, as Kira writes one.
pub(super) fn family_methods(
    out: &mut String,
    module: &Module,
    target: &Type,
    methods: &[FunctionDecl],
) {
    let (Type::Nominal { name, .. } | Type::TraitObject(name)) = target else {
        return;
    };
    let members: Vec<&str> = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Trait(t) if t.name == *name => {
                Some(t.methods.iter().map(|method| method.name.as_str()))
            }
            _ => None,
        })
        .flatten()
        .collect();
    let mut lines = Vec::new();
    for method in methods {
        if members.contains(&method.name.as_str()) {
            continue;
        }
        // A method that changes the value spells its receiver; one that
        // borrows it leaves it implicit, as Kira writes a family's methods.
        let receiver = match method.self_param {
            Some(SelfParam::Borrow) => None,
            Some(SelfParam::BorrowMut) => Some(target),
            _ => {
                lines.push(format!(
                    "    // skipped method `{}`: a family's method borrows its value",
                    method.name
                ));
                continue;
            }
        };
        match function(&module.name, receiver, method) {
            Ok(line) => lines.push(format!("    {line}")),
            Err(reason) => lines.push(format!("    // skipped method `{}`: {reason}", method.name)),
        }
    }
    if lines.is_empty() {
        return;
    }
    let _ = writeln!(out, "extend {name} {{\n{}\n}}\n", lines.join("\n"));
}

/// How a slot of type `ty` holding `family`'s values is spelled: `Any F`
/// holds one and `[Any F]` several, as a construct's `let content: some F`
/// and `let children: [some F]` do. A field holding values of another
/// family, as a link's `value: Any NavigationValue`, is an input.
fn slot(ty: &Type, family: &str) -> Option<String> {
    let ours = |held: &str| held.rsplit('.').next() == family.rsplit('.').next();
    match ty {
        Type::TraitObject(held) if ours(held) => Some(format!("some {held}")),
        Type::Array(element) => match element.as_ref() {
            Type::TraitObject(held) if ours(held) => Some(format!("[some {held}]")),
            _ => None,
        },
        _ => None,
    }
}

/// A struct conforming to a trait as a declaration of the trait's family:
/// its fields are the construct's inputs, with their defaults, except a
/// field holding the family's values with no default, which is a slot the
/// trailing content of a call fills (section 27.7), declared in the body, and
/// one holding them after it, which is a slot with a default.
pub(super) fn member(out: &mut String, s: &StructDecl, family: &str) {
    let Some(fields) = s.fields.as_ref() else {
        let _ = writeln!(
            out,
            "// skipped struct `{}`: a family's declaration has fields\n",
            s.name
        );
        return;
    };
    let mut inputs = Vec::new();
    let mut slots = Vec::new();
    for field in fields {
        if field.default.is_none()
            && let Some(held) = slot(&field.ty, family)
        {
            slots.push(format!("    let {}: {held}", field.name));
            continue;
        }
        // A field after the first slot is declared in the body too, so the
        // construct's fields keep the contract's order, which is its layout.
        // One there holding the family's values is a slot with a default,
        // which a labelled block fills, as `header: { ... }` does.
        let body = !slots.is_empty();
        if body && let (Some(held), Some(default)) = (slot(&field.ty, family), &field.default) {
            slots.push(format!("    let {}: {held} = {default}", field.name));
            continue;
        }
        match spell(&field.ty) {
            Ok(text) if body => slots.push(match &field.default {
                Some(default) => format!("    let {}: {text} = {default}", field.name),
                None => format!("    let {}: {text}", field.name),
            }),
            Ok(text) => inputs.push(match &field.default {
                Some(default) => format!("{}: {text} = {default}", field.name),
                None => format!("{}: {text}", field.name),
            }),
            Err(reason) => {
                let _ = writeln!(
                    out,
                    "// skipped struct `{}`: field `{}`: {reason}\n",
                    s.name, field.name
                );
                return;
            }
        }
    }
    let body = if slots.is_empty() {
        String::new()
    } else {
        format!("\n{}\n", slots.join("\n"))
    };
    let _ = writeln!(
        out,
        "construct {}{}({}) extends {family} {{{body}}}\n",
        s.name,
        parameters(&s.generics),
        inputs.join(", ")
    );
}
