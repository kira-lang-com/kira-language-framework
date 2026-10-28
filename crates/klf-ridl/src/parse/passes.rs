//! What the parser settles after reading the whole module: `@Open` enums,
//! generic parameter names, and aliases.

use std::collections::BTreeMap;

use crate::module::{ExtendBody, FunctionDecl, ItemKind, Module};
use crate::types::Type;

/// An alias has no ABI effect (section 20.3): every type that names one is
/// the type it stands for, so layout and the calling convention see the
/// target. The alias item stays, for a reader that writes the name back.
pub(super) fn expand_aliases(module: &mut Module) {
    let aliases: BTreeMap<String, Type> = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Alias(alias) => Some((alias.name.clone(), alias.ty.clone())),
            _ => None,
        })
        .collect();
    if aliases.is_empty() {
        return;
    }
    let expand = |ty: &mut Type| *ty = expanded(ty, &aliases, 0);
    let function = |f: &mut FunctionDecl| {
        for param in &mut f.params {
            expand(&mut param.ty);
        }
        if let Some(result) = &mut f.result {
            expand(result);
        }
    };
    for item in &mut module.items {
        match &mut item.kind {
            ItemKind::Struct(s) => {
                for field in s.fields.iter_mut().flatten() {
                    expand(&mut field.ty);
                }
            }
            ItemKind::Enum(e) => {
                for case in &mut e.cases {
                    case.payload.iter_mut().for_each(expand);
                }
            }
            ItemKind::Trait(t) => {
                t.parents.iter_mut().for_each(expand);
                t.methods.iter_mut().for_each(function);
            }
            ItemKind::Function(f) => function(f),
            ItemKind::Extend(e) => {
                expand(&mut e.target);
                match &mut e.body {
                    ExtendBody::Conformance(traits) => traits.iter_mut().for_each(expand),
                    ExtendBody::Methods(methods) => methods.iter_mut().for_each(function),
                }
            }
            ItemKind::Alias(alias) => expand(&mut alias.ty),
            ItemKind::Const(c) => expand(&mut c.ty),
        }
    }
}

/// `ty` with every alias it names replaced by its target. `depth` stops an
/// alias cycle at the number of aliases there are.
fn expanded(ty: &Type, aliases: &BTreeMap<String, Type>, depth: usize) -> Type {
    let inner = |t: &Type| expanded(t, aliases, depth);
    match ty {
        Type::Nominal { name, args } if args.is_empty() && depth < aliases.len() => {
            match aliases.get(name) {
                Some(target) => expanded(target, aliases, depth + 1),
                None => ty.clone(),
            }
        }
        Type::Nominal { name, args } => Type::Nominal {
            name: name.clone(),
            args: args.iter().map(inner).collect(),
        },
        Type::Array(item) => Type::Array(Box::new(inner(item))),
        Type::Optional(item) => Type::Optional(Box::new(inner(item))),
        Type::Result(ok, err) => Type::Result(Box::new(inner(ok)), Box::new(inner(err))),
        Type::Tuple(items) => Type::Tuple(items.iter().map(inner).collect()),
        Type::Function { params, result } => Type::Function {
            params: params.iter().map(inner).collect(),
            result: Box::new(inner(result)),
        },
        Type::BorrowFunction { params, result } => Type::BorrowFunction {
            params: params.iter().map(inner).collect(),
            result: Box::new(inner(result)),
        },
        Type::Scalar(_) | Type::String | Type::TraitObject(_) | Type::Generic(_) => ty.clone(),
    }
}

