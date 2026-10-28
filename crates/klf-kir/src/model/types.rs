//! The KIR type system (section 10.2).

use super::FunctionAbi;
use klf_core::Name;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Convention {
    Owned,
    Borrow,
    BorrowMut,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Type {
    Integer(u16),
    Float(u16),
    Pointer,
    Unit,
    Never,
    Vector(u32, Box<Type>),
    Tuple(Vec<Type>),
    Array(u64, Box<Type>),
    Nominal(Name, Vec<Type>),
    Generic(Name),
    Reference(Box<Type>),
    Unique(Box<Type>),
    Weak(Box<Type>),
    Address(Box<Type>),
    Function(FunctionType),
}

impl Type {
    #[must_use]
    pub fn is_integer(&self) -> bool {
        matches!(self, Self::Integer(_))
    }

    #[must_use]
    pub fn is_float(&self) -> bool {
        matches!(self, Self::Float(_))
    }

    /// The element type of a vector, or the type itself: arithmetic in section
    /// 12.2 and 12.3 applies elementwise to vectors.
    #[must_use]
    pub fn lane(&self) -> &Self {
        match self {
            Self::Vector(_, element) => element,
            other => other,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FunctionType {
    pub parameters: Vec<ParameterType>,
    pub result: Box<Type>,
    pub failure: Option<Box<Type>>,
    pub abi: FunctionAbi,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParameterType {
    pub context: bool,
    pub convention: Option<Convention>,
    pub ty: Type,
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Integer(bits) => write!(formatter, "i{bits}"),
            Self::Float(bits) => write!(formatter, "f{bits}"),
            Self::Pointer => formatter.write_str("ptr"),
            Self::Unit => formatter.write_str("unit"),
            Self::Never => formatter.write_str("never"),
            Self::Vector(count, element) => write!(formatter, "vec<{count} x {element}>"),
            Self::Tuple(elements) => {
                formatter.write_str("(")?;
                write_joined(formatter, elements)?;
                formatter.write_str(")")
            }
            Self::Array(count, element) => write!(formatter, "[{count} x {element}]"),
            Self::Nominal(name, arguments) => {
                write!(formatter, "%{name}")?;
                if !arguments.is_empty() {
                    formatter.write_str("<")?;
                    write_joined(formatter, arguments)?;
                    formatter.write_str(">")?;
                }
                Ok(())
            }
            Self::Generic(name) => write!(formatter, "${name}"),
            Self::Reference(inner) => write!(formatter, "ref {inner}"),
            Self::Unique(inner) => write!(formatter, "uniq {inner}"),
            Self::Weak(inner) => write!(formatter, "weak {inner}"),
            Self::Address(inner) => write!(formatter, "*{inner}"),
            Self::Function(function) => fmt::Display::fmt(function, formatter),
        }
    }
}

impl fmt::Display for FunctionType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("fn(")?;
        write_joined(formatter, &self.parameters)?;
        write!(formatter, ") -> {}", self.result)?;
        if let Some(failure) = &self.failure {
            write!(formatter, " fails {failure}")?;
        }
        if self.abi == FunctionAbi::Ridl {
            formatter.write_str(" abi(ridl)")?;
        } else if type_ends_with_function_abi(
            self.failure.as_deref().unwrap_or(self.result.as_ref()),
        ) {
            formatter.write_str(" abi(platform)")?;
        }
        Ok(())
    }
}

impl fmt::Display for ParameterType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.context {
            formatter.write_str("context ")?;
        }
        if let Some(convention) = self.convention {
            write!(formatter, "{convention} ")?;
        }
        fmt::Display::fmt(&self.ty, formatter)
    }
}

impl fmt::Display for Convention {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Owned => "owned",
            Self::Borrow => "borrow",
            Self::BorrowMut => "borrowmut",
        })
    }
}

fn write_joined<T: fmt::Display>(formatter: &mut fmt::Formatter<'_>, values: &[T]) -> fmt::Result {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        fmt::Display::fmt(value, formatter)?;
    }
    Ok(())
}

fn type_ends_with_function_abi(ty: &Type) -> bool {
    match ty {
        Type::Function(function) => {
            function.abi == FunctionAbi::Ridl
                || type_ends_with_function_abi(
                    function
                        .failure
                        .as_deref()
                        .unwrap_or(function.result.as_ref()),
                )
        }
        Type::Reference(inner) | Type::Unique(inner) | Type::Weak(inner) | Type::Address(inner) => {
            type_ends_with_function_abi(inner)
        }
        _ => false,
    }
}
