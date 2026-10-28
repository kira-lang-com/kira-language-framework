//! Property wrappers (`KLF-CLI.md` section 2).
//!
//! `@PropertyWrapper struct W<Value> { let wrappedValue: Value ... }` declares
//! a wrapper template. A stored property written `@W(label: v) let x: T = e`
//! becomes a backing field `_x` of the instance `W(T)`, built from the
//! annotation's labelled values, the template's field defaults for the rest,
//! and `wrappedValue` from `e` or from the struct literal that sets `x`.
//! Reading or writing `x` goes through `_x.wrappedValue`.
//!
//! This runs on the syntax tree before elaboration: templates leave the file,
//! one struct per instance enters it, and the owner's member is retyped. What
//! the lowerer needs to route `x` through `_x` is the [`WrappedTable`].

use crate::generics::{substitute_function, substitute_type};
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{
    AnnotationArgs, Decl, DeclKind, Expr, FieldInit, File, Ident, Member, MemberKind, Path,
    StructDecl, Type, TypeKind,
};
use klf_services::Session;
use klf_source::Span;
use std::collections::HashMap;

/// One wrapped stored property.
#[derive(Clone, Debug)]
pub(crate) struct Wrapped {
    pub property: Name,
    pub backing: Name,
    /// The wrapper instance the backing field holds.
    pub wrapper: Type,
    /// Every wrapper field but `wrappedValue`, with its value.
    pub fields: Vec<(Ident, Expr)>,
    /// The property's initial value, used when a struct literal omits it.
    pub init: Option<Expr>,
    pub span: Span,
}

/// The wrapped properties of each struct.
pub(crate) type WrappedTable = HashMap<Name, Vec<Wrapped>>;

pub(crate) const PROPERTY_WRAPPER: &str = "PropertyWrapper";

/// The backing field name of property `name`.
pub(crate) fn backing_name(name: &Name) -> Name {
    Name::new(format!("_{name}"))
}

/// Whether `decl` is a `@PropertyWrapper` struct.
pub(crate) fn is_template(decl: &Decl) -> bool {
    matches!(decl.kind, DeclKind::Struct(_))
        && decl
            .annotations
            .iter()
            .any(|annotation| annotation_name(&annotation.path) == Some(PROPERTY_WRAPPER))
}

pub(crate) fn annotation_name(path: &Path) -> Option<&str> {
    match path.segments.as_slice() {
        [segment] => Some(segment.name.as_str()),
        _ => None,
    }
}

/// Rewrites `file` so no wrapper template or wrapped property remains.
pub(crate) fn desugar(session: &mut Session, file: File) -> (File, WrappedTable) {
    let mut templates: HashMap<Name, StructDecl> = HashMap::new();
    let mut decls = Vec::with_capacity(file.decls.len());
    for decl in file.decls {
        if is_template(&decl) {
            let DeclKind::Struct(template) = decl.kind else {
                continue;
            };
            if validate_template(session, &template) {
                templates.insert(template.name.name.clone(), template);
            }
        } else {
            decls.push(decl);
        }
    }
    let mut table = WrappedTable::new();
    let mut instances: HashMap<String, StructDecl> = HashMap::new();
    for decl in &mut decls {
        let DeclKind::Struct(owner) = &mut decl.kind else {
            continue;
        };
        let mut wrapped = Vec::new();
        for member in &mut owner.members {
            if let Some(entry) = wrap_member(session, member, &templates, &mut instances) {
                wrapped.push(entry);
            }
        }
        if !wrapped.is_empty() {
            table.insert(owner.name.name.clone(), wrapped);
        }
    }
    let mut instances: Vec<StructDecl> = instances.into_values().collect();
    instances.sort_by(|a, b| a.name.name.as_str().cmp(b.name.name.as_str()));
    decls.extend(instances.into_iter().map(|instance| Decl {
        annotations: Vec::new(),
        span: instance.span,
        kind: DeclKind::Struct(instance),
    }));
    (
        File {
            imports: file.imports,
            decls,
        },
        table,
    )
}

fn validate_template(session: &mut Session, template: &StructDecl) -> bool {
    let has_value = template.members.iter().any(|member| {
        matches!(&member.kind, MemberKind::Field { name, .. } if name.name.as_str() == "wrappedValue")
    });
    if !has_value {
        session.report(
            Diagnostic::error_at(
                "KLFK-E0220",
                template.name.span,
                format!(
                    "property wrapper `{}` has no `wrappedValue` field",
                    template.name.name
                ),
            )
            .with_label(template.span, "declare `let wrappedValue: Value`"),
        );
        return false;
    }
    if template.generics.len() > 1 {
        session.report(
            Diagnostic::error_at(
                "KLFK-E0220",
                template.name.span,
                "a property wrapper takes at most one type parameter, the wrapped type",
            )
            .with_label(template.span, "remove the extra parameters"),
        );
        return false;
    }
    true
}

