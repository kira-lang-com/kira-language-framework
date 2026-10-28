//! The values RIDL carries across a transport (section 20), and the objects
//! it carries by reference.
//!
//! A value is self-describing: a `String` crossing the wire says it is a
//! string, a payload enum says its type and case. Neither side consults a
//! type table to read one, so a caller and a callee built from the same
//! `.ridl` need nothing but the bytes. Widths are absent for the same reason:
//! each side re-materializes a value through the C type its declaration gives
//! it, and a value that does not fit is a trap there, not a silent truncation.

use std::fmt;

/// Which side of a connection hosts the object a reference names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    /// The side that wrote the reference, which holds the object itself.
    Here,
    /// The other side, which holds a proxy that forwards to the host.
    Peer,
}

impl Side {
    #[must_use]
    pub fn flipped(self) -> Self {
        match self {
            Self::Here => Self::Peer,
            Self::Peer => Self::Here,
        }
    }
}

/// An object that crosses by reference: a box, an opaque struct, a trait
/// object or a closure. Each side maps an id to a live object of its own and
/// builds a proxy for the ones the other side names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectRef {
    pub side: Side,
    pub id: u32,
}

impl ObjectRef {
    #[must_use]
    pub fn here(id: u32) -> Self {
        Self {
            side: Side::Here,
            id,
        }
    }

    #[must_use]
    pub fn peer(id: u32) -> Self {
        Self {
            side: Side::Peer,
            id,
        }
    }

    /// The reference the other side writes for an object `id` it hosts.
    #[must_use]
    pub fn written_by_host(id: u32) -> Self {
        Self::here(id)
    }
}

impl fmt::Display for ObjectRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.side {
            Side::Here => write!(f, "object {} here", self.id),
            Side::Peer => write!(f, "object {} on the peer", self.id),
        }
    }
}

/// A RIDL value in the shape it takes on a wire.
///
/// `Int` and `Uint` are canonical across every width, and `Float` holds the
/// bits of an `f32` or an `f64` as the far side's declaration requires.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Void,
    Bool(bool),
    Int(i128),
    Uint(u128),
    Float(u64),
    RawPtr(u64),
    Bytes(Vec<u8>),
    Seq(Vec<Value>),
    Optional(Option<Box<Value>>),
    Ok(Box<Value>),
    Err(Box<Value>),
    Struct(String, Vec<Value>),
    Enum(String, String, Vec<Value>),
    Object(ObjectRef),
}

impl Value {
    #[must_use]
    pub fn unit() -> Self {
        Self::Void
    }

    #[must_use]
    pub fn some(value: Value) -> Self {
        Self::Optional(Some(Box::new(value)))
    }

    #[must_use]
    pub fn none() -> Self {
        Self::Optional(None)
    }

    #[must_use]
    pub fn seq(values: Vec<Value>) -> Self {
        Self::Seq(values)
    }

    /// The `Result` a function returning `Result<T, E>` reports: either the
    /// value or the error it carries.
    #[must_use]
    pub fn result(value: Result<Value, Value>) -> Self {
        match value {
            Ok(value) => Self::Ok(Box::new(value)),
            Err(error) => Self::Err(Box::new(error)),
        }
    }

    /// The value a `Result` holds, or why it has none.
    ///
    /// # Errors
    /// Returns the error value when the result is one, or a message when the
    /// value is neither a result nor a void, which a callee that returns
    /// nothing has no answer for.
    pub fn into_result(self) -> Result<Result<Value, Value>, String> {
        match self {
            Self::Ok(value) => Ok(Ok(*value)),
            Self::Err(value) => Ok(Err(*value)),
            Self::Void => Err("a result carries neither a value nor an error".to_owned()),
            other => Err(format!("{other:?} is not a result")),
        }
    }

    /// The value a `Result` carries, ignoring which side it was on.
    #[must_use]
    pub fn unwrap_result(self) -> Value {
        match self {
            Self::Ok(value) | Self::Err(value) => *value,
            other => other,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Void => f.write_str("void"),
            Self::Bool(value) => write!(f, "{value}"),
            Self::Int(value) => write!(f, "{value}"),
            Self::Uint(value) => write!(f, "{value}"),
            Self::Float(bits) => write!(f, "{bits:#018x}"),
            Self::RawPtr(address) => write!(f, "{address:#x}"),
            Self::Bytes(bytes) => write!(f, "{} bytes", bytes.len()),
            Self::Seq(values) => write!(f, "{} values", values.len()),
            Self::Optional(None) => f.write_str("none"),
            Self::Optional(Some(value)) => write!(f, "some {value}"),
            Self::Ok(value) => write!(f, "ok {value}"),
            Self::Err(value) => write!(f, "error {value}"),
            Self::Struct(name, _) => write!(f, "{name}"),
            Self::Enum(name, case, _) => write!(f, "{name}.{case}"),
            Self::Object(reference) => write!(f, "{reference}"),
        }
    }
}
