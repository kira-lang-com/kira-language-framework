//! RIDL symbol mangling (section 24.2).
//!
//! `symbol = "_RIDL1" path kind [ "_v" abitag ]`, `component = length identifier`.

use crate::types::{Scalar, Type};

const PREFIX: &str = "_RIDL1";

/// One path component, spelled as written.
fn component(ident: &str) -> String {
    format!("{}{}", ident.len(), ident)
}

/// A type's path component. A program spells a type that shares its name with
/// a type of another module it sees `Name#module`, by an index of its own
/// build; the symbol names the type by its RIDL identifier alone, which never
/// holds a `#`, so that the module declaring the type and every module using it
/// agree. The module component before it already tells the two apart. A
/// function's name keeps its `#`: there it tells apart the functions a build
/// makes for one type, such as its drop and its copy.
fn type_component(ident: &str) -> String {
    component(ident.split_once('#').map_or(ident, |(name, _)| name))
}

fn path(components: &[&str]) -> String {
    components.iter().map(|c| component(c)).collect()
}

/// `<module> <type>`, the path of a type.
fn type_path(module: &str, type_name: &str) -> String {
    format!("{}{}", component(module), type_component(type_name))
}

fn abitag(symbol: &mut String, tag: Option<&str>) {
    if let Some(tag) = tag {
        symbol.push_str("_v");
        symbol.push_str(tag);
    }
}

/// `_RIDL1 <module> <name> F [_v tag]`
#[must_use]
pub fn function_symbol(module: &str, name: &str, tag: Option<&str>) -> String {
    let mut s = format!("{PREFIX}{}F", path(&[module, name]));
    abitag(&mut s, tag);
    s
}

/// `_RIDL1 <module> <type> <method> M [_v tag]`
#[must_use]
pub fn method_symbol(module: &str, type_name: &str, method: &str, tag: Option<&str>) -> String {
    let mut s = format!(
        "{PREFIX}{}{}M",
        type_path(module, type_name),
        component(method)
    );
    abitag(&mut s, tag);
    s
}

/// `_RIDL1 <module> <type> D`
#[must_use]
pub fn descriptor_symbol(module: &str, type_name: &str) -> String {
    format!("{PREFIX}{}D", type_path(module, type_name))
}

/// `_RIDL1 <module> <type> B`
#[must_use]
pub fn builder_symbol(module: &str, type_name: &str) -> String {
    format!("{PREFIX}{}B", type_path(module, type_name))
}

/// `_RIDL1 <module> <type> Z`
#[must_use]
pub fn size_symbol(module: &str, type_name: &str) -> String {
    format!("{PREFIX}{}Z", type_path(module, type_name))
}

/// `_RIDL1 <type-path> W <trait-path>`
#[must_use]
pub fn witness_symbol(
    type_module: &str,
    type_name: &str,
    trait_module: &str,
    trait_name: &str,
) -> String {
    format!(
        "{PREFIX}{}W{}",
        type_path(type_module, type_name),
        type_path(trait_module, trait_name)
    )
}

/// `_RIDL1 <module> <name> I <typelist> E [_v tag]`
#[must_use]
pub fn instance_symbol(module: &str, name: &str, args: &[Type], tag: Option<&str>) -> String {
    let mut list = String::new();
    for a in args {
        list.push_str(&type_code(a));
    }
    let mut s = format!("{PREFIX}{}I{list}E", path(&[module, name]));
    abitag(&mut s, tag);
    s
}

/// `_RIDL1 <module> <type> <method> I <typelist> E [_v tag]`: a method of
/// the named instance of `type_name` over `args` (section 23.5).
#[must_use]
pub fn instance_method_symbol(
    module: &str,
    type_name: &str,
    method: &str,
    args: &[Type],
    tag: Option<&str>,
) -> String {
    let mut s = format!(
        "{PREFIX}{}{}I{}E",
        type_path(module, type_name),
        component(method),
        args.iter().map(type_code).collect::<String>()
    );
    abitag(&mut s, tag);
    s
}

/// The type code of section 24.2, used inside `I` instance symbols.
#[must_use]
pub fn type_code(ty: &Type) -> String {
    match ty {
        Type::Scalar(s) => scalar_code(*s).to_owned(),
        Type::String => "s".to_owned(),
        Type::Array(inner) => format!("A{}", type_code(inner)),
        Type::Optional(inner) => format!("O{}", type_code(inner)),
        Type::Result(ok, err) => format!("R{}{}", type_code(ok), type_code(err)),
        Type::Tuple(items) => {
            let mut s = String::from("T");
            for i in items {
                s.push_str(&type_code(i));
            }
            s.push('E');
            s
        }
        Type::Nominal { name, args } => {
            let mut s = format!("N{}", type_component(name));
            if !args.is_empty() {
                s.push('I');
                for a in args {
                    s.push_str(&type_code(a));
                }
                s.push('E');
            }
            s.push('E');
            s
        }
        Type::TraitObject(name) => format!("N{}E", type_component(name)),
        Type::Function { .. } | Type::BorrowFunction { .. } | Type::Generic(_) => "p".to_owned(),
    }
}

fn scalar_code(s: Scalar) -> &'static str {
    match s {
        Scalar::I8 => "a",
        Scalar::I16 => "b",
        Scalar::I32 => "c",
        Scalar::I64 => "d",
        Scalar::I128 => "e",
        Scalar::U8 => "h",
        Scalar::U16 => "i",
        Scalar::U32 => "j",
        Scalar::U64 => "k",
        Scalar::U128 => "l",
        Scalar::F32 => "f",
        Scalar::F64 => "g",
        Scalar::Bool => "o",
        Scalar::ISize => "v",
        Scalar::USize => "w",
        Scalar::RawPtr => "p",
        Scalar::Void => "V",
    }
}