/// Retypes a wrapped member to its backing field and answers what the
/// lowerer needs to route the property through it. `None` for a member no
/// wrapper annotates.
fn wrap_member(
    session: &mut Session,
    member: &mut Member,
    templates: &HashMap<Name, StructDecl>,
    instances: &mut HashMap<String, StructDecl>,
) -> Option<Wrapped> {
    let position = member.annotations.iter().position(|annotation| {
        annotation_name(&annotation.path)
            .is_some_and(|name| templates.contains_key(&Name::new(name)))
    })?;
    let annotation = member.annotations.remove(position);
    let template = &templates[&Name::new(annotation_name(&annotation.path)?)];
    let MemberKind::Field { name, ty, init, .. } = &mut member.kind else {
        session.report(
            Diagnostic::error_at(
                "KLFK-E0221",
                annotation.span,
                format!("`@{}` applies to stored properties", template.name.name),
            )
            .with_label(member.span, "not a stored property"),
        );
        return None;
    };
    let init = match init.take() {
        Some(FieldInit::Expr(value)) => Some(value),
        Some(FieldInit::Block(_)) => {
            session.report(
                Diagnostic::error_at(
                    "KLFK-E0221",
                    member.span,
                    "a wrapped property cannot be computed",
                )
                .with_label(member.span, "give it a stored initial value"),
            );
            return None;
        }
        None => None,
    };
    let given: Vec<(Ident, Expr)> = match annotation.args {
        AnnotationArgs::None => Vec::new(),
        AnnotationArgs::Fields(pairs) => pairs,
        AnnotationArgs::Call(args) => {
            let mut pairs = Vec::new();
            for arg in args {
                let Some(label) = arg.label else {
                    session.report(
                        Diagnostic::error_at(
                            "KLFK-E0221",
                            arg.value.span,
                            "property wrapper arguments are labelled with the wrapper's fields",
                        )
                        .with_label(arg.value.span, "add `field:`"),
                    );
                    return None;
                };
                pairs.push((label, arg.value));
            }
            pairs
        }
    };
    let fields = wrapper_fields(session, template, given, annotation.span)?;
    let instance = instance_name(&template.name.name, ty);
    let wrapper = named_type(&instance, ty.span);
    instances
        .entry(instance.clone())
        .or_insert_with(|| instantiate(template, ty, &instance));
    let property = name.name.clone();
    let backing = backing_name(&property);
    *name = Ident {
        name: backing.clone(),
        span: name.span,
    };
    *ty = wrapper.clone();
    Some(Wrapped {
        property,
        backing,
        wrapper,
        fields,
        init,
        span: member.span,
    })
}

/// The values of every wrapper field but `wrappedValue`: those the
/// annotation gives, else the template's defaults.
fn wrapper_fields(
    session: &mut Session,
    template: &StructDecl,
    mut given: Vec<(Ident, Expr)>,
    span: Span,
) -> Option<Vec<(Ident, Expr)>> {
    let mut fields = Vec::new();
    for member in &template.members {
        let MemberKind::Field { name, init, .. } = &member.kind else {
            continue;
        };
        if name.name.as_str() == "wrappedValue" {
            continue;
        }
        if let Some(index) = given.iter().position(|(label, _)| label.name == name.name) {
            fields.push(given.remove(index));
        } else if let Some(FieldInit::Expr(default)) = init {
            fields.push((name.clone(), default.clone()));
        } else {
            session.report(
                Diagnostic::error_at(
                    "KLFK-E0221",
                    span,
                    format!("`@{}` needs `{}`", template.name.name, name.name),
                )
                .with_label(span, "the wrapper field has no default"),
            );
            return None;
        }
    }
    if let Some((label, _)) = given.first() {
        session.report(
            Diagnostic::error_at(
                "KLFK-E0221",
                label.span,
                format!("`@{}` has no field `{}`", template.name.name, label.name),
            )
            .with_label(label.span, "unknown wrapper field"),
        );
        return None;
    }
    Some(fields)
}

/// `W(T)`, with generic arguments in parentheses too: KIR's text form reads
/// `<` in a nominal name as type arguments.
fn instance_name(wrapper: &Name, ty: &Type) -> String {
    let argument: String = type_text(ty)
        .chars()
        .filter(|c| *c != ' ')
        .map(|c| match c {
            '<' => '(',
            '>' => ')',
            other => other,
        })
        .collect();
    format!("{wrapper}({argument})")
}

/// `ty` as KLF Kira source.
pub(crate) fn type_text(ty: &Type) -> String {
    match &ty.kind {
        TypeKind::Named { path, args } => {
            let base: Vec<&str> = path.segments.iter().map(|s| s.name.as_str()).collect();
            if args.is_empty() {
                base.join(".")
            } else {
                let args: Vec<String> = args.iter().map(type_text).collect();
                format!("{}<{}>", base.join("."), args.join(", "))
            }
        }
        TypeKind::Array(element) => format!("[{}]", type_text(element)),
        TypeKind::Tuple(elements) => {
            let elements: Vec<String> = elements.iter().map(type_text).collect();
            format!("({})", elements.join(", "))
        }
        TypeKind::Function { params, ret } => {
            let params: Vec<String> = params.iter().map(type_text).collect();
            format!("({}) -> {}", params.join(", "), type_text(ret))
        }
        TypeKind::Any(inner) => format!("Any {}", type_text(inner)),
        TypeKind::Some(inner) => format!("some {}", type_text(inner)),
    }
}

fn named_type(name: &str, span: Span) -> Type {
    Type {
        kind: TypeKind::Named {
            path: Path {
                segments: vec![Ident {
                    name: Name::new(name),
                    span,
                }],
                span,
            },
            args: Vec::new(),
        },
        span,
    }
}

/// The template with its type parameter replaced by `ty`, named `name`.
fn instantiate(template: &StructDecl, ty: &Type, name: &str) -> StructDecl {
    let arguments: HashMap<Name, Type> = template
        .generics
        .iter()
        .map(|generic| (generic.name.name.clone(), ty.clone()))
        .collect();
    let mut instance = template.clone();
    instance.name = Ident {
        name: Name::new(name),
        span: template.name.span,
    };
    instance.generics.clear();
    for member in &mut instance.members {
        match &mut member.kind {
            MemberKind::Field { ty, .. } => substitute_type(ty, &arguments),
            MemberKind::Method(function) => substitute_function(function, &arguments),
            MemberKind::Init { .. } => {}
        }
    }
    instance
}
