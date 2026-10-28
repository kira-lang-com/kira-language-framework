//! Frontend types for the KLF Kira subset and their mapping to KIR.

use klf_core::Name;
use klf_kir::Type;
use std::collections::HashMap;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FeType {
    Int {
        bits: u16,
        signed: bool,
    },
    Float {
        bits: u16,
    },
    Bool,
    Void,
    /// A declared struct, by name. Represented at runtime by the address of its
    /// storage (an lvalue), so struct values live in stack slots.
    Struct(Name),
    /// A declared enum, represented by the address of its storage.
    Enum(Name),
    Generic(Name),
    /// A fixed-size array literal, represented by the address of its storage.
    /// An absent length is an unresolved `[T]` annotation.
    Array {
        element: Box<Self>,
        length: Option<u64>,
    },
    /// A growable array (`[T]`): the address of a `{count, cap, data}` header
    /// whose `data` points at a run-time-sized reference-counted buffer.
    List(Box<Self>),
    Address(Box<Self>),
    Pointer,
    DefinitionHandle(DefinitionHandle),
    /// A function or closure type, `(A, B) -> R`: the address of a closure
    /// record, its code and its counted context (section 21.8).
    Function {
        params: Vec<Self>,
        result: Box<Self>,
    },
}

/// The kernel's closure record: `code`, then the counted `ctx` box.
pub(crate) const CLOSURE_RECORD: &str = "__closure";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DefinitionHandle {
    Session,
    SourceFile,
    Unit,
    Tree,
    Program,
    KirModule,
    KirModuleBuilder,
    KirFunctionBuilder,
    KirValue,
    KirBlockBuilder,
    KirAddress,
    KirTypeBuilder,
    KirType,
    KirSignature,
    KirGlobal,
    ShaderModule,
    ShaderUnit,
}

