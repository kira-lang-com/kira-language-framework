use std::fmt;
use std::io::{Cursor, Read, Write};

use tir_core::{ContentHash, ContentHasher};
use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_target::{Platform, Profile};

use crate::strings::{StringTable, StringTableError, StringTableValidator};

pub const TIRB_HEADER_SIZE: usize = 64;
const SECTION_ENTRY_SIZE: usize = 32;
const SECTION_COMPRESSION: u32 = 1;
const HEADER_FLAGS_MASK: u32 = 0x7f;
const HEADER_CANONICAL: u32 = 1 << 6;
const MAX_SECTION_SIZE: u64 = 1 << 40;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub year: u8,
    pub month: u8,
    pub week: u8,
    pub revision: u8,
    pub flags: u32,
    pub profile: Profile,
    pub platform: Platform,
    pub section_count: u32,
    pub file_size: u64,
}

impl Header {
    #[must_use]
    pub const fn claims_canonical(self) -> bool {
        self.flags & HEADER_CANONICAL != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Section {
    pub kind: u32,
    pub flags: u32,
    pub uncompressed_size: u64,
    stored: Vec<u8>,
}

impl Section {
    #[must_use]
    pub fn from_uncompressed(kind: u32, stored: Vec<u8>) -> Self {
        Self {
            kind,
            flags: 0,
            uncompressed_size: stored.len() as u64,
            stored,
        }
    }

    #[must_use]
    pub fn is_compressed(&self) -> bool {
        self.flags & SECTION_COMPRESSION != 0
    }

    #[must_use]
    pub fn stored_bytes(&self) -> &[u8] {
        &self.stored
    }

    pub fn write_uncompressed(&self, output: &mut impl Write) -> Result<u64, FormatError> {
        self.for_each_uncompressed_chunk(|chunk| {
            output
                .write_all(chunk)
                .map_err(|error| FormatError::malformed(None, error.to_string()))
        })
    }

    fn for_each_uncompressed_chunk(
        &self,
        mut consume: impl FnMut(&[u8]) -> Result<(), FormatError>,
    ) -> Result<u64, FormatError> {
        if !self.is_compressed() {
            if self.stored.len() as u64 != self.uncompressed_size {
                return Err(FormatError::malformed(
                    None,
                    "section size does not match its declaration",
                ));
            }
            consume(&self.stored)?;
            return Ok(self.stored.len() as u64);
        }

        let decoder = zstd::stream::read::Decoder::new(Cursor::new(self.stored.as_slice()))
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
        let limit = self
            .uncompressed_size
            .checked_add(1)
            .ok_or_else(|| FormatError::malformed(None, "section size overflow"))?;
        let mut reader = decoder.take(limit);
        let mut buffer = [0u8; 16 * 1024];
        let mut written = 0u64;
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|error| FormatError::malformed(None, error.to_string()))?;
            if count == 0 {
                break;
            }
            written = written
                .checked_add(count as u64)
                .ok_or_else(|| FormatError::malformed(None, "section size overflow"))?;
            if written > self.uncompressed_size {
                return Err(FormatError::malformed(
                    None,
                    "decompressed section exceeds its declared size",
                ));
            }
            consume(&buffer[..count])?;
        }
        if written != self.uncompressed_size {
            return Err(FormatError::malformed(
                None,
                "decompressed section size does not match its declaration",
            ));
        }
        Ok(written)
    }

