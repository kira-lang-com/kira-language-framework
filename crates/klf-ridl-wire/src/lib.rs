//! RIDL across a process boundary (section 24.7).
//!
//! A value crosses in the shape it has in memory, because section 21.4 was
//! written for a crossing whose two sides do not share an allocator: a buffer
//! travels as its length and a reference to the owner's `BufferOps` rather than
//! a raw address, and a descriptor travels as its `copy`, `move` and `destroy`
//! behaviour rather than the storage they apply to.
//!
//! A value is self-describing, so neither side consults a type table to read
//! one: a string says it is a string, a payload enum says its type and case.
//! Widths do not travel — each side re-materializes through the C type its
//! declaration gives it — and an object that crosses arrives as a reference to
//! whichever side hosts it, never as its storage.
//!
//! This crate is the encoding alone. What travels over a channel, and what a
//! proxy is, is section 24.7.1 and 24.7.2; the limits 24.7.5 places on one
//! message are [`MAX_MESSAGE`] and [`MAX_DEPTH`].

mod value;
mod wire;

pub use value::{ObjectRef, Side, Value};
pub use wire::{MAX_LENGTH, Reader, Writer, describe, frame, unframe};

/// What one message carries, section 24.7.5.
pub const MAX_MESSAGE: usize = 64 * 1024;

/// How deep a value may nest before it is refused, section 24.7.5.
pub const MAX_DEPTH: usize = 64;

/// A call from one side of a connection to the other (section 24.7.3).
///
/// A request names the function to call and carries its arguments. A reply
/// names the request it answers, on a channel of its own, so a handler is
/// never waiting on the channel it is servicing.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    /// The connection this call travels on.
    pub connection: u64,
    /// The caller's id for this request, which the reply names back.
    pub request: u64,
    /// The function to run, by the name it was declared with.
    pub function: String,
    /// Its arguments, in declaration order.
    pub arguments: Vec<Value>,
}

impl Call {
    #[must_use]
    pub fn new(connection: u64, request: u64, function: &str, arguments: Vec<Value>) -> Self {
        Self {
            connection,
            request,
            function: function.to_owned(),
            arguments,
        }
    }
}

/// What a call returned, or why it did not (section 24.7.3).
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// The call returned, with this value.
    Returned(Value),
    /// The callee failed with this error value.
    Failed(Value),
    /// The callee trapped with this code.
    Trapped(String),
    /// The call violated this section: `KLF-RT-0203` for a cycle on a
    /// connection, `KLF-RT-0204` for a value nested too deep.
    Refused(String),
}

impl Reply {
    /// Whether the call returned a value.
    #[must_use]
    pub fn is_return(&self) -> bool {
        matches!(self, Self::Returned(_))
    }

    /// The value a reply carries, whichever side it was on.
    #[must_use]
    pub fn value(self) -> Value {
        match self {
            Self::Returned(value) | Self::Failed(value) => value,
            Self::Trapped(_) | Self::Refused(_) => Value::Void,
        }
    }
}

/// One message on a connection, in either direction.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// A side is calling the other.
    Request(Call),
    /// A side is answering a call it received.
    Reply { request: u64, reply: Reply },
    /// An object was registered on the exporting side.
    Exported { connection: u64, object: u32 },
    /// The last proxy for an object is gone.
    Released { connection: u64, object: u32 },
    /// A peer closed a connection; every proxy on it is dead.
    Closed { connection: u64 },
}

