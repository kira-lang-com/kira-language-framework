//! The RIDL type system (section 20).

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scalar {
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    Bool,
    ISize,
    USize,
    RawPtr,
    Void,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Type {
    Scalar(Scalar),
    String,
    Array(Box<Type>),
    Optional(Box<Type>),
    Result(Box<Type>, Box<Type>),
    Tuple(Vec<Type>),
    Nominal {
        name: String,
        args: Vec<Type>,
    },
    Function {
        params: Vec<Type>,
        result: Box<Type>,
    },
    BorrowFunction {
        params: Vec<Type>,
        result: Box<Type>,
    },
    TraitObject(String),
    Generic(String),
}

impl Type {
    #[must_use]
    pub fn scalar(s: Scalar) -> Self {
        Type::Scalar(s)
    }

    #[must_use]
    pub fn nominal(name: impl Into<String>) -> Self {
        Type::Nominal {
            name: name.into(),
            args: Vec::new(),
        }
    }

    #[must_use]
    pub fn substitute(&self, subst: &BTreeMap<String, Type>) -> Type {
        match self {
            Type::Generic(name) => subst.get(name).cloned().unwrap_or_else(|| self.clone()),
            Type::Array(inner) => Type::Array(Box::new(inner.substitute(subst))),
            Type::Optional(inner) => Type::Optional(Box::new(inner.substitute(subst))),
            Type::Result(ok, err) => Type::Result(
                Box::new(ok.substitute(subst)),
                Box::new(err.substitute(subst)),
            ),
            Type::Tuple(items) => Type::Tuple(items.iter().map(|t| t.substitute(subst)).collect()),
            Type::Nominal { name, args } => Type::Nominal {
                name: name.clone(),
                args: args.iter().map(|t| t.substitute(subst)).collect(),
            },
            Type::Function { params, result } => Type::Function {
                params: params.iter().map(|t| t.substitute(subst)).collect(),
                result: Box::new(result.substitute(subst)),
            },
            Type::BorrowFunction { params, result } => Type::BorrowFunction {
                params: params.iter().map(|t| t.substitute(subst)).collect(),
                result: Box::new(result.substitute(subst)),
            },
            Type::Scalar(_) | Type::String | Type::TraitObject(_) => self.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    /// The value a reader writes in when it builds the struct and leaves the
    /// field out (section 20.3), as written in the `.ridl`. It has no ABI
    /// effect.
    pub default: Option<String>,
}

/// The sized numbers (section 27.5): types of their own, spelled by name in a
/// `.ridl`, each stored as the integer of its width (section 21.1).
pub const SIZED_NUMBERS: [&str; 6] = ["N8", "N16", "N32", "UN8", "UN16", "UN32"];

/// The integer a sized number is stored as.
#[must_use]
pub fn sized_number_storage(name: &str) -> Option<Scalar> {
    Some(match name {
        "N8" => Scalar::I8,
        "N16" => Scalar::I16,
        "N32" => Scalar::I32,
        "UN8" => Scalar::U8,
        "UN16" => Scalar::U16,
        "UN32" => Scalar::U32,
        _ => return None,
    })
}

/// A sized number's layout under its own name: one field of its storage
/// integer, so it is laid out exactly as that integer.
#[must_use]
pub fn sized_number_layout(name: &str) -> Option<StructDecl> {
    let storage = sized_number_storage(name)?;
    Some(StructDecl {
        name: name.to_owned(),
        generics: Vec::new(),
        fields: Some(vec![Field {
            name: "value".to_owned(),
            ty: Type::Scalar(storage),
            default: None,
        }]),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructDecl {
    pub name: String,
    pub generics: Vec<String>,
    /// `None` is an opaque struct (a handle).
    pub fields: Option<Vec<Field>>,
}

impl StructDecl {
    #[must_use]
    pub fn is_opaque(&self) -> bool {
        self.fields.is_none()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Case {
    pub name: String,
    pub payload: Vec<Type>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumDecl {
    pub name: String,
    pub generics: Vec<String>,
    pub open: bool,
    pub cases: Vec<Case>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decl {
    Struct(StructDecl),
    Enum(EnumDecl),
}

impl Decl {
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Decl::Struct(s) => &s.name,
            Decl::Enum(e) => &e.name,
        }
    }

    #[must_use]
    pub fn generics(&self) -> &[String] {
        match self {
            Decl::Struct(s) => &s.generics,
            Decl::Enum(e) => &e.generics,
        }
    }
}

/// A set of nominal declarations that layout resolves names against.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    decls: BTreeMap<String, Decl>,
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, decl: Decl) {
        self.decls.insert(decl.name().to_owned(), decl);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Decl> {
        self.decls.get(name)
    }
}