    pub fn read_uncompressed(&self, limit: u64) -> Result<Vec<u8>, FormatError> {
        if self.uncompressed_size > limit {
            return Err(FormatError::malformed(
                None,
                "section exceeds the caller's memory limit",
            ));
        }
        let capacity = usize::try_from(self.uncompressed_size)
            .map_err(|_| FormatError::malformed(None, "section does not fit in memory"))?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(capacity)
            .map_err(|_| FormatError::malformed(None, "section allocation failed"))?;
        self.write_uncompressed(&mut output)?;
        Ok(output)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryModule {
    pub header: Header,
    pub sections: Vec<Section>,
    pub content_hash: ContentHash,
}

impl BinaryModule {
    pub fn string_table(&self, memory_limit: u64) -> Result<StringTable, FormatError> {
        let section = self
            .sections
            .iter()
            .find(|section| section.kind == 1)
            .ok_or_else(|| FormatError::malformed(None, "STRINGS section is missing"))?;
        let bytes = section.read_uncompressed(memory_limit)?;
        StringTable::decode(&bytes).map_err(map_string_error)
    }

    /// Writes the container with sorted, uncompressed, contiguous sections.
    /// When the canonical flag is set, each section payload must already have
    /// its spec-defined canonical contents.
    pub fn write_uncompressed_container(
        &self,
        output: &mut impl Write,
    ) -> Result<(Header, ContentHash), FormatError> {
        if !(1..=99).contains(&self.header.year)
            || !(1..=12).contains(&self.header.month)
            || !(1..=5).contains(&self.header.week)
            || self.header.revision != 0
            || self.header.flags & !HEADER_FLAGS_MASK != 0
        {
            return Err(FormatError::malformed(None, "invalid module header fields"));
        }

        let section_count = u32::try_from(self.sections.len())
            .map_err(|_| FormatError::malformed(None, "too many sections"))?;
        let table_size = u64::from(section_count)
            .checked_mul(SECTION_ENTRY_SIZE as u64)
            .ok_or_else(|| FormatError::malformed(None, "section table size overflow"))?;
        let mut next_offset = (TIRB_HEADER_SIZE as u64)
            .checked_add(table_size)
            .ok_or_else(|| FormatError::malformed(None, "file size overflow"))?;
        let mut sections: Vec<_> = self.sections.iter().collect();
        sections.sort_by_key(|section| section.kind);
        let mut previous_kind = None;
        for section in &sections {
            if !known_section(section.kind) {
                return Err(FormatError::malformed(None, "unknown section kind"));
            }
            if previous_kind == Some(section.kind) && section.kind != 255 {
                return Err(FormatError::malformed(None, "duplicate section kind"));
            }
            if section.flags & !SECTION_COMPRESSION != 0 {
                return Err(FormatError::malformed(
                    None,
                    "reserved section flags are set",
                ));
            }
            if section.uncompressed_size >= MAX_SECTION_SIZE {
                return Err(FormatError::malformed(
                    None,
                    "section exceeds the size limit",
                ));
            }
            previous_kind = Some(section.kind);
            next_offset = next_offset
                .checked_add(section.uncompressed_size)
                .ok_or_else(|| FormatError::malformed(None, "file size overflow"))?;
        }
        validate_required_sections(sections.iter().copied())?;

        let mut table = Vec::new();
        table
            .try_reserve_exact(usize::try_from(table_size).map_err(|_| {
                FormatError::malformed(None, "section table does not fit in memory")
            })?)
            .map_err(|_| FormatError::malformed(None, "section table allocation failed"))?;
        let mut offset = (TIRB_HEADER_SIZE as u64) + table_size;
        for section in &sections {
            table.extend_from_slice(&section.kind.to_le_bytes());
            table.extend_from_slice(&0u32.to_le_bytes());
            table.extend_from_slice(&offset.to_le_bytes());
            table.extend_from_slice(&section.uncompressed_size.to_le_bytes());
            table.extend_from_slice(&section.uncompressed_size.to_le_bytes());
            offset += section.uncompressed_size;
        }

        let mut hasher = ContentHasher::new();
        hasher.update(&table);
        for section in &sections {
            if section.kind == 1 {
                validate_strings(section)?;
            }
            section.for_each_uncompressed_chunk(|chunk| {
                hasher.update(chunk);
                Ok(())
            })?;
        }
        let content_hash = hasher.finish();
        let header = Header {
            section_count,
            file_size: next_offset,
            ..self.header
        };
        let header_bytes = encode_header(header, content_hash);
        output
            .write_all(&header_bytes)
            .and_then(|()| output.write_all(&table))
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
        for section in &sections {
            section.write_uncompressed(output)?;
        }
        Ok((header, content_hash))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < TIRB_HEADER_SIZE {
            return Err(FormatError::malformed(
                Some(bytes.len()),
                "truncated header",
            ));
        }
        if bytes[..4] != *b"TIRB" {
            return Err(FormatError::malformed(Some(0), "invalid TIRB magic"));
        }

        let year = bytes[4];
        let month = bytes[5];
        let week = bytes[6];
        let revision = bytes[7];
        if !(1..=99).contains(&year)
            || !(1..=12).contains(&month)
            || !(1..=5).contains(&week)
            || revision != 0
        {
            return Err(FormatError::malformed(
                Some(4),
                "invalid TIR version or encoding revision",
            ));
        }
        let flags = read_u32(bytes, 8)?;
        if flags & !HEADER_FLAGS_MASK != 0 {
            return Err(FormatError::malformed(
                Some(8),
                "reserved header flags are set",
            ));
        }
        let profile = Profile::from_code(bytes[12])
            .ok_or_else(|| FormatError::malformed(Some(12), "unknown profile code"))?;
        let platform = Platform::from_code(bytes[13])
            .ok_or_else(|| FormatError::malformed(Some(13), "unknown platform code"))?;
        if bytes[14] != 0 || bytes[15] != 0 || read_u32(bytes, 20)? != 0 {
            return Err(FormatError::malformed(
                Some(14),
                "reserved header bytes are nonzero",
            ));
        }
        let section_count = read_u32(bytes, 16)?;
        let file_size = read_u64(bytes, 56)?;
        if file_size != bytes.len() as u64 {
            return Err(FormatError::malformed(
                Some(56),
                "declared file size does not match the input",
            ));
        }
        let content_hash = ContentHash::of(&bytes[TIRB_HEADER_SIZE..]);
        if content_hash.as_bytes().as_slice() != &bytes[24..56] {
            return Err(FormatError::malformed(Some(24), "content hash mismatch"));
        }

        let table_bytes = usize::try_from(section_count)
            .ok()
            .and_then(|count| count.checked_mul(SECTION_ENTRY_SIZE))
            .ok_or_else(|| FormatError::malformed(Some(16), "section table size overflow"))?;
        let table_end = TIRB_HEADER_SIZE
            .checked_add(table_bytes)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| FormatError::malformed(Some(16), "truncated section table"))?;

        let mut sections = Vec::with_capacity(section_count as usize);
        let mut previous_kind = 0u32;
        let mut table_offsets = Vec::with_capacity(section_count as usize);
        let mut kinds = std::collections::HashSet::new();
        for index in 0..section_count as usize {
            let entry = TIRB_HEADER_SIZE + index * SECTION_ENTRY_SIZE;
            let kind = read_u32(bytes, entry)?;
            let section_flags = read_u32(bytes, entry + 4)?;
            let offset = read_u64(bytes, entry + 8)?;
            let stored_size = read_u64(bytes, entry + 16)?;
            let uncompressed_size = read_u64(bytes, entry + 24)?;
            if kind < previous_kind {
                return Err(FormatError::malformed(
                    Some(entry),
                    "section table is not sorted by kind",
                ));
            }
            previous_kind = kind;
            if !known_section(kind) {
                return Err(FormatError::malformed(Some(entry), "unknown section kind"));
            }
            if kind != 255 && !kinds.insert(kind) {
                return Err(FormatError::malformed(
                    Some(entry),
                    "duplicate section kind",
                ));
            }
            if section_flags & !SECTION_COMPRESSION != 0 {
                return Err(FormatError::malformed(
                    Some(entry + 4),
                    "reserved section flags are set",
                ));
            }
            if uncompressed_size >= MAX_SECTION_SIZE {
                return Err(FormatError::malformed(
                    Some(entry + 24),
                    "section exceeds the size limit",
                ));
            }
            if section_flags & SECTION_COMPRESSION == 0 && stored_size != uncompressed_size {
                return Err(FormatError::malformed(
                    Some(entry + 16),
                    "uncompressed section sizes differ",
                ));
            }
            let start = usize::try_from(offset).map_err(|_| {
                FormatError::malformed(Some(entry + 8), "section offset does not fit in memory")
            })?;
            let stored_len = usize::try_from(stored_size).map_err(|_| {
                FormatError::malformed(Some(entry + 16), "section size does not fit in memory")
            })?;
            let end = start
                .checked_add(stored_len)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| {
                    FormatError::malformed(Some(entry + 8), "section extends beyond the file")
                })?;
            if start < table_end {
                return Err(FormatError::malformed(
                    Some(entry + 8),
                    "section overlaps the header or section table",
                ));
            }
            table_offsets.push((start, end, entry));
            sections.push(Section {
                kind,
                flags: section_flags,
                uncompressed_size,
                stored: bytes[start..end].to_vec(),
            });
        }

        let mut offsets = table_offsets.clone();
        offsets.sort_by_key(|entry| entry.0);
        let mut previous_end = table_end;
        for (start, end, entry) in &offsets {
            if *start < previous_end {
                return Err(FormatError::malformed(Some(*entry + 8), "sections overlap"));
            }
            previous_end = *end;
        }

        let header = Header {
            year,
            month,
            week,
            revision,
            flags,
            profile,
            platform,
            section_count,
            file_size,
        };
        if header.claims_canonical() {
            if sections.iter().any(Section::is_compressed) {
                return Err(FormatError::noncanonical(
                    Some(8),
                    "canonical sections must be uncompressed",
                ));
            }
            validate_canonical_layout(table_end, &table_offsets, bytes.len())?;
        }
        for section in sections.iter().filter(|section| section.kind == 1) {
            validate_strings(section)?;
        }
        validate_required_sections(sections.iter())?;

        Ok(Self {
            header,
            sections,
            content_hash,
        })
    }
}