/// A name a declaration binds as a type parameter is that parameter in the
/// declaration, not a nominal type (section 20.2): a field `let value: Value`
/// of `struct Holder<Value>` is substituted when `Holder<Number>` is laid
/// out. An `extend` of a generic declaration binds its parameters for its
/// methods, whether it writes them (`extend Holder<Value>`) or leaves them
/// out (`extend Holder`), and the target is then written with them.
pub(super) fn resolve_generics(module: &mut Module) {
    let declared: BTreeMap<String, Vec<String>> = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Struct(s) if !s.generics.is_empty() => {
                Some((s.name.clone(), s.generics.clone()))
            }
            ItemKind::Enum(e) if !e.generics.is_empty() => {
                Some((e.name.clone(), e.generics.clone()))
            }
            _ => None,
        })
        .collect();
    let function = |f: &mut FunctionDecl, outer: &[String]| {
        let mut names = outer.to_vec();
        names.extend(f.generics.iter().map(|g| g.name.clone()));
        for param in &mut f.params {
            param.ty = parameterized(&param.ty, &names);
        }
        if let Some(result) = &mut f.result {
            *result = parameterized(result, &names);
        }
        for generic in &mut f.generics {
            for bound in &mut generic.bounds {
                *bound = parameterized(bound, &names);
            }
        }
    };
    for item in &mut module.items {
        match &mut item.kind {
            ItemKind::Struct(s) => {
                let names = s.generics.clone();
                for field in s.fields.iter_mut().flatten() {
                    field.ty = parameterized(&field.ty, &names);
                }
            }
            ItemKind::Enum(e) => {
                let names = e.generics.clone();
                for case in &mut e.cases {
                    for ty in &mut case.payload {
                        *ty = parameterized(ty, &names);
                    }
                }
            }
            ItemKind::Trait(t) => {
                let names: Vec<String> = t.generics.iter().map(|g| g.name.clone()).collect();
                for method in &mut t.methods {
                    function(method, &names);
                }
            }
            ItemKind::Function(f) => function(f, &[]),
            ItemKind::Extend(e) => {
                let names = match &mut e.target {
                    Type::Nominal { name, args } => match declared.get(name.as_str()) {
                        Some(params) if args.is_empty() => {
                            *args = params.iter().cloned().map(Type::Generic).collect();
                            params.clone()
                        }
                        Some(params) => {
                            let named: Vec<String> = args
                                .iter()
                                .filter_map(|arg| match arg {
                                    Type::Nominal { name, args }
                                        if args.is_empty() && params.contains(name) =>
                                    {
                                        Some(name.clone())
                                    }
                                    _ => None,
                                })
                                .collect();
                            for arg in args.iter_mut() {
                                *arg = parameterized(arg, &named);
                            }
                            named
                        }
                        None => Vec::new(),
                    },
                    _ => Vec::new(),
                };
                if let ExtendBody::Methods(methods) = &mut e.body {
                    for method in methods {
                        function(method, &names);
                    }
                }
            }
            ItemKind::Alias(_) | ItemKind::Const(_) => {}
        }
    }
}

/// `ty` with each of `names` it mentions bare as the type parameter it is.
fn parameterized(ty: &Type, names: &[String]) -> Type {
    if names.is_empty() {
        return ty.clone();
    }
    let inner = |t: &Type| parameterized(t, names);
    match ty {
        Type::Nominal { name, args } if args.is_empty() && names.contains(name) => {
            Type::Generic(name.clone())
        }
        Type::Nominal { name, args } => Type::Nominal {
            name: name.clone(),
            args: args.iter().map(inner).collect(),
        },
        Type::Array(item) => Type::Array(Box::new(inner(item))),
        Type::Optional(item) => Type::Optional(Box::new(inner(item))),
        Type::Result(ok, err) => Type::Result(Box::new(inner(ok)), Box::new(inner(err))),
        Type::Tuple(items) => Type::Tuple(items.iter().map(inner).collect()),
        Type::Function { params, result } => Type::Function {
            params: params.iter().map(inner).collect(),
            result: Box::new(inner(result)),
        },
        Type::BorrowFunction { params, result } => Type::BorrowFunction {
            params: params.iter().map(inner).collect(),
            result: Box::new(inner(result)),
        },
        Type::Scalar(_) | Type::String | Type::TraitObject(_) | Type::Generic(_) => ty.clone(),
    }
}

/// `@Open enum` sets the enum's open flag; the parser reads the annotation
/// generically, so fold it into the declaration here.
pub(super) fn apply_open_annotations(module: &mut Module) {
    for item in &mut module.items {
        let is_open = item.annotations.iter().any(|a| a.name == "Open");
        if is_open && let ItemKind::Enum(e) = &mut item.kind {
            e.open = true;
        }
    }
}
