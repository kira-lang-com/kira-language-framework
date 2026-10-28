//! Emission of `.ridl` text and the canonical form used for hashing (24.6).

use crate::module::{
    AnnArg, AnnValue, Annotation, Convention, ExtendBody, FunctionDecl, GenericParam, Item,
    ItemKind, Module, Param, SelfParam, TraitDecl,
};
use crate::types::{EnumDecl, Scalar, StructDecl, Type};

/// Emit readable `.ridl` text for a module's declarations, imports first.
///
/// Every token is separated by whitespace so [`canonical_text`] can normalize
/// by collapsing runs of whitespace.
#[must_use]
pub fn emit(module: &Module) -> String {
    let mut out = String::new();
    for import in &module.imports {
        out.push_str("import ");
        out.push_str(import);
        out.push('\n');
    }
    if !module.imports.is_empty() {
        out.push('\n');
    }
    for (n, item) in module.items.iter().enumerate() {
        if n > 0 {
            out.push('\n');
        }
        emit_item(&mut out, item);
    }
    out
}

/// The canonical text of section 24.6: declarations sorted by path, one space
/// between tokens, no comments, no `@Source`. This is what the metadata
/// sha256 covers.
#[must_use]
pub fn canonical_text(module: &Module) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut imports: Vec<&String> = module.imports.iter().collect();
    imports.sort();
    for import in imports {
        parts.push(collapse_ws(&format!("import {import}")));
    }
    let mut items: Vec<(String, String)> = module
        .items
        .iter()
        .map(|item| {
            // `@Source` points into the providing language's text and says
            // nothing about the ABI (KLF-EDITOR.md section 15.3).
            let mut item = item.clone();
            item.annotations
                .retain(|annotation| annotation.name != "Source");
            let mut s = String::new();
            emit_item(&mut s, &item);
            (item_path(&item), collapse_ws(&s))
        })
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    for (_, text) in items {
        parts.push(text);
    }
    parts.join(" ")
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The sort/identity path of a declaration (its declared name, or the target
/// type for an `extend`).
#[must_use]
pub fn item_path_of(item: &Item) -> String {
    item_path(item)
}

fn item_path(item: &Item) -> String {
    match &item.kind {
        ItemKind::Struct(s) => s.name.clone(),
        ItemKind::Enum(e) => e.name.clone(),
        ItemKind::Trait(t) => t.name.clone(),
        ItemKind::Function(f) => f.name.clone(),
        ItemKind::Alias(a) => a.name.clone(),
        ItemKind::Const(c) => c.name.clone(),
        ItemKind::Extend(e) => format!("{}", TypeDisplay(&e.target)),
    }
}

fn emit_item(out: &mut String, item: &Item) {
    for ann in &item.annotations {
        emit_annotation(out, ann);
        out.push('\n');
    }
    match &item.kind {
        ItemKind::Struct(s) => emit_struct(out, s),
        ItemKind::Enum(e) => emit_enum(out, e),
        ItemKind::Trait(t) => emit_trait(out, t),
        ItemKind::Function(f) => {
            out.push_str("function ");
            emit_function_sig(out, f);
        }
        ItemKind::Extend(e) => emit_extend(out, &e.target, &e.body),
        ItemKind::Alias(a) => {
            out.push_str("type ");
            out.push_str(&a.name);
            out.push_str(" = ");
            emit_type(out, &a.ty);
        }
        ItemKind::Const(c) => {
            out.push_str("let ");
            out.push_str(&c.name);
            out.push_str(" : ");
            emit_type(out, &c.ty);
            out.push_str(" = ");
            out.push_str(&c.literal);
        }
    }
}

fn emit_annotation(out: &mut String, ann: &Annotation) {
    out.push('@');
    out.push_str(&ann.name);
    if ann.args.is_empty() {
        return;
    }
    out.push_str(" ( ");
    for (n, arg) in ann.args.iter().enumerate() {
        if n > 0 {
            out.push_str(" , ");
        }
        emit_ann_arg(out, arg);
    }
    out.push_str(" )");
}

fn emit_ann_arg(out: &mut String, arg: &AnnArg) {
    if let Some(label) = &arg.label {
        out.push_str(label);
        out.push_str(" : ");
    }
    match &arg.value {
        AnnValue::String(s) => {
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        AnnValue::Ident(i) => out.push_str(i),
        AnnValue::Type(t) => emit_type(out, t),
    }
}

fn emit_generics(out: &mut String, generics: &[GenericParam]) {
    if generics.is_empty() {
        return;
    }
    out.push_str(" < ");
    for (n, g) in generics.iter().enumerate() {
        if n > 0 {
            out.push_str(" , ");
        }
        out.push_str(&g.name);
        if !g.bounds.is_empty() {
            out.push_str(" : ");
            for (m, b) in g.bounds.iter().enumerate() {
                if m > 0 {
                    out.push_str(" , ");
                }
                emit_type(out, b);
            }
        }
    }
    out.push_str(" >");
}

fn emit_generic_names(out: &mut String, names: &[String]) {
    if names.is_empty() {
        return;
    }
    out.push_str(" < ");
    out.push_str(&names.join(" , "));
    out.push_str(" >");
}

fn emit_struct(out: &mut String, s: &StructDecl) {
    out.push_str("struct ");
    out.push_str(&s.name);
    emit_generic_names(out, &s.generics);
    let Some(fields) = &s.fields else {
        return;
    };
    out.push_str(" {\n");
    for f in fields {
        out.push_str("    let ");
        out.push_str(&f.name);
        out.push_str(" : ");
        emit_type(out, &f.ty);
        if let Some(default) = &f.default {
            out.push_str(" = ");
            out.push_str(default);
        }
        out.push('\n');
    }
    out.push('}');
}

fn emit_enum(out: &mut String, e: &EnumDecl) {
    out.push_str("enum ");
    out.push_str(&e.name);
    emit_generic_names(out, &e.generics);
    out.push_str(" {\n");
    for c in &e.cases {
        out.push_str("    ");
        out.push_str(&c.name);
        if !c.payload.is_empty() {
            out.push_str(" ( ");
            for (n, p) in c.payload.iter().enumerate() {
                if n > 0 {
                    out.push_str(" , ");
                }
                emit_type(out, p);
            }
            out.push_str(" )");
        }
        out.push('\n');
    }
    out.push('}');
}

fn emit_trait(out: &mut String, t: &TraitDecl) {
    out.push_str("trait ");
    out.push_str(&t.name);
    emit_generics(out, &t.generics);
    if !t.parents.is_empty() {
        out.push_str(" : ");
        for (n, p) in t.parents.iter().enumerate() {
            if n > 0 {
                out.push_str(" , ");
            }
            emit_type(out, p);
        }
    }
    out.push_str(" {\n");
    for a in &t.assoc {
        out.push_str("    type ");
        out.push_str(&a.name);
        if let Some(b) = &a.bound {
            out.push_str(" : ");
            emit_type(out, b);
        }
        out.push('\n');
    }
    for m in &t.methods {
        out.push_str("    function ");
        emit_function_sig(out, m);
        out.push('\n');
    }
    out.push('}');
}

fn emit_extend(out: &mut String, target: &Type, body: &ExtendBody) {
    out.push_str("extend ");
    emit_type(out, target);
    match body {
        ExtendBody::Conformance(traits) => {
            out.push_str(" : ");
            for (n, tr) in traits.iter().enumerate() {
                if n > 0 {
                    out.push_str(" , ");
                }
                emit_type(out, tr);
            }
        }
        ExtendBody::Methods(methods) => {
            out.push_str(" {\n");
            for m in methods {
                out.push_str("    function ");
                emit_function_sig(out, m);
                out.push('\n');
            }
            out.push('}');
        }
    }
}

fn emit_function_sig(out: &mut String, f: &FunctionDecl) {
    out.push_str(&f.name);
    emit_generics(out, &f.generics);
    out.push_str(" ( ");
    let mut first = true;
    if let Some(sp) = f.self_param {
        match sp {
            SelfParam::Owned => out.push_str("self"),
            SelfParam::Borrow => out.push_str("borrow self"),
            SelfParam::BorrowMut => out.push_str("borrow mut self"),
        }
        first = false;
    }
    for p in &f.params {
        if !first {
            out.push_str(" , ");
        }
        first = false;
        emit_param(out, p);
    }
    out.push_str(" )");
    if let Some(result) = &f.result {
        out.push_str(" -> ");
        emit_type(out, result);
    }
}

fn emit_param(out: &mut String, p: &Param) {
    out.push_str(&p.label);
    out.push_str(" : ");
    match p.convention {
        Convention::Owned => {}
        Convention::Borrow => out.push_str("borrow "),
        Convention::BorrowMut => out.push_str("borrow mut "),
    }
    emit_type(out, &p.ty);
    if let Some(default) = &p.default {
        out.push_str(" = ");
        out.push_str(default);
    }
}

fn emit_type(out: &mut String, ty: &Type) {
    use std::fmt::Write as _;
    let _ = write!(out, "{}", TypeDisplay(ty));
}

/// `ty` as `.ridl` spells it.
#[must_use]
pub fn type_text(ty: &Type) -> String {
    TypeDisplay(ty).to_string()
}

/// `ty` with no whitespace: the key two spellings of one type share.
#[must_use]
pub fn type_key(ty: &Type) -> String {
    type_text(ty)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

struct TypeDisplay<'a>(&'a Type);

impl std::fmt::Display for TypeDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Type::Scalar(s) => f.write_str(scalar_name(*s)),
            Type::String => f.write_str("String"),
            Type::Array(inner) => write!(f, "[ {} ]", TypeDisplay(inner)),
            Type::Optional(inner) => write!(f, "Optional < {} >", TypeDisplay(inner)),
            Type::Result(ok, err) => {
                write!(f, "Result < {} , {} >", TypeDisplay(ok), TypeDisplay(err))
            }
            Type::Tuple(items) => {
                f.write_str("(")?;
                for (n, t) in items.iter().enumerate() {
                    if n > 0 {
                        f.write_str(" ,")?;
                    }
                    write!(f, " {}", TypeDisplay(t))?;
                }
                f.write_str(" )")
            }
            Type::Nominal { name, args } => {
                f.write_str(name)?;
                if !args.is_empty() {
                    f.write_str(" < ")?;
                    for (n, a) in args.iter().enumerate() {
                        if n > 0 {
                            f.write_str(" , ")?;
                        }
                        write!(f, "{}", TypeDisplay(a))?;
                    }
                    f.write_str(" >")?;
                }
                Ok(())
            }
            Type::Function { params, result } => write_fn(f, "", params, result),
            Type::BorrowFunction { params, result } => write_fn(f, "borrow ", params, result),
            Type::TraitObject(name) => write!(f, "Any {name}"),
            Type::Generic(name) => f.write_str(name),
        }
    }
}

fn write_fn(
    f: &mut std::fmt::Formatter<'_>,
    prefix: &str,
    params: &[Type],
    result: &Type,
) -> std::fmt::Result {
    f.write_str(prefix)?;
    f.write_str("(")?;
    for (n, t) in params.iter().enumerate() {
        if n > 0 {
            f.write_str(" ,")?;
        }
        write!(f, " {}", TypeDisplay(t))?;
    }
    write!(f, " ) -> {}", TypeDisplay(result))
}

fn scalar_name(s: Scalar) -> &'static str {
    match s {
        Scalar::I8 => "I8",
        Scalar::I16 => "I16",
        Scalar::I32 => "I32",
        Scalar::I64 => "I64",
        Scalar::I128 => "I128",
        Scalar::U8 => "U8",
        Scalar::U16 => "U16",
        Scalar::U32 => "U32",
        Scalar::U64 => "U64",
        Scalar::U128 => "U128",
        Scalar::F32 => "F32",
        Scalar::F64 => "F64",
        Scalar::Bool => "Bool",
        Scalar::ISize => "ISize",
        Scalar::USize => "USize",
        Scalar::RawPtr => "RawPtr",
        Scalar::Void => "Void",
    }
}