fn encode_header(header: Header, content_hash: ContentHash) -> [u8; TIRB_HEADER_SIZE] {
    let mut bytes = [0; TIRB_HEADER_SIZE];
    bytes[..4].copy_from_slice(b"TIRB");
    bytes[4] = header.year;
    bytes[5] = header.month;
    bytes[6] = header.week;
    bytes[7] = header.revision;
    bytes[8..12].copy_from_slice(&header.flags.to_le_bytes());
    bytes[12] = header.profile.code();
    bytes[13] = header.platform.code();
    bytes[16..20].copy_from_slice(&header.section_count.to_le_bytes());
    bytes[24..56].copy_from_slice(content_hash.as_bytes());
    bytes[56..64].copy_from_slice(&header.file_size.to_le_bytes());
    bytes
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatError {
    pub diagnostic: Diagnostic,
    pub offset: Option<usize>,
}

impl FormatError {
    pub(crate) fn malformed(offset: Option<usize>, message: impl Into<String>) -> Self {
        let mut diagnostic = Diagnostic::error(Code::new(Dialect::Tir, Area::Encoding, 1), message);
        if let Some(offset) = offset {
            diagnostic = diagnostic.with_primary_span(Span::new(offset, offset.saturating_add(1)));
        }
        Self { diagnostic, offset }
    }

    pub(crate) fn noncanonical(offset: Option<usize>, message: impl Into<String>) -> Self {
        let mut diagnostic = Diagnostic::error(Code::new(Dialect::Tir, Area::Encoding, 2), message);
        if let Some(offset) = offset {
            diagnostic = diagnostic.with_primary_span(Span::new(offset, offset.saturating_add(1)));
        }
        Self { diagnostic, offset }
    }
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.diagnostic, formatter)
    }
}

