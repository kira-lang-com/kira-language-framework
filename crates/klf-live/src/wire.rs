//! The section 17.5 hot-push wire: frames on a TCP connection between the
//! client that compiles and the device that runs.
//!
//! A frame is `u32 length, u8 type, payload`, as section 17.5 specifies. The
//! payload of `PUSH_MODULE` is a module's KIR rather than its bytecode: KIR is
//! the portable form, and the device lowers it with the toolchain it already
//! runs, so a push survives a KLF release that changed the bytecode encoding.
//!
//! The client opens the exchange with `HELLO`, naming the protocol version and
//! the bundle it speaks for; the device answers with its own `HELLO` carrying
//! the layout hash of every module it has loaded. Both sides compare the
//! hashes before anything is pushed, which is what catches a layout change
//! while it is still cheap to refuse.

use klf_ridl::sha256::{hex, sha256};
use std::fmt;

/// The protocol version this implementation speaks. A device refuses a client
/// whose major version differs, and tolerates a higher minor on either side.
pub const VERSION: u16 = 1;

/// Refusing a push because of its content (section 17.5's reason codes).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    /// A global's or an exported type's layout hash differs; the device must
    /// restart to adopt it.
    LayoutChanged = 1,
    /// A function the native half calls changed signature.
    SignatureChanged = 2,
    /// The pushed module does not verify.
    VerifyFailed = 3,
    /// The two sides are not speaking the same protocol.
    VersionMismatch = 4,
    /// Both code versions are still running, so a third cannot be installed.
    OldCodeBusy = 5,
    /// The device does not have the module the push names.
    UnknownModule = 6,
}

impl Reason {
    #[must_use]
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::LayoutChanged,
            2 => Self::SignatureChanged,
            3 => Self::VerifyFailed,
            4 => Self::VersionMismatch,
            5 => Self::OldCodeBusy,
            6 => Self::UnknownModule,
            _ => return None,
        })
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::LayoutChanged => "layout changed; the device must restart",
            Self::SignatureChanged => "a function the native half calls changed signature",
            Self::VerifyFailed => "the pushed module does not verify",
            Self::VersionMismatch => "protocol version mismatch",
            Self::OldCodeBusy => "both code versions are still running",
            Self::UnknownModule => "no such module is loaded",
        })
    }
}

/// One message on the wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Frame {
    /// The opening exchange. The client sends it first with its own version and
    /// bundle; the device answers with its version and the layout hashes of the
    /// modules it holds.
    Hello(Hello),
    /// A module's KIR, to lower and install.
    Push(Push),
    /// A module was installed; the count is how many functions it replaced.
    Ack(Ack),
    /// A module was refused; the reason and the device's own words.
    Nack(Nack),
    /// Text from the device, for a live log.
    Log(String),
}

/// The `HELLO` payload: a protocol version, a KLF version, a bundle id, and
/// the layout hash of every module the sender holds.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Hello {
    pub version: u16,
    pub klf_version: String,
    pub bundle: String,
    pub layouts: Vec<(String, String)>,
}

impl Hello {
    /// The layout hash `module` has, if this hello carries one.
    #[must_use]
    pub fn layout_of(&self, module: &str) -> Option<&str> {
        self.layouts
            .iter()
            .find(|(name, _)| name == module)
            .map(|(_, hash)| hash.as_str())
    }

    /// Whether the two sides agree on the module `name`'s layout. A hello
    /// carrying no entry for it means the module is new, which is allowed.
    #[must_use]
    pub fn agrees_on(&self, other: &Hello, name: &str) -> bool {
        match (self.layout_of(name), other.layout_of(name)) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => true,
        }
    }
}

/// The `PUSH_MODULE` payload: a module's name and its KIR text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Push {
    pub module: String,
    pub kir: String,
}

/// The `ACK` payload: the module and how many functions the push replaced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ack {
    pub module: String,
    pub replaced: u32,
}

/// The `NACK` payload: the module, why, and what the device said about it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Nack {
    pub module: String,
    pub reason: Reason,
    pub message: String,
}

/// The frame type byte on the wire.
const TYPE_HELLO: u8 = 1;
const TYPE_PUSH: u8 = 2;
const TYPE_ACK: u8 = 3;
const TYPE_NACK: u8 = 4;
const TYPE_LOG: u8 = 5;

/// The largest frame this protocol reads, so a corrupt length cannot make a
/// device allocate without bound. A module's KIR is the largest thing that
/// legitimately crosses, and 64 MiB leaves room for a very large one.
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// A frame that could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireError(pub String);

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for WireError {}

impl Frame {
    /// The frame's type byte.
    #[must_use]
    pub fn kind(&self) -> u8 {
        match self {
            Self::Hello(_) => TYPE_HELLO,
            Self::Push(_) => TYPE_PUSH,
            Self::Ack(_) => TYPE_ACK,
            Self::Nack(_) => TYPE_NACK,
            Self::Log(_) => TYPE_LOG,
        }
    }

