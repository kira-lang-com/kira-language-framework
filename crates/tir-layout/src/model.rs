use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Layout {
    pub size: u64,
    pub align: u64,
}

impl Layout {
    #[must_use]
    pub const fn new(size: u64, align: u64) -> Self {
        Self { size, align }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LayoutRule {
    Ridl,
    C,
    Packed(u8),
    Explicit,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DefinitionKind {
    Struct,
    Union,
    Enum,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LayoutField {
    pub name: Option<String>,
    pub ty: tir_types::Type,
    pub explicit_offset: Option<u64>,
    pub explicit_bit_offset: Option<u16>,
    pub bit_width: Option<u16>,
    pub flexible: bool,
}

impl LayoutField {
    #[must_use]
    pub fn named(name: impl Into<String>, ty: tir_types::Type) -> Self {
        Self {
            name: Some(name.into()),
            ty,
            explicit_offset: None,
            explicit_bit_offset: None,
            bit_width: None,
            flexible: false,
        }
    }

    #[must_use]
    pub fn positional(ty: tir_types::Type) -> Self {
        Self {
            name: None,
            ty,
            explicit_offset: None,
            explicit_bit_offset: None,
            bit_width: None,
            flexible: false,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EnumVariant {
    pub fields: Vec<LayoutField>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NominalDefinition {
    pub path: String,
    pub parameters: Vec<String>,
    pub opaque: bool,
    pub rule: LayoutRule,
    pub kind: DefinitionKind,
    pub fields: Vec<LayoutField>,
    pub variants: Vec<EnumVariant>,
    pub tag_type: Option<tir_types::Type>,
    pub explicit_size: Option<u64>,
    pub explicit_align: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FieldLayout {
    pub byte_offset: u64,
    pub bit_offset: Option<u16>,
    pub bit_width: Option<u16>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NominalLayout {
    pub layout: Layout,
    pub fields: Vec<FieldLayout>,
    pub variants: Vec<Vec<FieldLayout>>,
    pub payload_offset: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutError {
    DuplicateDefinition(String),
    UnknownNominal(String),
    GenericArgumentCount {
        path: String,
        expected: usize,
        actual: usize,
    },
    RecursiveNominal(String),
    AddressOnlyType,
    RegisterOnlyType,
    InvalidPacking(u8),
    InvalidBitField(String),
    InvalidFlexibleArray,
    ObjectTooLarge,
    MissingExplicitSize,
    MissingExplicitAlignment,
    MissingEnumTag,
    InvalidType(String),
    MissingExplicitOffset(String),
    InvalidExplicitLayout(String),
    Overflow,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateDefinition(path) => write!(formatter, "duplicate layout for {path}"),
            Self::UnknownNominal(path) => write!(formatter, "unknown nominal type {path}"),
            Self::GenericArgumentCount {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "{path} expects {expected} generic arguments but received {actual}"
            ),
            Self::RecursiveNominal(path) => {
                write!(formatter, "{path} contains itself by value")
            }
            Self::AddressOnlyType => formatter.write_str("type has no static memory layout"),
            Self::RegisterOnlyType => {
                formatter.write_str("register-only GPU type has no ordinary memory layout")
            }
            Self::InvalidPacking(alignment) => {
                write!(
                    formatter,
                    "packed alignment {alignment} is not 1, 2, 4, or 8"
                )
            }
            Self::InvalidBitField(name) => write!(formatter, "invalid bit-field {name}"),
            Self::InvalidFlexibleArray => {
                formatter.write_str("flexible array must be the last C struct field")
            }
            Self::ObjectTooLarge => formatter.write_str("object size must be less than 2^62 bytes"),
            Self::MissingExplicitSize => formatter.write_str("explicit layout has no size"),
            Self::MissingExplicitAlignment => {
                formatter.write_str("explicit layout has no alignment")
            }
            Self::MissingEnumTag => formatter.write_str("enum layout has no tag type"),
            Self::InvalidType(message) => formatter.write_str(message),
            Self::MissingExplicitOffset(name) => {
                write!(formatter, "explicit field {name} has no offset")
            }
            Self::InvalidExplicitLayout(message) => formatter.write_str(message),
            Self::Overflow => formatter.write_str("type layout arithmetic overflowed"),
        }
    }
}

impl std::error::Error for LayoutError {}