impl std::error::Error for FormatError {}

fn validate_strings(section: &Section) -> Result<(), FormatError> {
    let mut validator = StringTableValidator::new(section.uncompressed_size);
    section.for_each_uncompressed_chunk(|chunk| validator.push(chunk).map_err(map_string_error))?;
    validator.finish().map_err(map_string_error)
}

fn validate_required_sections<'a>(
    sections: impl IntoIterator<Item = &'a Section>,
) -> Result<(), FormatError> {
    let mut strings = false;
    let mut module = false;
    let mut analysis = false;
    for section in sections {
        strings |= section.kind == 1;
        module |= section.kind == 2;
        analysis |= section.kind == 14;
    }
    if !strings {
        return Err(FormatError::malformed(None, "STRINGS section is missing"));
    }
    if !module {
        return Err(FormatError::malformed(None, "MODULE section is missing"));
    }
    if !analysis {
        return Err(FormatError::malformed(None, "ANALYSIS section is missing"));
    }
    Ok(())
}

fn map_string_error(error: StringTableError) -> FormatError {
    FormatError::malformed(usize::try_from(error.offset).ok(), error.message)
}

fn validate_canonical_layout(
    table_end: usize,
    offsets: &[(usize, usize, usize)],
    file_size: usize,
) -> Result<(), FormatError> {
    let mut next = table_end;
    for (start, end, entry) in offsets {
        if *start != next {
            return Err(FormatError::noncanonical(
                Some(*entry + 8),
                "canonical sections must be contiguous and have no padding",
            ));
        }
        next = *end;
    }
    if next != file_size {
        return Err(FormatError::noncanonical(
            Some(next),
            "canonical file has trailing bytes",
        ));
    }
    Ok(())
}

fn known_section(kind: u32) -> bool {
    (1..=18).contains(&kind) || kind == 255
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, FormatError> {
    let end = offset
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| FormatError::malformed(Some(offset), "truncated u32"))?;
    let mut value = [0; 4];
    value.copy_from_slice(&bytes[offset..end]);
    Ok(u32::from_le_bytes(value))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, FormatError> {
    let end = offset
        .checked_add(8)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| FormatError::malformed(Some(offset), "truncated u64"))?;
    let mut value = [0; 8];
    value.copy_from_slice(&bytes[offset..end]);
    Ok(u64::from_le_bytes(value))
}