    /// The frame as it goes on the wire: `u32 length, u8 type, payload`, where
    /// the length counts the type byte and the payload.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.push(self.kind());
        match self {
            Self::Hello(hello) => encode_hello(hello, &mut payload),
            Self::Push(push) => {
                write_string(&push.module, &mut payload);
                write_string(&push.kir, &mut payload);
            }
            Self::Ack(ack) => {
                write_string(&ack.module, &mut payload);
                payload.extend_from_slice(&ack.replaced.to_le_bytes());
            }
            Self::Nack(nack) => {
                write_string(&nack.module, &mut payload);
                payload.push(nack.reason as u8);
                write_string(&nack.message, &mut payload);
            }
            Self::Log(text) => write_string(text, &mut payload),
        }
        // A payload past `u32::MAX` cannot be described by the length field, so it
        // saturates rather than panicking: a device must not die because a peer
        // offered it something enormous. `framed_length` then reads the frame
        // as over `MAX_FRAME` and the reader refuses it.
        let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
        let mut frame = Vec::with_capacity(payload.len() + 4);
        frame.extend_from_slice(&length.to_le_bytes());
        frame.extend_from_slice(&payload);
        frame
    }

    /// Reads a frame from `bytes`, which must hold one whole frame.
    ///
    /// # Errors
    /// Returns a message when the frame is truncated, oversized, names an
    /// unknown type, or carries a payload that does not fit its type.
    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() < 4 {
            return Err(WireError("frame is shorter than its length".to_owned()));
        }
        let length = u32::from_le_bytes(bytes[..4].try_into().unwrap_or([0; 4])) as usize;
        if length < 1 {
            return Err(WireError("frame carries no type".to_owned()));
        }
        if length > MAX_FRAME {
            return Err(WireError(format!("frame of {length} bytes is too large")));
        }
        let body = bytes
            .get(4..4 + length)
            .ok_or_else(|| WireError("frame is truncated".to_owned()))?;
        let mut reader = Reader::new(body);
        let kind = reader.byte()?;
        match kind {
            TYPE_HELLO => Ok(Self::Hello(decode_hello(&mut reader)?)),
            TYPE_PUSH => Ok(Self::Push(Push {
                module: reader.string()?,
                kir: reader.string()?,
            })),
            TYPE_ACK => Ok(Self::Ack(Ack {
                module: reader.string()?,
                replaced: reader.u32()?,
            })),
            TYPE_NACK => Ok(Self::Nack(Nack {
                module: reader.string()?,
                reason: Reason::from_code(reader.byte()?)
                    .ok_or_else(|| WireError("unknown reason code".to_owned()))?,
                message: reader.string()?,
            })),
            TYPE_LOG => Ok(Self::Log(reader.string()?)),
            other => Err(WireError(format!("unknown frame type {other}"))),
        }
    }

    /// How many bytes the frame at the head of `bytes` occupies, or `None` when
    /// more bytes are needed. A reader uses this to frame a stream.
    #[must_use]
    pub fn framed_length(bytes: &[u8]) -> Option<usize> {
        let header: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
        let length = u32::from_le_bytes(header) as usize;
        if length > MAX_FRAME {
            return None;
        }
        let total = length.checked_add(4)?;
        (bytes.len() >= total).then_some(total)
    }
}

fn encode_hello(hello: &Hello, payload: &mut Vec<u8>) {
    payload.extend_from_slice(&hello.version.to_le_bytes());
    write_string(&hello.klf_version, payload);
    write_string(&hello.bundle, payload);
    payload.extend_from_slice(
        &u32::try_from(hello.layouts.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    for (module, hash) in &hello.layouts {
        write_string(module, payload);
        write_string(hash, payload);
    }
}

fn decode_hello(reader: &mut Reader<'_>) -> Result<Hello, WireError> {
    let version = reader.u16()?;
    let klf_version = reader.string()?;
    let bundle = reader.string()?;
    let count = reader.u32()? as usize;
    let mut layouts = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        layouts.push((reader.string()?, reader.string()?));
    }
    Ok(Hello {
        version,
        klf_version,
        bundle,
        layouts,
    })
}

fn write_string(value: &str, payload: &mut Vec<u8>) {
    payload.extend_from_slice(&u32::try_from(value.len()).unwrap_or(u32::MAX).to_le_bytes());
    payload.extend_from_slice(value.as_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], WireError> {
        let end = self
            .position
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| WireError("payload is truncated".to_owned()))?;
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn string(&mut self) -> Result<String, WireError> {
        let length = self.u32()? as usize;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| WireError("string is not UTF-8".to_owned()))
    }
}

/// The layout fingerprint of a module: the shape a running device cannot hot
/// swap past. It covers every global's size and alignment and every exported
/// type's printed form, never the function bodies, so an edit to a body leaves
/// it unchanged and a push of it is allowed.
#[must_use]
pub fn layout_hash(module: &klf_kir::Module) -> String {
    let mut text = String::new();
    for global in &module.globals {
        text.push_str("global ");
        text.push_str(global.name.as_str());
        text.push(' ');
        text.push_str(&global.ty.to_string());
        text.push('\n');
    }
    for record in &module.exports {
        text.push_str("export ");
        text.push_str(record.symbol.as_str());
        text.push(' ');
        text.push_str(&record.ty.to_string());
        text.push('\n');
    }
    hex(&sha256(text.as_bytes()))
}
