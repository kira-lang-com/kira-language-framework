//! The `.ridl` module model (section 20): a parsed interface surface.

use crate::types::{EnumDecl, StructDecl, Type};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Annotation {
    pub name: String,
    pub args: Vec<AnnArg>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnnArg {
    /// `None` for a positional argument.
    pub label: Option<String>,
    pub value: AnnValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnnValue {
    String(String),
    Type(Type),
    Ident(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Convention {
    Owned,
    Borrow,
    BorrowMut,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Param {
    pub label: String,
    pub convention: Convention,
    pub ty: Type,
    /// What a call that leaves the slot empty passes, written as a field's
    /// default is (section 20.3).
    pub default: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelfParam {
    Owned,
    Borrow,
    BorrowMut,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenericParam {
    pub name: String,
    pub bounds: Vec<Type>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionDecl {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub self_param: Option<SelfParam>,
    pub params: Vec<Param>,
    pub result: Option<Type>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssocType {
    pub name: String,
    pub bound: Option<Type>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraitDecl {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub parents: Vec<Type>,
    pub assoc: Vec<AssocType>,
    pub methods: Vec<FunctionDecl>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtendBody {
    /// `extend T: Trait, ...` — a conformance.
    Conformance(Vec<Type>),
    /// `extend T { function ... }` — methods.
    Methods(Vec<FunctionDecl>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtendDecl {
    pub target: Type,
    pub body: ExtendBody,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AliasDecl {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstDecl {
    pub name: String,
    pub ty: Type,
    pub literal: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemKind {
    Struct(StructDecl),
    Enum(EnumDecl),
    Trait(TraitDecl),
    Function(FunctionDecl),
    Extend(ExtendDecl),
    Alias(AliasDecl),
    Const(ConstDecl),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Item {
    pub annotations: Vec<Annotation>,
    pub kind: ItemKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Module {
    pub name: String,
    pub imports: Vec<String>,
    pub items: Vec<Item>,
}

impl Module {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            imports: Vec::new(),
            items: Vec::new(),
        }
    }

    /// Adds what `other` declares of the same module and this one does not:
    /// two programs built apart each declare what they use of it.
    pub fn join(&mut self, other: Module) {
        for import in other.imports {
            if !self.imports.contains(&import) {
                self.imports.push(import);
            }
        }
        for item in other.items {
            if !self.items.contains(&item) {
                self.items.push(item);
            }
        }
    }

    /// Build a layout [`Registry`](crate::types::Registry) from the struct and
    /// enum declarations in this module, with the sized numbers, which a
    /// `.ridl` spells by name and never declares (section 20.2).
    #[must_use]
    pub fn registry(&self) -> crate::types::Registry {
        let mut r = crate::types::Registry::new();
        for item in &self.items {
            match &item.kind {
                ItemKind::Struct(s) => r.insert(crate::types::Decl::Struct(s.clone())),
                ItemKind::Enum(e) => r.insert(crate::types::Decl::Enum(e.clone())),
                _ => {}
            }
        }
        for name in crate::types::SIZED_NUMBERS {
            if r.get(name).is_none()
                && let Some(layout) = crate::types::sized_number_layout(name)
            {
                r.insert(crate::types::Decl::Struct(layout));
            }
        }
        r
    }
}