impl FeType {
    /// Resolves a built-in Kira type name. Declared nominal types are resolved
    /// against the source declaration tables elsewhere.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let ty = match name {
            "Int" | "I64" => Self::Int {
                bits: 64,
                signed: true,
            },
            "I32" => Self::Int {
                bits: 32,
                signed: true,
            },
            "I16" => Self::Int {
                bits: 16,
                signed: true,
            },
            "I8" => Self::Int {
                bits: 8,
                signed: true,
            },
            "U64" => Self::Int {
                bits: 64,
                signed: false,
            },
            "U32" => Self::Int {
                bits: 32,
                signed: false,
            },
            "U16" => Self::Int {
                bits: 16,
                signed: false,
            },
            "U8" => Self::Int {
                bits: 8,
                signed: false,
            },
            "F64" | "Float" => Self::Float { bits: 64 },
            "F32" => Self::Float { bits: 32 },
            "Bool" => Self::Bool,
            "Void" => Self::Void,
            "Session" => Self::DefinitionHandle(DefinitionHandle::Session),
            "SourceFile" => Self::DefinitionHandle(DefinitionHandle::SourceFile),
            "Unit" => Self::DefinitionHandle(DefinitionHandle::Unit),
            "Tree" => Self::DefinitionHandle(DefinitionHandle::Tree),
            "Program" => Self::DefinitionHandle(DefinitionHandle::Program),
            "KirModule" => Self::DefinitionHandle(DefinitionHandle::KirModule),
            "KirModuleBuilder" => Self::DefinitionHandle(DefinitionHandle::KirModuleBuilder),
            "KirFunctionBuilder" => Self::DefinitionHandle(DefinitionHandle::KirFunctionBuilder),
            "KirValue" => Self::DefinitionHandle(DefinitionHandle::KirValue),
            "KirBlockBuilder" => Self::DefinitionHandle(DefinitionHandle::KirBlockBuilder),
            "KirAddress" => Self::DefinitionHandle(DefinitionHandle::KirAddress),
            "KirTypeBuilder" => Self::DefinitionHandle(DefinitionHandle::KirTypeBuilder),
            "KirType" => Self::DefinitionHandle(DefinitionHandle::KirType),
            "KirSignature" => Self::DefinitionHandle(DefinitionHandle::KirSignature),
            "KirGlobal" => Self::DefinitionHandle(DefinitionHandle::KirGlobal),
            "ShaderModule" => Self::DefinitionHandle(DefinitionHandle::ShaderModule),
            "ShaderUnit" => Self::DefinitionHandle(DefinitionHandle::ShaderUnit),
            _ => return None,
        };
        Some(ty)
    }

    #[must_use]
    pub fn to_kir(&self) -> Type {
        match self {
            Self::Int { bits, .. } => Type::Integer(*bits),
            Self::Float { bits } => Type::Float(*bits),
            Self::Bool => Type::Integer(1),
            Self::Void => Type::Unit,
            Self::Struct(name) | Self::Enum(name) => Type::Nominal(name.clone(), Vec::new()),
            Self::Generic(name) => Type::Generic(name.clone()),
            Self::Array { element, length } => {
                Type::Array(length.unwrap_or(0), Box::new(element.to_kir()))
            }
            Self::List(_) => Type::Nominal(Name::new("List"), Vec::new()),
            Self::Address(inner) => Type::Address(Box::new(inner.to_kir())),
            Self::Pointer => Type::Pointer,
            Self::DefinitionHandle(_) => Type::Integer(64),
            Self::Function { .. } => Type::Nominal(Name::new(CLOSURE_RECORD), Vec::new()),
        }
    }

    #[must_use]
    pub fn value_kir(&self) -> Type {
        match self {
            Self::Struct(name) | Self::Enum(name) => {
                Type::Address(Box::new(Type::Nominal(name.clone(), Vec::new())))
            }
            Self::Array { .. } | Self::List(_) | Self::Function { .. } => {
                Type::Address(Box::new(self.to_kir()))
            }
            Self::DefinitionHandle(_) => Type::Integer(64),
            _ => self.to_kir(),
        }
    }

    #[must_use]
    pub fn is_signed(&self) -> bool {
        matches!(self, Self::Int { signed: true, .. })
    }

    #[must_use]
    pub fn is_float(&self) -> bool {
        matches!(self, Self::Float { .. })
    }

    #[must_use]
    pub fn is_scalar(&self) -> bool {
        matches!(self, Self::Int { .. } | Self::Float { .. } | Self::Bool)
    }

    #[must_use]
    pub fn compatible(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Array {
                    element: left,
                    length: left_length,
                },
                Self::Array {
                    element: right,
                    length: right_length,
                },
            ) => {
                left.compatible(right)
                    && (left_length.is_none()
                        || right_length.is_none()
                        || left_length == right_length)
            }
            (Self::List(left), Self::List(right)) => left.compatible(right),
            _ => self == other,
        }
    }

    /// Whether a type parameter no argument has pinned yet appears in it.
    #[must_use]
    pub fn is_open(&self) -> bool {
        match self {
            Self::Generic(_) => true,
            Self::Array { element, .. } | Self::List(element) | Self::Address(element) => {
                element.is_open()
            }
            Self::Function { params, result } => {
                params.iter().any(Self::is_open) || result.is_open()
            }
            _ => false,
        }
    }

    #[must_use]
    pub fn substitute(&self, substitutions: &HashMap<Name, Self>) -> Self {
        match self {
            Self::Generic(name) => substitutions
                .get(name)
                .cloned()
                .unwrap_or_else(|| self.clone()),
            Self::Array { element, length } => Self::Array {
                element: Box::new(element.substitute(substitutions)),
                length: *length,
            },
            Self::List(element) => Self::List(Box::new(element.substitute(substitutions))),
            Self::Address(inner) => Self::Address(Box::new(inner.substitute(substitutions))),
            Self::Function { params, result } => Self::Function {
                params: params
                    .iter()
                    .map(|param| param.substitute(substitutions))
                    .collect(),
                result: Box::new(result.substitute(substitutions)),
            },
            other => other.clone(),
        }
    }

    pub fn infer_from(&self, actual: &Self, substitutions: &mut HashMap<Name, Self>) -> bool {
        match (self, actual) {
            (Self::Generic(name), actual) => {
                if let Some(existing) = substitutions.get(name) {
                    existing.compatible(actual)
                } else {
                    substitutions.insert(name.clone(), actual.clone());
                    true
                }
            }
            (
                Self::Array { element, length },
                Self::Array {
                    element: actual_element,
                    length: actual_length,
                },
            ) => {
                element.infer_from(actual_element, substitutions)
                    && (length.is_none() || actual_length.is_none() || length == actual_length)
            }
            (Self::Address(expected), Self::Address(actual))
            | (Self::List(expected), Self::List(actual)) => {
                expected.infer_from(actual, substitutions)
            }
            (
                Self::Function { params, result },
                Self::Function {
                    params: actual_params,
                    result: actual_result,
                },
            ) => {
                params.len() == actual_params.len()
                    && params
                        .iter()
                        .zip(actual_params)
                        .all(|(param, actual)| param.infer_from(actual, substitutions))
                    && result.infer_from(actual_result, substitutions)
            }
            _ => self.compatible(actual),
        }
    }

    /// The type as it appears inside a generic instance's name: `display`,
    /// except that nominal names keep their instance spelling.
    #[must_use]
    pub fn spelling(&self) -> String {
        match self {
            Self::Struct(name) | Self::Enum(name) => name.to_string(),
            Self::Array { element, .. } | Self::List(element) => {
                format!("[{}]", element.spelling())
            }
            other => other.display(),
        }
    }

    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Int {
                bits: 64,
                signed: true,
            } => "Int".to_string(),
            Self::Int { bits, signed: true } => format!("I{bits}"),
            Self::Int {
                bits,
                signed: false,
            } => format!("U{bits}"),
            Self::Float { bits } => format!("F{bits}"),
            Self::Bool => "Bool".to_string(),
            Self::Void => "Void".to_string(),
            Self::Struct(name) | Self::Enum(name) => name
                .as_str()
                .replace('(', "<")
                .replace(')', ">")
                .replace(',', ", "),
            Self::Generic(name) => format!("{name}"),
            Self::Array { element, .. } | Self::List(element) => format!("[{}]", element.display()),
            Self::Address(inner) => format!("*{}", inner.display()),
            Self::Pointer => "RawPtr".to_string(),
            Self::DefinitionHandle(handle) => handle.name().to_owned(),
            Self::Function { params, result } => {
                let params: Vec<String> = params.iter().map(Self::display).collect();
                format!("({}) -> {}", params.join(", "), result.display())
            }
        }
    }
}

impl DefinitionHandle {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::SourceFile => "SourceFile",
            Self::Unit => "Unit",
            Self::Tree => "Tree",
            Self::Program => "Program",
            Self::KirModule => "KirModule",
            Self::KirModuleBuilder => "KirModuleBuilder",
            Self::KirFunctionBuilder => "KirFunctionBuilder",
            Self::KirValue => "KirValue",
            Self::KirBlockBuilder => "KirBlockBuilder",
            Self::KirAddress => "KirAddress",
            Self::KirTypeBuilder => "KirTypeBuilder",
            Self::KirType => "KirType",
            Self::KirSignature => "KirSignature",
            Self::KirGlobal => "KirGlobal",
            Self::ShaderModule => "ShaderModule",
            Self::ShaderUnit => "ShaderUnit",
        }
    }
}
