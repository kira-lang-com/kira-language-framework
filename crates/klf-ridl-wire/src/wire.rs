//! How a [`Value`] is written to a frame and read back (section 20).
//!
//! The encoding is fixed little-endian with a one-byte tag per value and no
//! alignment padding, so a frame is readable by a C implementation that walks
//! it the same way. Lengths are read against a limit before anything is
//! allocated, so a corrupt frame cannot make a peer reserve unbounded memory.

use crate::value::{ObjectRef, Side, Value};
use std::fmt;
use std::fmt::Write as _;

const VOID: u8 = 0x00;
const BOOL: u8 = 0x01;
const INT: u8 = 0x02;
const UINT: u8 = 0x03;
const FLOAT: u8 = 0x04;
const RAWPTR: u8 = 0x05;
const BYTES: u8 = 0x06;
const SEQ: u8 = 0x07;
const OPTIONAL: u8 = 0x08;
const OK: u8 = 0x09;
const ERR: u8 = 0x0a;
const STRUCT: u8 = 0x0b;
const ENUM: u8 = 0x0c;
const OBJECT: u8 = 0x0d;

/// The largest a length prefix may claim, so a corrupt frame is rejected
/// before the memory it asks for is reserved.
pub const MAX_LENGTH: u32 = 1 << 30;

/// Writes values into a frame.
#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write(&mut self, value: &Value) {
        self.bytes.push(tag(value));
        match value {
            Value::Void => {}
            Value::Bool(value) => self.bytes.push(u8::from(*value)),
            Value::Int(value) => self.bytes.extend_from_slice(&value.to_le_bytes()),
            Value::Uint(value) => self.bytes.extend_from_slice(&value.to_le_bytes()),
            Value::Float(bits) | Value::RawPtr(bits) => {
                self.bytes.extend_from_slice(&bits.to_le_bytes());
            }
            Value::Bytes(bytes) => {
                self.length(bytes.len());
                self.bytes.extend_from_slice(bytes);
            }
            Value::Seq(values) => self.sequence(values),
            Value::Optional(None) => self.bytes.push(0),
            Value::Optional(Some(value)) => {
                self.bytes.push(1);
                self.write(value);
            }
            Value::Ok(value) | Value::Err(value) => self.write(value),
            Value::Struct(name, fields) => {
                self.text(name);
                self.sequence(fields);
            }
            Value::Enum(name, case, payload) => {
                self.text(name);
                self.text(case);
                self.sequence(payload);
            }
            Value::Object(reference) => {
                self.bytes.push(match reference.side {
                    Side::Here => 0,
                    Side::Peer => 1,
                });
                self.bytes.extend_from_slice(&reference.id.to_le_bytes());
            }
        }
    }

    pub fn write_all(&mut self, values: &[Value]) {
        self.length(values.len());
        for value in values {
            self.write(value);
        }
    }

    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }

    fn sequence(&mut self, values: &[Value]) {
        self.length(values.len());
        for value in values {
            self.write(value);
        }
    }

    fn text(&mut self, value: &str) {
        self.length(value.len());
        self.bytes.extend_from_slice(value.as_bytes());
    }

    fn length(&mut self, length: usize) {
        let length = u32::try_from(length).unwrap_or(MAX_LENGTH);
        self.bytes.extend_from_slice(&length.to_le_bytes());
    }
}

fn tag(value: &Value) -> u8 {
    match value {
        Value::Void => VOID,
        Value::Bool(_) => BOOL,
        Value::Int(_) => INT,
        Value::Uint(_) => UINT,
        Value::Float(_) => FLOAT,
        Value::RawPtr(_) => RAWPTR,
        Value::Bytes(_) => BYTES,
        Value::Seq(_) => SEQ,
        Value::Optional(_) => OPTIONAL,
        Value::Ok(_) => OK,
        Value::Err(_) => ERR,
        Value::Struct(_, _) => STRUCT,
        Value::Enum(_, _, _) => ENUM,
        Value::Object(_) => OBJECT,
    }
}

