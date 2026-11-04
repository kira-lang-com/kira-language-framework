use std::fmt;
use std::io::{Cursor, Read, Write};

use tir_core::{ContentHash, ContentHasher};
use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::{StringTable, StringTableError, StringTableValidator};
use tir_target::Profile;

use crate::function_index::FunctionIndexValidator;

pub const TVISA_HEADER_SIZE: usize = 64;
const SECTION_ENTRY_SIZE: usize = 32;
const SECTION_COMPRESSION: u32 = 1;
const HEADER_FLAGS_MASK: u32 = 0xff;
const HEADER_CANONICAL: u32 = 1 << 6;
const RUNTIME_SECTION: u32 = 21;
const MAX_RUNTIME_SECTION_SIZE: u64 = 5 + 65_536 * 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub year: u8,
    pub month: u8,
    pub week: u8,
    pub revision: u8,
    pub flags: u32,
    pub profile: Profile,
    pub section_count: u32,
    pub function_count: u32,
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

    pub(crate) fn for_each_uncompressed_chunk(
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
pub struct TvisaImage {
    pub header: Header,
    pub sections: Vec<Section>,
    pub content_hash: ContentHash,
}

/// Collects canonical section payloads and assembles a TVISA image container.
/// Section encoding and cross-section references remain the caller's job.
#[derive(Clone, Debug)]
pub struct TvisaImageBuilder {
    header: Header,
    sections: Vec<Section>,
}

impl TvisaImageBuilder {
    #[must_use]
    pub fn new(year: u8, month: u8, week: u8, profile: Profile, function_count: u32) -> Self {
        Self {
            header: Header {
                year,
                month,
                week,
                revision: 0,
                flags: 0,
                profile,
                section_count: 0,
                function_count,
                file_size: 0,
            },
            sections: Vec::new(),
        }
    }

    pub fn set_canonical(&mut self, canonical: bool) {
        if canonical {
            self.header.flags |= HEADER_CANONICAL;
        } else {
            self.header.flags &= !HEADER_CANONICAL;
        }
    }

    pub fn add_section(&mut self, section: Section) {
        self.sections.push(section);
    }

    pub fn add_uncompressed_section(&mut self, kind: u32, payload: Vec<u8>) {
        self.add_section(Section::from_uncompressed(kind, payload));
    }

    pub fn finish(self) -> Result<TvisaImage, FormatError> {
        let mut image = TvisaImage {
            header: self.header,
            sections: self.sections,
            content_hash: ContentHash::of(&[]),
        };
        let (header, content_hash) = image.write_uncompressed_container(&mut std::io::sink())?;
        image.header = header;
        image.content_hash = content_hash;
        Ok(image)
    }

    pub fn write_uncompressed_container(
        self,
        output: &mut impl Write,
    ) -> Result<(Header, ContentHash), FormatError> {
        TvisaImage {
            header: self.header,
            sections: self.sections,
            content_hash: ContentHash::of(&[]),
        }
        .write_uncompressed_container(output)
    }
}

impl TvisaImage {
    pub fn string_table(&self, memory_limit: u64) -> Result<StringTable, FormatError> {
        let section = self
            .sections
            .iter()
            .find(|section| section.kind == 1)
            .ok_or_else(|| FormatError::malformed(None, "STRINGS section is missing"))?;
        let bytes = section.read_uncompressed(memory_limit)?;
        StringTable::decode(&bytes).map_err(map_string_error)
    }

    pub fn runtime_entry_ids(&self) -> Result<Vec<u16>, FormatError> {
        self.sections
            .iter()
            .find(|section| section.kind == RUNTIME_SECTION)
            .map_or(Ok(Vec::new()), decode_runtime_entries)
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
            || self.header.week > 5
            || self.header.revision != 0
            || self.header.flags & !HEADER_FLAGS_MASK != 0
        {
            return Err(FormatError::malformed(None, "invalid image header fields"));
        }
        if self.header.function_count >= 1 << 28 {
            return Err(FormatError::malformed(
                None,
                "function count exceeds the image limit",
            ));
        }

        let section_count = u32::try_from(self.sections.len())
            .map_err(|_| FormatError::malformed(None, "too many sections"))?;
        if section_count > 21 {
            return Err(FormatError::malformed(
                None,
                "section count exceeds the defined section kinds",
            ));
        }
        let table_size = u64::from(section_count)
            .checked_mul(SECTION_ENTRY_SIZE as u64)
            .ok_or_else(|| FormatError::malformed(None, "section table size overflow"))?;
        let mut next_offset = (TVISA_HEADER_SIZE as u64)
            .checked_add(table_size)
            .ok_or_else(|| FormatError::malformed(None, "file size overflow"))?;
        let mut sections: Vec<_> = self.sections.iter().collect();
        sections.sort_by_key(|section| section.kind);
        let mut previous_kind = None;
        for section in &sections {
            if !known_section(section.kind) {
                return Err(FormatError::unknown_section(None, "unknown section kind"));
            }
            if previous_kind == Some(section.kind) {
                return Err(FormatError::malformed(None, "duplicate section kind"));
            }
            if section.flags & !SECTION_COMPRESSION != 0 {
                return Err(FormatError::malformed(
                    None,
                    "reserved section flags are set",
                ));
            }
            previous_kind = Some(section.kind);
            next_offset = next_offset
                .checked_add(section.uncompressed_size)
                .ok_or_else(|| FormatError::malformed(None, "file size overflow"))?;
        }

        validate_required_sections(&self.sections)?;
        validate_runtime_section(&self.sections)?;
        validate_function_index(
            self.header.function_count,
            &self.sections,
            self.header.claims_canonical(),
        )?;
        let mut table = Vec::new();
        table
            .try_reserve_exact(usize::try_from(table_size).map_err(|_| {
                FormatError::malformed(None, "section table does not fit in memory")
            })?)
            .map_err(|_| FormatError::malformed(None, "section table allocation failed"))?;
        let mut offset = (TVISA_HEADER_SIZE as u64) + table_size;
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
        if bytes.len() < TVISA_HEADER_SIZE {
            return Err(FormatError::malformed(
                Some(bytes.len()),
                "truncated header",
            ));
        }
        if bytes[..4] != *b"TVIS" {
            return Err(FormatError::malformed(Some(0), "invalid TVIS magic"));
        }

        let year = bytes[4];
        let month = bytes[5];
        let week = bytes[6];
        let revision = bytes[7];
        if !(1..=99).contains(&year) || !(1..=12).contains(&month) || week > 5 || revision != 0 {
            return Err(FormatError::malformed(
                Some(4),
                "invalid TVISA version or encoding revision",
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
        if bytes[13..16].iter().any(|byte| *byte != 0) {
            return Err(FormatError::malformed(
                Some(13),
                "reserved header bytes are nonzero",
            ));
        }
        let section_count = read_u32(bytes, 16)?;
        let function_count = read_u32(bytes, 20)?;
        if section_count > 21 {
            return Err(FormatError::malformed(
                Some(16),
                "section count exceeds the defined section kinds",
            ));
        }
        if function_count >= 1 << 28 {
            return Err(FormatError::malformed(
                Some(20),
                "function count exceeds the image limit",
            ));
        }
        let file_size = read_u64(bytes, 56)?;
        if file_size != bytes.len() as u64 {
            return Err(FormatError::malformed(
                Some(56),
                "declared file size does not match the input",
            ));
        }
        let content_hash = ContentHash::of(&bytes[TVISA_HEADER_SIZE..]);
        if content_hash.as_bytes().as_slice() != &bytes[24..56] {
            return Err(FormatError::malformed(Some(24), "content hash mismatch"));
        }

        let table_bytes = usize::try_from(section_count)
            .ok()
            .and_then(|count| count.checked_mul(SECTION_ENTRY_SIZE))
            .ok_or_else(|| FormatError::malformed(Some(16), "section table size overflow"))?;
        let table_end = TVISA_HEADER_SIZE
            .checked_add(table_bytes)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| FormatError::malformed(Some(16), "truncated section table"))?;

        let mut sections = Vec::with_capacity(section_count as usize);
        let mut table_offsets = Vec::with_capacity(section_count as usize);
        let mut previous_kind = 0u32;
        for index in 0..section_count as usize {
            let entry = TVISA_HEADER_SIZE + index * SECTION_ENTRY_SIZE;
            let kind = read_u32(bytes, entry)?;
            let section_flags = read_u32(bytes, entry + 4)?;
            let offset = read_u64(bytes, entry + 8)?;
            let stored_size = read_u64(bytes, entry + 16)?;
            let uncompressed_size = read_u64(bytes, entry + 24)?;
            if !known_section(kind) {
                return Err(FormatError::unknown_section(
                    Some(entry),
                    "unknown section kind",
                ));
            }
            if kind <= previous_kind {
                return Err(FormatError::malformed(
                    Some(entry),
                    "section table is not strictly sorted by kind",
                ));
            }
            previous_kind = kind;
            if section_flags & !SECTION_COMPRESSION != 0 {
                return Err(FormatError::malformed(
                    Some(entry + 4),
                    "reserved section flags are set",
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
            section_count,
            function_count,
            file_size,
        };
        if header.claims_canonical() {
            if sections.iter().any(Section::is_compressed) {
                return Err(FormatError::noncanonical(
                    Some(8),
                    "canonical sections must be uncompressed",
                ));
            }
            validate_canonical_layout(&table_offsets, table_end, bytes.len())?;
        }
        for section in sections.iter().filter(|section| section.kind == 1) {
            validate_strings(section)?;
        }
        validate_required_sections(&sections)?;
        validate_runtime_section(&sections)?;
        validate_function_index(function_count, &sections, header.claims_canonical())?;

        Ok(Self {
            header,
            sections,
            content_hash,
        })
    }
}

fn encode_header(header: Header, content_hash: ContentHash) -> [u8; TVISA_HEADER_SIZE] {
    let mut bytes = [0; TVISA_HEADER_SIZE];
    bytes[..4].copy_from_slice(b"TVIS");
    bytes[4] = header.year;
    bytes[5] = header.month;
    bytes[6] = header.week;
    bytes[7] = header.revision;
    bytes[8..12].copy_from_slice(&header.flags.to_le_bytes());
    bytes[12] = header.profile.code();
    bytes[16..20].copy_from_slice(&header.section_count.to_le_bytes());
    bytes[20..24].copy_from_slice(&header.function_count.to_le_bytes());
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
        Self::coded(offset, 1, message)
    }

    pub(crate) fn noncanonical(offset: Option<usize>, message: impl Into<String>) -> Self {
        Self::coded(offset, 2, message)
    }

    pub(crate) fn unknown_section(offset: Option<usize>, message: impl Into<String>) -> Self {
        Self::coded(offset, 4, message)
    }

    pub(crate) fn unsupported_type(offset: Option<usize>, message: impl Into<String>) -> Self {
        Self::coded(offset, 3, message)
    }

    fn coded(offset: Option<usize>, number: u16, message: impl Into<String>) -> Self {
        let mut diagnostic =
            Diagnostic::error(Code::new(Dialect::Tvisa, Area::Encoding, number), message);
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
    section.for_each_uncompressed_chunk(|chunk| {
        validator.push(chunk).map_err(|error| {
            FormatError::malformed(usize::try_from(error.offset).ok(), error.message)
        })
    })?;
    validator
        .finish()
        .map_err(|error| FormatError::malformed(usize::try_from(error.offset).ok(), error.message))
}

fn map_string_error(error: StringTableError) -> FormatError {
    FormatError::malformed(usize::try_from(error.offset).ok(), error.message)
}

fn validate_required_sections(sections: &[Section]) -> Result<(), FormatError> {
    for required in [1, 2, 20] {
        if !sections.iter().any(|section| section.kind == required) {
            return Err(FormatError::malformed(
                None,
                if required == 1 {
                    "STRINGS section is missing"
                } else if required == 2 {
                    "IMAGE section is missing"
                } else {
                    "ANALYSIS section is missing"
                },
            ));
        }
    }
    Ok(())
}

fn validate_runtime_section(sections: &[Section]) -> Result<(), FormatError> {
    if let Some(section) = sections
        .iter()
        .find(|section| section.kind == RUNTIME_SECTION)
    {
        decode_runtime_entries(section)?;
    }
    Ok(())
}

fn decode_runtime_entries(section: &Section) -> Result<Vec<u16>, FormatError> {
    let bytes = section.read_uncompressed(MAX_RUNTIME_SECTION_SIZE)?;
    let mut cursor = 0;
    let count = tir_format::leb::read_uleb(&bytes, &mut cursor)
        .map_err(|_| FormatError::malformed(Some(0), "invalid RUNTIME entry count"))?;
    if count == 0 {
        return Err(FormatError::malformed(
            Some(0),
            "RUNTIME must be absent when the image uses no runtime entries",
        ));
    }
    if count > u64::from(u16::MAX) + 1 {
        return Err(FormatError::malformed(
            Some(0),
            "RUNTIME contains more identifiers than the 16-bit space permits",
        ));
    }
    let expected_size = cursor
        .checked_add((count as usize).saturating_mul(2))
        .ok_or_else(|| FormatError::malformed(Some(0), "RUNTIME size overflow"))?;
    if bytes.len() != expected_size {
        return Err(FormatError::malformed(
            Some(cursor),
            "RUNTIME size does not match its identifier count",
        ));
    }
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let id = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]);
        if entries.last().is_some_and(|previous| *previous >= id) {
            return Err(FormatError::malformed(
                Some(cursor),
                "RUNTIME identifiers must be strictly increasing",
            ));
        }
        entries.push(id);
        cursor += 2;
    }
    Ok(entries)
}

fn validate_function_index(
    function_count: u32,
    sections: &[Section],
    canonical: bool,
) -> Result<(), FormatError> {
    let index = sections.iter().find(|section| section.kind == 8);
    let code = sections.iter().find(|section| section.kind == 9);
    let expected_size = u64::from(function_count) * 16;

    if function_count == 0 {
        if index.is_some_and(|section| section.uncompressed_size != 0)
            || code.is_some_and(|section| section.uncompressed_size != 0)
        {
            return Err(FormatError::malformed(
                None,
                "an image without defined functions has function bodies",
            ));
        }
        return Ok(());
    }

    let index = index.ok_or_else(|| {
        FormatError::malformed(None, "FUNCINDEX is missing for defined functions")
    })?;
    let code =
        code.ok_or_else(|| FormatError::malformed(None, "CODE is missing for defined functions"))?;
    if index.uncompressed_size != expected_size {
        return Err(FormatError::malformed(
            None,
            "FUNCINDEX size does not match the defined function count",
        ));
    }

    let mut validator =
        FunctionIndexValidator::new(function_count, code.uncompressed_size, canonical)?;
    index.for_each_uncompressed_chunk(|chunk| validator.push(chunk))?;
    validator.finish()
}

fn validate_canonical_layout(
    offsets: &[(usize, usize, usize)],
    table_end: usize,
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
            "canonical image has trailing bytes",
        ));
    }
    Ok(())
}

fn known_section(kind: u32) -> bool {
    (1..=21).contains(&kind)
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
