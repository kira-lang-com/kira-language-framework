//! Reading the sources a phase can reach: a file's text by handle, and the
//! name a span of source text spells, by file or by content hash.

use super::DefinitionServiceState;
use klf_core::{FileId, Name};
use klf_source::Span;
use std::sync::Arc;

impl DefinitionServiceState {
    /// The length of one file's text. Reading another unit's source is what
    /// lets a phase run a check over the whole program rather than its own
    /// unit.
    pub(crate) fn source_file_len(&self, arguments: &[u64]) -> u64 {
        self.source_of(arguments).map_or(u64::MAX, |text| {
            u64::try_from(text.len()).unwrap_or(u64::MAX)
        })
    }

    /// One byte of a file's text, zero outside it, matching `klf.source.byte`.
    pub(crate) fn source_file_byte(&self, arguments: &[u64]) -> u64 {
        let Some(text) = self.source_of(arguments) else {
            return u64::MAX;
        };
        let Some(index) = arguments
            .get(2)
            .copied()
            .and_then(|index| usize::try_from(index).ok())
        else {
            return 0;
        };
        text.as_bytes().get(index).copied().map_or(0, u64::from)
    }

    /// Copies a file's whole text to the buffer the third argument names,
    /// which holds the fourth argument's byte count.
    pub(crate) fn source_file_copy(
        &self,
        arguments: &[u64],
        memory: &mut dyn klf_rt::ServiceMemory,
    ) -> u64 {
        let (Some(text), [_, _, address, length]) = (self.source_of(arguments), arguments) else {
            return 0;
        };
        klf_rt::copy_out(memory, text.as_bytes(), *address, *length)
    }

    /// The text of the file named by the second argument, checked against this
    /// phase's session.
    pub(crate) fn source_of(&self, arguments: &[u64]) -> Option<&str> {
        if arguments.first().copied() != Some(self.session.as_u64()) {
            return None;
        }
        let file = FileId::from_u32(u32::try_from(arguments.get(1).copied()?).ok()?);
        self.source_entry(file).map(|(_, _, text)| text.as_ref())
    }

    /// The entry of `sources` for `file`, which is the one at its index
    /// unless a phase pushed sources out of order.
    pub(crate) fn source_entry(&self, file: FileId) -> Option<&(FileId, String, Arc<str>)> {
        match self.sources.get(file.as_u32() as usize) {
            Some(entry) if entry.0 == file => Some(entry),
            _ => self.sources.iter().find(|(known, _, _)| *known == file),
        }
    }

    /// The first entry of `sources` `selector` names.
    pub(crate) fn selected_source(&self, selector: SourceSelector) -> Option<&(FileId, String, Arc<str>)> {
        let hash = match selector {
            SourceSelector::File(file) => return self.source_entry(file),
            SourceSelector::Hash(hash) => hash,
        };
        let mut index = self.sources_by_hash.borrow_mut();
        let (indexed, by_hash) = &mut *index;
        for (position, known) in self.source_hashes.iter().enumerate().skip(*indexed) {
            by_hash.entry(*known).or_insert(position);
        }
        *indexed = self.source_hashes.len();
        self.sources.get(*by_hash.get(&hash)?)
    }

    pub(crate) fn source_text(&self, start: Option<&u64>, end: Option<&u64>) -> Option<String> {
        let raw_start = i64::from_ne_bytes((*start?).to_ne_bytes());
        let raw_end = *end?;
        if raw_start < 0 {
            let (selector, start, end) = selected_span(raw_start, raw_end)?;
            let (_, _, text) = self.selected_source(selector)?;
            return text.get(start..end).map(str::to_owned);
        }
        let start = usize::try_from(raw_start).ok()?;
        let end = usize::try_from(raw_end).ok()?;
        self.source.get(start..end).map(str::to_owned)
    }

    /// A trap's message: the source text the span covers, and for a span
    /// that names its file, where that file holds it, as `path:line:column:`
    /// leads a compiler diagnostic.
    pub(crate) fn trap_message(&self, start: Option<&u64>, end: Option<&u64>) -> Option<String> {
        let text = self.source_text(start, end)?;
        let raw_start = i64::from_ne_bytes((*start?).to_ne_bytes());
        if raw_start >= 0 {
            return Some(text);
        }
        let (selector, offset, _) = selected_span(raw_start, *end?)?;
        let (_, path, source) = self.selected_source(selector)?;
        let before = source.get(..offset)?;
        let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = before.len() - before.rfind('\n').map_or(0, |newline| newline + 1) + 1;
        Some(format!("{path}:{line}:{column}: {text}"))
    }

    pub(crate) fn source_name(
        &self,
        start: Option<&u64>,
        end: Option<&u64>,
    ) -> Option<(Name, Span)> {
        let raw_start = i64::from_ne_bytes((*start?).to_ne_bytes());
        let raw_end = *end?;
        let (selector, start, end) = if raw_start < 0 {
            let (selector, start, end) = selected_span(raw_start, raw_end)?;
            (Some(selector), start, end)
        } else {
            (
                None,
                usize::try_from(raw_start).ok()?,
                usize::try_from(raw_end).ok()?,
            )
        };
        if start >= end {
            return None;
        }
        let valid = |text: &str| {
            !text.is_empty()
                && text
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b':')
        };
        let candidate = |file: FileId, text: &str| {
            let name = text.get(start..end)?.trim();
            if !valid(name) {
                return None;
            }
            let start = u32::try_from(start).ok()?;
            let end = u32::try_from(end).ok()?;
            Some((Name::new(name), Span::new(file, start, end)))
        };
        if selector.is_none()
            && let Some(file) = self.file
            && let Some(name) = candidate(file, &self.source)
        {
            return Some(name);
        }
        if let Some(selector) = selector {
            let (file, _, text) = self.selected_source(selector)?;
            if let Some(name) = candidate(*file, text) {
                return Some(name);
            }
            // Two sources can share a hash, and the name is in the other.
            return self
                .sources
                .iter()
                .zip(&self.source_hashes)
                .filter(|((file, _, _), hash)| selector.names(*file, **hash))
                .find_map(|((file, _, text), _)| candidate(*file, text));
        }
        for (file, _, text) in &self.sources {
            if let Some(name) = candidate(*file, text) {
                return Some(name);
            }
        }
        None
    }
}

/// The registered source a negative span names. A negative start word is
/// `-(hash << 32 | start)`, with the source's content hash as the selector;
/// a frontend that knows the file sets the end word's high half to its
/// `FileId` plus one, which names it exactly. A content hash alone can name
/// two sources once a build holds tens of thousands of them, as one whose
/// macros splice a source per call does.
#[derive(Clone, Copy)]
pub(crate) enum SourceSelector {
    File(FileId),
    Hash(u64),
}

impl SourceSelector {
    fn names(self, file: FileId, hash: u64) -> bool {
        match self {
            Self::File(selected) => selected == file,
            Self::Hash(selected) => selected == hash,
        }
    }
}

/// The selector and byte range of a negative span.
pub(crate) fn selected_span(raw_start: i64, raw_end: u64) -> Option<(SourceSelector, usize, usize)> {
    let encoded = raw_start.unsigned_abs();
    let start = usize::try_from(encoded & u64::from(u32::MAX)).ok()?;
    let end = usize::try_from(raw_end & u64::from(u32::MAX)).ok()?;
    let selector = match u32::try_from(raw_end >> 32).ok()? {
        0 => SourceSelector::Hash(encoded >> 32),
        file => SourceSelector::File(FileId::from_u32(file - 1)),
    };
    Some((selector, start, end))
}