impl Message {
    /// Writes this message for a channel to send.
    ///
    /// # Errors
    /// Returns why the message is not one a channel may carry: a payload over
    /// [`MAX_MESSAGE`], or a value nested deeper than [`MAX_DEPTH`].
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let body = self.body();
        if body.len() > MAX_MESSAGE {
            return Err(format!(
                "a message of {} bytes is over the {} byte limit, so it is a transfer \
                 rather than a message (section 24.7.5)",
                body.len(),
                MAX_MESSAGE
            ));
        }
        Ok(body)
    }

    fn body(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        match self {
            Self::Request(call) => {
                writer.write(&Value::Struct(
                    String::from("Call"),
                    vec![
                        Value::Uint(u128::from(call.connection)),
                        Value::Uint(u128::from(call.request)),
                        Value::Bytes(call.function.clone().into_bytes()),
                        Value::Seq(call.arguments.clone()),
                    ],
                ));
            }
            Self::Reply { request, reply } => {
                let (name, payload) = match reply {
                    Reply::Returned(value) => ("Returned", vec![value.clone()]),
                    Reply::Failed(value) => ("Failed", vec![value.clone()]),
                    Reply::Trapped(code) => {
                        ("Trapped", vec![Value::Bytes(code.clone().into_bytes())])
                    }
                    Reply::Refused(code) => {
                        ("Refused", vec![Value::Bytes(code.clone().into_bytes())])
                    }
                };
                writer.write(&Value::Struct(
                    String::from("Reply"),
                    vec![
                        Value::Uint(u128::from(*request)),
                        Value::Struct(String::from(name), payload),
                    ],
                ));
            }
            Self::Exported { connection, object } => {
                writer.write(&Value::Struct(
                    String::from("Exported"),
                    vec![
                        Value::Uint(u128::from(*connection)),
                        Value::Uint(u128::from(*object)),
                    ],
                ));
            }
            Self::Released { connection, object } => {
                writer.write(&Value::Struct(
                    String::from("Released"),
                    vec![
                        Value::Uint(u128::from(*connection)),
                        Value::Uint(u128::from(*object)),
                    ],
                ));
            }
            Self::Closed { connection } => {
                writer.write(&Value::Struct(
                    String::from("Closed"),
                    vec![Value::Uint(u128::from(*connection))],
                ));
            }
        }
        writer.finish()
    }

    /// Reads a message a channel received.
    ///
    /// # Errors
    /// Returns why the bytes are not a message: a frame that does not decode,
    /// or one naming a message this section does not define.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let value = unframe(bytes)?;
        let Value::Struct(name, fields) = value else {
            return Err(format!("a message names no kind, got {value}"));
        };
        let number = |at: usize| -> Result<u64, String> {
            match fields.get(at) {
                Some(Value::Uint(value)) => {
                    u64::try_from(*value).map_err(|_| "a message holds an id too wide".to_owned())
                }
                other => Err(format!("field {at} of {name} is not an id: {other:?}")),
            }
        };
        let text = |at: usize| -> Result<String, String> {
            match fields.get(at) {
                Some(Value::Bytes(bytes)) => String::from_utf8(bytes.clone())
                    .map_err(|error| format!("field {at} of {name} is not UTF-8: {error}")),
                other => Err(format!("field {at} of {name} is not text: {other:?}")),
            }
        };
        match name.as_str() {
            "Call" => {
                let arguments = match fields.get(3) {
                    Some(Value::Seq(values)) => values.clone(),
                    other => return Err(format!("Call's arguments are not a sequence: {other:?}")),
                };
                Ok(Self::Request(Call {
                    connection: number(0)?,
                    request: number(1)?,
                    function: text(2)?,
                    arguments,
                }))
            }
            "Reply" => {
                let request = number(0)?;
                let Some(Value::Struct(kind, payload)) = fields.get(1) else {
                    return Err("a reply names no outcome".to_owned());
                };
                let one = || {
                    payload
                        .first()
                        .cloned()
                        .ok_or_else(|| format!("a {kind} reply carries no value"))
                };
                let reply = match kind.as_str() {
                    "Returned" => Reply::Returned(one()?),
                    "Failed" => Reply::Failed(one()?),
                    "Trapped" => Reply::Trapped(one()?.text()?),
                    "Refused" => Reply::Refused(one()?.text()?),
                    other => return Err(format!("a reply names the unknown outcome {other}")),
                };
                Ok(Self::Reply { request, reply })
            }
            "Exported" => Ok(Self::Exported {
                connection: number(0)?,
                object: u32::try_from(number(1)?).map_err(|_| "an object id is too wide")?,
            }),
            "Released" => Ok(Self::Released {
                connection: number(0)?,
                object: u32::try_from(number(1)?).map_err(|_| "an object id is too wide")?,
            }),
            "Closed" => Ok(Self::Closed {
                connection: number(0)?,
            }),
            other => Err(format!("a message names the unknown kind {other}")),
        }
    }
}

trait AsText {
    fn text(self) -> Result<String, String>;
}

impl AsText for Value {
    fn text(self) -> Result<String, String> {
        match self {
            Value::Bytes(bytes) => {
                String::from_utf8(bytes).map_err(|error| format!("not UTF-8: {error}"))
            }
            other => Err(format!("{other:?} is not bytes")),
        }
    }
}