/// Reads values back out of a frame.
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// The values a frame carries.
    ///
    /// # Errors
    /// Returns where the frame ended early, held a tag it does not define, or
    /// claimed a length past what the frame holds.
    pub fn read_all(&mut self) -> Result<Vec<Value>, String> {
        let count = self.length()?;
        let mut values = Vec::new();
        for _ in 0..count {
            values.push(self.read()?);
        }
        Ok(values)
    }

    /// The next value in the frame.
    ///
    /// # Errors
    /// Returns where the frame ended early, held a tag it does not define, or
    /// claimed a length past what the frame holds.
    pub fn read(&mut self) -> Result<Value, String> {
        let tag = self.byte()?;
        match tag {
            VOID => Ok(Value::Void),
            BOOL => Ok(Value::Bool(self.byte()? != 0)),
            INT => Ok(Value::Int(i128::from_le_bytes(self.sixteen()?))),
            UINT => Ok(Value::Uint(u128::from_le_bytes(self.sixteen()?))),
            FLOAT => Ok(Value::Float(u64::from_le_bytes(self.eight()?))),
            RAWPTR => Ok(Value::RawPtr(u64::from_le_bytes(self.eight()?))),
            BYTES => {
                let length = self.length()?;
                Ok(Value::Bytes(self.take(length)?.to_vec()))
            }
            SEQ => Ok(Value::Seq(self.sequence()?)),
            OPTIONAL => {
                if self.byte()? == 0 {
                    Ok(Value::Optional(None))
                } else {
                    Ok(Value::Optional(Some(Box::new(self.read()?))))
                }
            }
            OK => Ok(Value::Ok(Box::new(self.read()?))),
            ERR => Ok(Value::Err(Box::new(self.read()?))),
            STRUCT => {
                let name = self.text()?;
                Ok(Value::Struct(name, self.sequence()?))
            }
            ENUM => {
                let name = self.text()?;
                let case = self.text()?;
                Ok(Value::Enum(name, case, self.sequence()?))
            }
            OBJECT => {
                let side = if self.byte()? == 0 {
                    Side::Here
                } else {
                    Side::Peer
                };
                Ok(Value::Object(ObjectRef {
                    side,
                    id: self.word()?,
                }))
            }
            other => Err(format!("a frame holds the unknown value tag {other:#04x}")),
        }
    }

    fn sequence(&mut self) -> Result<Vec<Value>, String> {
        let count = self.length()?;
        let mut values = Vec::new();
        for _ in 0..count {
            values.push(self.read()?);
        }
        Ok(values)
    }

    fn text(&mut self) -> Result<String, String> {
        let length = self.length()?;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|error| format!("a frame holds invalid UTF-8: {error}"))
    }

    fn byte(&mut self) -> Result<u8, String> {
        let byte = *self
            .bytes
            .get(self.at)
            .ok_or_else(|| "a frame ended early".to_owned())?;
        self.at += 1;
        Ok(byte)
    }

    fn eight(&mut self) -> Result<[u8; 8], String> {
        let end = self.at.checked_add(8).ok_or("a frame offset overflowed")?;
        let bytes = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| "a frame ended early".to_owned())?;
        self.at = end;
        Ok(bytes.try_into().map_err(|_| "a frame ended early")?)
    }

    fn sixteen(&mut self) -> Result<[u8; 16], String> {
        let end = self.at.checked_add(16).ok_or("a frame offset overflowed")?;
        let bytes = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| "a frame ended early".to_owned())?;
        self.at = end;
        Ok(bytes.try_into().map_err(|_| "a frame ended early")?)
    }

    fn word(&mut self) -> Result<u32, String> {
        let end = self.at.checked_add(4).ok_or("a frame offset overflowed")?;
        let bytes = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| "a frame ended early".to_owned())?;
        self.at = end;
        Ok(u32::from_le_bytes(
            bytes.try_into().map_err(|_| "a frame ended early")?,
        ))
    }

    fn length(&mut self) -> Result<usize, String> {
        let length = self.word()?;
        if length > MAX_LENGTH {
            return Err(format!("a frame claims the length {length}"));
        }
        Ok(length as usize)
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(length)
            .ok_or("a frame offset overflowed")?;
        let bytes = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| "a frame ended early".to_owned())?;
        self.at = end;
        Ok(bytes)
    }
}

/// One value as a frame body, for a caller that sends exactly one.
#[must_use]
pub fn frame(value: &Value) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.write(value);
    writer.finish()
}

/// The one value a frame body holds.
///
/// # Errors
/// Returns why the frame does not hold exactly one value.
pub fn unframe(bytes: &[u8]) -> Result<Value, String> {
    let mut reader = Reader::new(bytes);
    let value = reader.read()?;
    if reader.at != bytes.len() {
        return Err(format!(
            "a frame holds {} bytes past its value",
            bytes.len() - reader.at
        ));
    }
    Ok(value)
}

/// A human-readable dump of a frame, for a trace of a call.
#[must_use]
pub fn describe(value: &Value) -> String {
    let mut text = String::new();
    let _ = write_value(&mut text, value, 0);
    text
}

fn write_value(text: &mut String, value: &Value, depth: usize) -> fmt::Result {
    let pad = "  ".repeat(depth);
    match value {
        Value::Seq(values) => {
            writeln!(text, "{pad}[")?;
            for value in values {
                write_value(text, value, depth + 1)?;
            }
            write!(text, "{pad}]")
        }
        Value::Struct(name, fields) => {
            writeln!(text, "{pad}{name} {{")?;
            for field in fields {
                write_value(text, field, depth + 1)?;
            }
            write!(text, "{pad}}}")
        }
        Value::Enum(name, case, payload) => {
            writeln!(text, "{pad}{name}.{case}")?;
            for field in payload {
                write_value(text, field, depth + 1)?;
            }
            Ok(())
        }
        Value::Bytes(bytes) => write!(text, "{pad}{} bytes", bytes.len()),
        other => write!(text, "{pad}{other}"),
    }
}
