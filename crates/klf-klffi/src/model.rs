//! The `.klffi` model (section 26, grammar A.3). A `.klffi` file describes a
//! platform C ABI library: extern functions, data-symbol addresses, structs,
//! pointer/callback/alias types and integer/string constants. It is
//! language-neutral; any KLF toolchain imports the same file.

/// A parsed `.klffi` module: a flat list of declarations in source order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FfiModule {
    pub items: Vec<FfiItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FfiItem {
    Function(FfiFunction),
    Struct(FfiStruct),
    Constant(FfiConstant),
}

/// One `@FFI.*` annotation with its `{ key: value }` pairs. Pair values are kept
/// as raw text (an ident, a string or a `[a, b]` list) so every documented key
/// of section 26.2 round-trips without a fixed schema.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Annotation {
    pub kind: AnnotationKind,
    pub pairs: Vec<(String, String)>,
}

impl Annotation {
    #[must_use]
    pub fn value(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnnotationKind {
    Extern,
    Address,
    Struct,
    Union,
    Pointer,
    Array,
    Callback,
    Alias,
    Syscall,
    Variadic,
}

impl AnnotationKind {
    #[must_use]
    pub fn spelling(self) -> &'static str {
        match self {
            Self::Extern => "Extern",
            Self::Address => "Address",
            Self::Struct => "Struct",
            Self::Union => "Union",
            Self::Pointer => "Pointer",
            Self::Array => "Array",
            Self::Callback => "Callback",
            Self::Alias => "Alias",
            Self::Syscall => "Syscall",
            Self::Variadic => "Variadic",
        }
    }

    #[must_use]
    pub fn from_spelling(text: &str) -> Option<Self> {
        Some(match text {
            "Extern" => Self::Extern,
            "Address" => Self::Address,
            "Struct" => Self::Struct,
            "Union" => Self::Union,
            "Pointer" => Self::Pointer,
            "Array" => Self::Array,
            "Callback" => Self::Callback,
            "Alias" => Self::Alias,
            "Syscall" => Self::Syscall,
            "Variadic" => Self::Variadic,
            _ => return None,
        })
    }
}

/// A `function` declaration. An `@FFI.Extern` function is a platform-C call, an
/// `@FFI.Address` function names a data symbol, an `@FFI.Syscall` a syscall.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FfiFunction {
    pub annotations: Vec<Annotation>,
    pub name: String,
    pub parameters: Vec<(String, FfiType)>,
    pub result: FfiType,
}

impl FfiFunction {
    #[must_use]
    pub fn annotation(&self, kind: AnnotationKind) -> Option<&Annotation> {
        self.annotations.iter().find(|a| a.kind == kind)
    }
}

/// A `struct` declaration. `@FFI.Struct`/`@FFI.Union` carry fields; a
/// `@FFI.Pointer`/`@FFI.Callback`/`@FFI.Array` struct is a named opaque type
/// with no fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FfiStruct {
    pub annotations: Vec<Annotation>,
    pub name: String,
    pub fields: Vec<FfiField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FfiField {
    pub name: String,
    pub ty: FfiType,
    pub bits: Option<u32>,
}

/// A `let name: Type = literal` constant (an integer or string from a C macro).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FfiConstant {
    pub name: String,
    pub ty: FfiType,
    pub value: String,
}

/// A type name in an FFI signature: a RIDL scalar, `CString`, `RawPtr`, or a
/// named FFI type declared elsewhere in the file (grammar A.3 `ftype = ident`).
pub type FfiType = String;
