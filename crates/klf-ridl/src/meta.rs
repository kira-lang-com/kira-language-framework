//! Module metadata blob (section 24.3) and linking checks (section 24.5).

use crate::emit::canonical_text;
use crate::module::Module;
use crate::sha256::sha256;

pub type Uuid = [u8; 16];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbiStatus {
    Current,
    Supported,
    Retired,
}

impl AbiStatus {
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            AbiStatus::Current => 0,
            AbiStatus::Supported => 1,
            AbiStatus::Retired => 2,
        }
    }

    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => AbiStatus::Current,
            1 => AbiStatus::Supported,
            2 => AbiStatus::Retired,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbiEntry {
    pub uuid: Uuid,
    pub status: AbiStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportReq {
    pub module: String,
    pub required: Uuid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    pub module_name: String,
    pub abis: Vec<AbiEntry>,
    pub imports: Vec<ImportReq>,
    pub sha256: [u8; 32],
    pub unmanaged: Vec<String>,
}

const MAGIC: &[u8; 4] = b"RIDL";

impl Metadata {
    /// Build metadata for a module, hashing its canonical text. `abis` and
    /// `unmanaged` come from the ABI history and the emitter.
    #[must_use]
    pub fn for_module(
        module: &Module,
        abis: Vec<AbiEntry>,
        imports: Vec<ImportReq>,
        unmanaged: Vec<String>,
    ) -> Self {
        let sha = sha256(canonical_text(module).as_bytes());
        Self {
            module_name: module.name.clone(),
            abis,
            imports,
            sha256: sha,
            unmanaged,
        }
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        write_string(&mut b, &self.module_name);
        write_u32(&mut b, self.abis.len());
        for abi in &self.abis {
            b.extend_from_slice(&abi.uuid);
            b.push(abi.status.code());
        }
        write_u32(&mut b, self.imports.len());
        for import in &self.imports {
            write_string(&mut b, &import.module);
            b.extend_from_slice(&import.required);
        }
        b.extend_from_slice(&self.sha256);
        write_u32(&mut b, self.unmanaged.len());
        for name in &self.unmanaged {
            write_string(&mut b, name);
        }
        b
    }

    /// Decode a metadata blob.
    ///
    /// # Errors
    /// Returns [`MetaError`] on a bad magic, an unknown status byte, or a
    /// truncated blob.
    pub fn decode(bytes: &[u8]) -> Result<Self, MetaError> {
        let mut c = Cursor { bytes, pos: 0 };
        if c.take(4)? != MAGIC {
            return Err(MetaError::BadMagic);
        }
        let _major = c.u16()?;
        let _minor = c.u16()?;
        let module_name = c.string()?;
        let abi_count = c.u32()?;
        let mut abis = Vec::with_capacity(abi_count);
        for _ in 0..abi_count {
            let uuid = c.uuid()?;
            let status = AbiStatus::from_code(c.byte()?).ok_or(MetaError::BadStatus)?;
            abis.push(AbiEntry { uuid, status });
        }
        let import_count = c.u32()?;
        let mut imports = Vec::with_capacity(import_count);
        for _ in 0..import_count {
            let module = c.string()?;
            let required = c.uuid()?;
            imports.push(ImportReq { module, required });
        }
        let sha_slice = c.take(32)?;
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(sha_slice);
        let unmanaged_count = c.u32()?;
        let mut unmanaged = Vec::with_capacity(unmanaged_count);
        for _ in 0..unmanaged_count {
            unmanaged.push(c.string()?);
        }
        Ok(Self {
            module_name,
            abis,
            imports,
            sha256,
            unmanaged,
        })
    }

    #[must_use]
    pub fn current_uuid(&self) -> Option<Uuid> {
        self.abis
            .iter()
            .find(|a| a.status == AbiStatus::Current)
            .map(|a| a.uuid)
    }

    #[must_use]
    pub fn status_of(&self, uuid: &Uuid) -> Option<AbiStatus> {
        self.abis.iter().find(|a| &a.uuid == uuid).map(|a| a.status)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetaError {
    BadMagic,
    BadStatus,
    Truncated,
}

impl std::fmt::Display for MetaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetaError::BadMagic => f.write_str("metadata blob has a bad magic"),
            MetaError::BadStatus => f.write_str("metadata blob has an unknown ABI status"),
            MetaError::Truncated => f.write_str("metadata blob is truncated"),
        }
    }
}

impl std::error::Error for MetaError {}

/// The result of checking one import against its provider (section 24.5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkCheck {
    /// The required UUID is current or supported.
    Ok,
    /// The required UUID is retired: warning `KLF-RT-0201`, proceed unless
    /// `KLF_RIDL_STRICT`.
    RetiredWarning { proceed: bool },
    /// The required UUID is absent: fatal `KLF-RT-0200`.
    Missing,
}

/// Check whether `provider` satisfies a required UUID (section 24.5).
#[must_use]
pub fn check_import(provider: &Metadata, required: &Uuid, strict: bool) -> LinkCheck {
    match provider.status_of(required) {
        Some(AbiStatus::Current | AbiStatus::Supported) => LinkCheck::Ok,
        Some(AbiStatus::Retired) => LinkCheck::RetiredWarning { proceed: !strict },
        None => LinkCheck::Missing,
    }
}

/// Two loaded copies of one module with different current UUIDs fail with
/// `KLF-RT-0202` (section 24.5).
#[must_use]
pub fn conflicting_current(a: &Metadata, b: &Metadata) -> bool {
    a.module_name == b.module_name && a.current_uuid() != b.current_uuid()
}

/// Parse a hex UUID (up to 32 hex digits; a 16-digit abitag is left-justified).
/// Returns `None` on a non-hex digit or more than 32 digits.
#[must_use]
pub fn parse_uuid(hex: &str) -> Option<Uuid> {
    if hex.len() > 32 {
        return None;
    }
    let mut padded = hex.to_owned();
    while padded.len() < 32 {
        padded.push('0');
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = hex_digit(padded.as_bytes()[i * 2])?;
        let lo = hex_digit(padded.as_bytes()[i * 2 + 1])?;
        *byte = (hi << 4) | lo;
    }
    Some(out)
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn write_u32(b: &mut Vec<u8>, n: usize) {
    let n = u32::try_from(n).unwrap_or(u32::MAX);
    b.extend_from_slice(&n.to_le_bytes());
}

fn write_string(b: &mut Vec<u8>, s: &str) {
    write_u32(b, s.len());
    b.extend_from_slice(s.as_bytes());
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], MetaError> {
        let end = self.pos.checked_add(n).ok_or(MetaError::Truncated)?;
        if end > self.bytes.len() {
            return Err(MetaError::Truncated);
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, MetaError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, MetaError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<usize, MetaError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }

    fn uuid(&mut self) -> Result<Uuid, MetaError> {
        let mut u = [0u8; 16];
        u.copy_from_slice(self.take(16)?);
        Ok(u)
    }

    fn string(&mut self) -> Result<String, MetaError> {
        let len = self.u32()?;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| MetaError::Truncated)
    }
}
