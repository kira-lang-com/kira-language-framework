//! Source files and byte spans used by kernel diagnostics.

use klf_core::{FileId, Name};
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[must_use]
    pub const fn new(file: FileId, start: u32, end: u32) -> Self {
        Self { file, start, end }
    }

    #[must_use]
    pub const fn empty(file: FileId, offset: u32) -> Self {
        Self::new(file, offset, offset)
    }
}

/// The content hash definitions use to name a registered source they spliced
/// in, such as an included header. It must match the definitions' own copy.
#[must_use]
pub fn source_hash(text: &str) -> u64 {
    const PRIME: u64 = 2_147_483_647;
    let mut hash = 17_u64;
    for byte in text.bytes() {
        // The remainder by a Mersenne prime, without dividing: a value below
        // 2^62 folds to below twice the prime.
        let value = hash * 131 + u64::from(byte) + 1;
        hash = (value & PRIME) + (value >> 31);
        if hash >= PRIME {
            hash -= PRIME;
        }
    }
    if hash == 0 { 1 } else { hash }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineColumn {
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug)]
pub struct SourceFile {
    id: FileId,
    name: Name,
    text: Arc<str>,
    hash: u64,
    line_starts: Vec<u32>,
}

impl SourceFile {
    /// Creates a source file and indexes its line starts.
    ///
    /// # Panics
    ///
    /// Panics when the source text is 4 GiB or larger, because KLF source
    /// offsets are 32-bit values.
    #[must_use]
    pub fn new(id: FileId, name: impl Into<Name>, text: impl Into<Arc<str>>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(u32::try_from(index + 1).expect("source file exceeds 4 GiB"));
            }
        }
        Self {
            id,
            name: name.into(),
            hash: source_hash(&text),
            text,
            line_starts,
        }
    }

    #[must_use]
    pub const fn id(&self) -> FileId {
        self.id
    }

    #[must_use]
    pub fn name(&self) -> &Name {
        &self.name
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text, shared without copying.
    #[must_use]
    pub fn shared_text(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }

    /// [`source_hash`] of the text, computed once.
    #[must_use]
    pub const fn hash(&self) -> u64 {
        self.hash
    }

    /// Resolves an in-range byte offset to a one-based source position.
    ///
    /// # Panics
    ///
    /// Panics if the source contains more lines than a 32-bit position can
    /// represent.
    #[must_use]
    pub fn line_column(&self, offset: u32) -> LineColumn {
        let line = self.line_starts.partition_point(|start| *start <= offset) - 1;
        LineColumn {
            line: u32::try_from(line + 1).expect("line count exceeds u32"),
            column: offset - self.line_starts[line] + 1,
        }
    }

    /// Returns the byte range occupied by a one-based line number.
    ///
    /// # Panics
    ///
    /// Panics only on a platform whose pointer width cannot represent a
    /// 32-bit source offset.
    #[must_use]
    pub fn line_range(&self, line: u32) -> Option<Range<usize>> {
        let index = usize::try_from(line.checked_sub(1)?).ok()?;
        let start = usize::try_from(*self.line_starts.get(index)?).ok()?;
        let end = self
            .line_starts
            .get(index + 1)
            .map_or(self.text.len(), |value| {
                usize::try_from(*value).expect("offset fits usize")
            });
        Some(start..end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_offsets_to_one_based_positions() {
        let file = SourceFile::new(FileId::from_u32(0), "test.kir", Arc::<str>::from("a\nbc\n"));
        assert_eq!(file.line_column(0), LineColumn { line: 1, column: 1 });
        assert_eq!(file.line_column(3), LineColumn { line: 2, column: 2 });
    }
}
