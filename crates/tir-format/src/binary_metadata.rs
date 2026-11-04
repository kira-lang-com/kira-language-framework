use std::collections::HashSet;
use std::fmt;

use crate::leb::{read_uleb, write_uleb};
use crate::{BinaryModule, FormatError, StringTable};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinaryMetadataTable {
    pub domains: Vec<BinaryAliasDomain>,
    pub scopes: Vec<BinaryAliasScope>,
    pub attachments: Vec<BinaryMetadataAttachment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryAliasDomain {
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryAliasScope {
    pub domain_index: u64,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryMetadataAttachment {
    pub function_index: u64,
    pub instruction_ordinal: u64,
    pub metadata: BinaryMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BinaryMetadata {
    BranchWeights(Vec<u64>),
    LoopHints {
        unroll_count: u64,
        vectorize_width: u64,
        interleave_count: u64,
        must_progress: bool,
    },
    Range {
        type_index: u64,
        intervals: Vec<(u64, u64)>,
    },
    NonNull,
    Align(u64),
    NoAlias(Vec<u64>),
    AliasScope(Vec<u64>),
    Tbaa(String),
    ProfileId(u64),
    InvariantLoad,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataDecodeError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for MetadataDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for MetadataDecodeError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataEncodeError {
    pub message: String,
}

impl fmt::Display for MetadataEncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for MetadataEncodeError {}

impl BinaryMetadataTable {
    pub fn encode(&self, strings: &StringTable) -> Result<Vec<u8>, MetadataEncodeError> {
        if self.domains.is_empty() && self.scopes.is_empty() && self.attachments.is_empty() {
            return Ok(Vec::new());
        }
        check_count(self.domains.len(), "alias-domain")?;
        check_count(self.scopes.len(), "alias-scope")?;

        let mut output = Vec::new();
        write_count(self.domains.len(), "alias-domain", &mut output)?;
        let mut previous_domain: Option<&str> = None;
        for domain in &self.domains {
            if domain.name.is_empty()
                || previous_domain
                    .is_some_and(|previous| previous.as_bytes() >= domain.name.as_bytes())
            {
                return Err(encode_error("invalid or unordered alias-domain name"));
            }
            write_string_index(&domain.name, strings, "alias-domain name", &mut output)?;
            previous_domain = Some(&domain.name);
        }

        write_count(self.scopes.len(), "alias-scope", &mut output)?;
        let mut previous_scope: Option<(u64, String)> = None;
        for scope in &self.scopes {
            if usize::try_from(scope.domain_index).map_or(true, |index| index >= self.domains.len())
                || scope.name.is_empty()
                || previous_scope.as_ref().is_some_and(|(domain, name)| {
                    (*domain, name.as_bytes()) >= (scope.domain_index, scope.name.as_bytes())
                })
            {
                return Err(encode_error("invalid or unordered alias-scope name"));
            }
            write_index(scope.domain_index, "alias-scope domain index", &mut output)?;
            write_string_index(&scope.name, strings, "alias-scope name", &mut output)?;
            previous_scope = Some((scope.domain_index, scope.name.clone()));
        }

        let record_count = self.metadata_record_count()?;
        write_count(record_count, "metadata attachment record", &mut output)?;
        let mut used_scopes = HashSet::new();
        let mut start = 0;
        while start < self.attachments.len() {
            let first = &self.attachments[start];
            let key = (first.function_index, first.instruction_ordinal);
            let end = (start + 1..self.attachments.len())
                .find(|index| {
                    let attachment = &self.attachments[*index];
                    (attachment.function_index, attachment.instruction_ordinal) != key
                })
                .unwrap_or(self.attachments.len());
            write_index(key.0, "metadata function index", &mut output)?;
            write_index(key.1, "metadata instruction ordinal", &mut output)?;
            write_count(end - start, "instruction metadata", &mut output)?;
            for attachment in &self.attachments[start..end] {
                encode_metadata(
                    &attachment.metadata,
                    strings,
                    &self.scopes,
                    &mut used_scopes,
                    &mut output,
                )?;
            }
            start = end;
        }

        if used_scopes.len() != self.scopes.len() {
            return Err(encode_error(
                "METADATA contains an unreferenced alias scope",
            ));
        }
        let used_domains: HashSet<_> = used_scopes
            .iter()
            .filter_map(|index| self.scopes.get(*index).map(|scope| scope.domain_index))
            .collect();
        if used_domains.len() != self.domains.len() {
            return Err(encode_error(
                "METADATA contains an unreferenced alias domain",
            ));
        }
        Ok(output)
    }

    fn metadata_record_count(&self) -> Result<usize, MetadataEncodeError> {
        let mut records = 0usize;
        let mut previous_key = None;
        let mut previous_kind = None;
        for attachment in &self.attachments {
            let key = (attachment.function_index, attachment.instruction_ordinal);
            let kind = metadata_kind(&attachment.metadata);
            match previous_key {
                Some(previous) if key < previous => {
                    return Err(encode_error(
                        "metadata records are not ordered by instruction",
                    ));
                }
                Some(previous) if key == previous => {
                    if previous_kind.is_some_and(|previous| kind <= previous) {
                        return Err(encode_error(
                            "metadata kinds are not strictly ordered for an instruction",
                        ));
                    }
                }
                _ => {
                    records = records
                        .checked_add(1)
                        .ok_or_else(|| encode_error("metadata record count overflows memory"))?;
                }
            }
            previous_key = Some(key);
            previous_kind = Some(kind);
        }
        check_count(records, "metadata attachment record")?;
        Ok(records)
    }

    pub fn decode(bytes: &[u8], strings: &StringTable) -> Result<Self, MetadataDecodeError> {
        let mut decoder = Decoder {
            bytes,
            cursor: 0,
            strings,
        };
        let domain_count = decoder.count(1, "alias-domain")?;
        let mut domains = decoder.reserve(domain_count, "alias-domain")?;
        let mut previous_domain: Option<String> = None;
        for _ in 0..domain_count {
            let offset = decoder.cursor;
            let name = decoder.string("alias-domain name")?;
            if name.is_empty()
                || previous_domain
                    .as_ref()
                    .is_some_and(|previous| previous.as_bytes() >= name.as_bytes())
            {
                return Err(decoder.error(offset, "invalid or unordered alias-domain name"));
            }
            previous_domain = Some(name.clone());
            domains.push(BinaryAliasDomain { name });
        }

        let scope_count = decoder.count(2, "alias-scope")?;
        let mut scopes = decoder.reserve(scope_count, "alias-scope")?;
        let mut previous_scope: Option<(u64, String)> = None;
        for _ in 0..scope_count {
            let offset = decoder.cursor;
            let domain_index = decoder.index("alias-scope domain index")?;
            if usize::try_from(domain_index).map_or(true, |index| index >= domains.len()) {
                return Err(decoder.error(offset, "alias-scope domain index is invalid"));
            }
            let name = decoder.string("alias-scope name")?;
            if name.is_empty()
                || previous_scope
                    .as_ref()
                    .is_some_and(|(previous_domain, previous_name)| {
                        (*previous_domain, previous_name.as_bytes())
                            >= (domain_index, name.as_bytes())
                    })
            {
                return Err(decoder.error(offset, "invalid or unordered alias-scope name"));
            }
            previous_scope = Some((domain_index, name.clone()));
            scopes.push(BinaryAliasScope { domain_index, name });
        }

        let record_count = decoder.count(3, "metadata attachment record")?;
        let mut attachments = Vec::new();
        let mut used_scopes = HashSet::new();
        used_scopes
            .try_reserve(scope_count)
            .map_err(|_| decoder.error(decoder.cursor, "metadata scope allocation failed"))?;
        attachments
            .try_reserve_exact(record_count)
            .map_err(|_| decoder.error(decoder.cursor, "metadata attachment allocation failed"))?;
        let mut previous_key = None;
        for _ in 0..record_count {
            let offset = decoder.cursor;
            let function_index = decoder.index("metadata function index")?;
            let instruction_ordinal = decoder.index("metadata instruction ordinal")?;
            let key = (function_index, instruction_ordinal);
            if previous_key.is_some_and(|previous| key <= previous) {
                return Err(
                    decoder.error(offset, "metadata records are not ordered by instruction")
                );
            }
            previous_key = Some(key);
            let metadata_count = decoder.count(1, "instruction metadata")?;
            if metadata_count == 0 {
                return Err(decoder.error(offset, "metadata record has no attachments"));
            }
            attachments.try_reserve(metadata_count).map_err(|_| {
                decoder.error(decoder.cursor, "metadata attachment allocation failed")
            })?;
            let mut previous_kind = None;
            for _ in 0..metadata_count {
                let kind_offset = decoder.cursor;
                let kind = decoder.byte("metadata kind")?;
                if previous_kind.is_some_and(|previous| kind <= previous) {
                    return Err(
                        decoder.error(kind_offset, "metadata kinds are not strictly ordered")
                    );
                }
                previous_kind = Some(kind);
                let metadata = decoder.metadata(kind, &scopes, &mut used_scopes)?;
                attachments.push(BinaryMetadataAttachment {
                    function_index,
                    instruction_ordinal,
                    metadata,
                });
            }
        }
        if decoder.cursor != bytes.len() {
            return Err(decoder.error(decoder.cursor, "trailing bytes in METADATA section"));
        }
        if domains.is_empty() && scopes.is_empty() && attachments.is_empty() {
            return Err(decoder.error(0, "empty METADATA section must be omitted"));
        }
        if used_scopes.len() != scopes.len() {
            return Err(decoder.error(0, "METADATA contains an unreferenced alias scope"));
        }
        let used_domains: HashSet<_> = used_scopes
            .iter()
            .filter_map(|index| scopes.get(*index).map(|scope| scope.domain_index))
            .collect();
        if used_domains.len() != domains.len() {
            return Err(decoder.error(0, "METADATA contains an unreferenced alias domain"));
        }
        Ok(Self {
            domains,
            scopes,
            attachments,
        })
    }
}

impl BinaryModule {
    pub fn metadata_table(&self, memory_limit: u64) -> Result<BinaryMetadataTable, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 15) else {
            return Ok(BinaryMetadataTable::default());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        BinaryMetadataTable::decode(&bytes, &strings)
            .map_err(|error| FormatError::malformed(Some(error.offset), error.message))
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
    strings: &'a StringTable,
}

impl Decoder<'_> {
    fn metadata(
        &mut self,
        kind: u8,
        scopes: &[BinaryAliasScope],
        used_scopes: &mut HashSet<usize>,
    ) -> Result<BinaryMetadata, MetadataDecodeError> {
        match kind {
            0 => {
                let offset = self.cursor;
                let count = self.count(1, "branch weight")?;
                if count < 2 {
                    return Err(self.error(offset, "branch weights require at least two values"));
                }
                let mut weights = self.reserve(count, "branch weight")?;
                for _ in 0..count {
                    weights.push(self.index("branch weight")?);
                }
                Ok(BinaryMetadata::BranchWeights(weights))
            }
            1 => {
                let offset = self.cursor;
                let unroll_count = self.index("loop unroll count")?;
                let vectorize_width = self.index("loop vectorize width")?;
                let interleave_count = self.index("loop interleave count")?;
                let must_progress = match self.byte("loop mustprogress flag")? {
                    0 => false,
                    1 => true,
                    _ => return Err(self.error(offset, "invalid loop mustprogress flag")),
                };
                if unroll_count == 0
                    && vectorize_width == 0
                    && interleave_count == 0
                    && !must_progress
                {
                    return Err(self.error(offset, "empty loop hint record"));
                }
                Ok(BinaryMetadata::LoopHints {
                    unroll_count,
                    vectorize_width,
                    interleave_count,
                    must_progress,
                })
            }
            2 => {
                let type_index = self.index("range type index")?;
                let count_offset = self.cursor;
                let count = self.count(2, "range interval")?;
                if count == 0 {
                    return Err(self.error(count_offset, "range metadata has no intervals"));
                }
                let mut intervals = self.reserve(count, "range interval")?;
                for _ in 0..count {
                    intervals.push((
                        self.index("range lower constant")?,
                        self.index("range upper constant")?,
                    ));
                }
                Ok(BinaryMetadata::Range {
                    type_index,
                    intervals,
                })
            }
            3 => Ok(BinaryMetadata::NonNull),
            4 => {
                let offset = self.cursor;
                let alignment = self.index("metadata alignment")?;
                if alignment == 0 || !alignment.is_power_of_two() {
                    return Err(self.error(offset, "metadata alignment is not a power of two"));
                }
                Ok(BinaryMetadata::Align(alignment))
            }
            5 | 6 => {
                let offset = self.cursor;
                let count = self.count(1, "alias-scope reference")?;
                if count == 0 {
                    return Err(self.error(offset, "alias metadata has no scopes"));
                }
                let mut references = self.reserve(count, "alias-scope reference")?;
                let mut previous = None;
                for _ in 0..count {
                    let scope_offset = self.cursor;
                    let index = self.index("alias-scope index")?;
                    let index = usize::try_from(index)
                        .map_err(|_| self.error(scope_offset, "alias-scope index overflows"))?;
                    if index >= scopes.len() || previous.is_some_and(|old| index <= old) {
                        return Err(
                            self.error(scope_offset, "invalid or unordered alias-scope index")
                        );
                    }
                    previous = Some(index);
                    used_scopes.insert(index);
                    references.push(index as u64);
                }
                if kind == 5 {
                    Ok(BinaryMetadata::NoAlias(references))
                } else {
                    Ok(BinaryMetadata::AliasScope(references))
                }
            }
            7 => {
                let offset = self.cursor;
                let tag = self.string("TBAA tag")?;
                if tag.is_empty() {
                    return Err(self.error(offset, "TBAA tag is empty"));
                }
                Ok(BinaryMetadata::Tbaa(tag))
            }
            8 => {
                let offset = self.cursor;
                let profile_id = self.index("profile id")?;
                if profile_id >= 1 << 32 {
                    return Err(self.error(offset, "profile id exceeds u32"));
                }
                Ok(BinaryMetadata::ProfileId(profile_id))
            }
            9 => Ok(BinaryMetadata::InvariantLoad),
            _ => Err(self.error(self.cursor.saturating_sub(1), "unknown METADATA kind")),
        }
    }

    fn count(
        &mut self,
        minimum_record_size: usize,
        label: &str,
    ) -> Result<usize, MetadataDecodeError> {
        let offset = self.cursor;
        let count = self.index(&format!("{label} count"))?;
        if count >= 1 << 32 {
            return Err(self.error(offset, format!("{label} count exceeds 2^32")));
        }
        let count = usize::try_from(count)
            .map_err(|_| self.error(offset, format!("{label} count does not fit in memory")))?;
        if minimum_record_size > 0
            && count > self.bytes.len().saturating_sub(self.cursor) / minimum_record_size
        {
            return Err(self.error(offset, format!("{label} count exceeds section size")));
        }
        Ok(count)
    }

    fn reserve<T>(&self, count: usize, label: &str) -> Result<Vec<T>, MetadataDecodeError> {
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| self.error(self.cursor, format!("{label} allocation failed")))?;
        Ok(values)
    }

    fn index(&mut self, label: &str) -> Result<u64, MetadataDecodeError> {
        let offset = self.cursor;
        read_uleb(self.bytes, &mut self.cursor)
            .map_err(|error| self.error(offset, format!("invalid {label} ULEB128: {error:?}")))
    }

    fn byte(&mut self, label: &str) -> Result<u8, MetadataDecodeError> {
        let offset = self.cursor;
        let value = self
            .bytes
            .get(self.cursor)
            .copied()
            .ok_or_else(|| self.error(offset, format!("truncated {label}")))?;
        self.cursor += 1;
        Ok(value)
    }

    fn string(&mut self, label: &str) -> Result<String, MetadataDecodeError> {
        let offset = self.cursor;
        let index = self.index(label)?;
        let value = usize::try_from(index)
            .ok()
            .and_then(|index| self.strings.get(index))
            .ok_or_else(|| self.error(offset, format!("{label} index is invalid")))?;
        Ok(value.to_owned())
    }

    fn error(&self, offset: usize, message: impl Into<String>) -> MetadataDecodeError {
        MetadataDecodeError {
            offset,
            message: message.into(),
        }
    }
}

fn encode_metadata(
    metadata: &BinaryMetadata,
    strings: &StringTable,
    scopes: &[BinaryAliasScope],
    used_scopes: &mut HashSet<usize>,
    output: &mut Vec<u8>,
) -> Result<(), MetadataEncodeError> {
    output.push(metadata_kind(metadata));
    match metadata {
        BinaryMetadata::BranchWeights(weights) => {
            if weights.len() < 2 {
                return Err(encode_error("branch weights require at least two values"));
            }
            write_count(weights.len(), "branch weight", output)?;
            for weight in weights {
                write_index(*weight, "branch weight", output)?;
            }
        }
        BinaryMetadata::LoopHints {
            unroll_count,
            vectorize_width,
            interleave_count,
            must_progress,
        } => {
            if *unroll_count == 0
                && *vectorize_width == 0
                && *interleave_count == 0
                && !must_progress
            {
                return Err(encode_error("empty loop hint record"));
            }
            for (value, label) in [
                (*unroll_count, "loop unroll count"),
                (*vectorize_width, "loop vectorize width"),
                (*interleave_count, "loop interleave count"),
            ] {
                write_index(value, label, output)?;
            }
            output.push(u8::from(*must_progress));
        }
        BinaryMetadata::Range {
            type_index,
            intervals,
        } => {
            if intervals.is_empty() {
                return Err(encode_error("range metadata has no intervals"));
            }
            write_index(*type_index, "range type index", output)?;
            write_count(intervals.len(), "range interval", output)?;
            for (lower, upper) in intervals {
                write_index(*lower, "range lower constant", output)?;
                write_index(*upper, "range upper constant", output)?;
            }
        }
        BinaryMetadata::NonNull | BinaryMetadata::InvariantLoad => {}
        BinaryMetadata::Align(alignment) => {
            if *alignment == 0 || !alignment.is_power_of_two() {
                return Err(encode_error(
                    "metadata alignment must be a nonzero power of two",
                ));
            }
            write_index(*alignment, "metadata alignment", output)?;
        }
        BinaryMetadata::NoAlias(references) | BinaryMetadata::AliasScope(references) => {
            if references.is_empty() {
                return Err(encode_error("alias metadata has no scopes"));
            }
            write_count(references.len(), "alias-scope reference", output)?;
            let mut previous = None;
            for reference in references {
                let index = usize::try_from(*reference)
                    .map_err(|_| encode_error("alias-scope index overflows memory"))?;
                if index >= scopes.len() || previous.is_some_and(|old| *reference <= old) {
                    return Err(encode_error("invalid or unordered alias-scope index"));
                }
                previous = Some(*reference);
                used_scopes.insert(index);
                write_index(*reference, "alias-scope index", output)?;
            }
        }
        BinaryMetadata::Tbaa(tag) => {
            if tag.is_empty() {
                return Err(encode_error("TBAA tag is empty"));
            }
            write_string_index(tag, strings, "TBAA tag", output)?;
        }
        BinaryMetadata::ProfileId(profile_id) => {
            if *profile_id >= 1 << 32 {
                return Err(encode_error("profile id exceeds u32"));
            }
            write_index(*profile_id, "profile id", output)?;
        }
    }
    Ok(())
}

fn metadata_kind(metadata: &BinaryMetadata) -> u8 {
    match metadata {
        BinaryMetadata::BranchWeights(_) => 0,
        BinaryMetadata::LoopHints { .. } => 1,
        BinaryMetadata::Range { .. } => 2,
        BinaryMetadata::NonNull => 3,
        BinaryMetadata::Align(_) => 4,
        BinaryMetadata::NoAlias(_) => 5,
        BinaryMetadata::AliasScope(_) => 6,
        BinaryMetadata::Tbaa(_) => 7,
        BinaryMetadata::ProfileId(_) => 8,
        BinaryMetadata::InvariantLoad => 9,
    }
}

fn write_string_index(
    value: &str,
    strings: &StringTable,
    label: &str,
    output: &mut Vec<u8>,
) -> Result<(), MetadataEncodeError> {
    let index = (0..strings.len())
        .find(|index| strings.get(*index) == Some(value))
        .ok_or_else(|| encode_error(format!("{label} is missing from STRINGS")))?;
    let index =
        u64::try_from(index).map_err(|_| encode_error(format!("{label} index overflows")))?;
    write_index(index, label, output)
}

fn write_count(count: usize, label: &str, output: &mut Vec<u8>) -> Result<(), MetadataEncodeError> {
    check_count(count, label)?;
    let count =
        u64::try_from(count).map_err(|_| encode_error(format!("{label} count overflows")))?;
    write_index(count, label, output)
}

fn check_count(count: usize, label: &str) -> Result<(), MetadataEncodeError> {
    if u64::try_from(count).map_or(true, |count| count >= 1 << 32) {
        return Err(encode_error(format!("{label} count exceeds 2^32")));
    }
    Ok(())
}

fn write_index(value: u64, label: &str, output: &mut Vec<u8>) -> Result<(), MetadataEncodeError> {
    write_uleb(value, output).map_err(|_| encode_error(format!("failed to write {label}")))
}

fn encode_error(message: impl Into<String>) -> MetadataEncodeError {
    MetadataEncodeError {
        message: message.into(),
    }
}
