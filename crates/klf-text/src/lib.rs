//! Line indexes, position encodings and incremental edits (`KLF-EDITOR.md`
//! section 8.2).
//!
//! Kernel spans are byte offsets into UTF-8 text. Editors speak in lines and
//! columns counted in UTF-8, UTF-16 or UTF-32 code units. This crate converts
//! between the two and applies an editor's incremental edits to a document.

/// The code unit a column counts, as negotiated with the client.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Encoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl Encoding {
    /// The name LSP uses for the encoding.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf16 => "utf-16",
            Self::Utf32 => "utf-32",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "utf-8" => Some(Self::Utf8),
            "utf-16" => Some(Self::Utf16),
            "utf-32" => Some(Self::Utf32),
            _ => None,
        }
    }

    /// The encoding a server picks from the client's offered list: UTF-8 when
    /// offered, then UTF-16, then UTF-32. UTF-16 when the list is empty, as
    /// LSP defines.
    #[must_use]
    pub fn negotiate(offered: &[Self]) -> Self {
        [Self::Utf8, Self::Utf16, Self::Utf32]
            .into_iter()
            .find(|encoding| offered.contains(encoding))
            .unwrap_or(Self::Utf16)
    }
}

/// A zero-based line and a column in code units of some [`Encoding`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

impl Position {
    #[must_use]
    pub const fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }
}

/// A half-open range of positions.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    #[must_use]
    pub const fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

/// One multi-byte character of a line: its byte offset in the line and the
/// UTF-16 and UTF-32 columns at which it starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Wide {
    byte: u32,
    utf16: u32,
    utf32: u32,
    len: u8,
}

/// Line starts of a text, and the multi-byte characters of each line that
/// has any, so conversions cost a binary search.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LineIndex {
    /// Byte offset of each line's first byte.
    starts: Vec<u32>,
    /// Byte offset of the end of each line's content, before its line break.
    ends: Vec<u32>,
    /// The multi-byte characters of each line, empty for ASCII lines.
    wide: Vec<Vec<Wide>>,
    len: u32,
}

impl LineIndex {
    /// Indexes `text`. Line breaks are `\n`, `\r\n` and `\r`.
    ///
    /// # Panics
    ///
    /// Panics when the text is 4 GiB or larger: kernel offsets are 32-bit.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let len = u32::try_from(text.len()).expect("text exceeds 4 GiB");
        let bytes = text.as_bytes();
        let mut index = Self {
            starts: vec![0],
            ends: Vec::new(),
            wide: vec![Vec::new()],
            len,
        };
        let mut line_start = 0usize;
        let mut utf16 = 0u32;
        let mut utf32 = 0u32;
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            if byte == b'\n' || byte == b'\r' {
                index.ends.push(offset(cursor));
                let next = if byte == b'\r' && bytes.get(cursor + 1) == Some(&b'\n') {
                    cursor + 2
                } else {
                    cursor + 1
                };
                index.starts.push(offset(next));
                index.wide.push(Vec::new());
                cursor = next;
                line_start = next;
                utf16 = 0;
                utf32 = 0;
                continue;
            }
            let width = utf8_width(byte);
            if width > 1 {
                let line = index.wide.last_mut().expect("a line is open");
                line.push(Wide {
                    byte: offset(cursor - line_start),
                    utf16,
                    utf32,
                    len: width,
                });
                utf16 += if width == 4 { 2 } else { 1 };
            } else {
                utf16 += 1;
            }
            utf32 += 1;
            cursor += usize::from(width);
        }
        index.ends.push(len);
        index
    }

    /// The number of lines. An empty text has one line.
    #[must_use]
    pub fn line_count(&self) -> u32 {
        offset(self.starts.len())
    }

    /// The text's length in bytes.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The byte offset of a line's start, or the text's end past the last line.
    #[must_use]
    pub fn line_start(&self, line: u32) -> u32 {
        self.starts.get(line as usize).copied().unwrap_or(self.len)
    }

    /// The byte offset where a line's content ends, before its line break.
    #[must_use]
    pub fn line_end(&self, line: u32) -> u32 {
        self.ends.get(line as usize).copied().unwrap_or(self.len)
    }

    /// The line holding a byte offset. An offset inside a line break belongs to
    /// the line the break ends.
    #[must_use]
    pub fn line_of(&self, byte: u32) -> u32 {
        let byte = byte.min(self.len);
        offset(self.starts.partition_point(|start| *start <= byte) - 1)
    }

    /// The position of a byte offset. Offsets past the end clamp to the end; an
    /// offset inside a multi-byte character or a line break clamps back to the
    /// character's start or the line's end.
    #[must_use]
    pub fn position(&self, byte: u32, encoding: Encoding) -> Position {
        let line = self.line_of(byte);
        let start = self.starts[line as usize];
        let column_bytes = byte.min(self.line_end(line)).saturating_sub(start);
        Position::new(line, self.column_of(line, column_bytes, encoding))
    }

    /// The byte offset of a position. Columns past the end of a line clamp to
    /// the line's end and lines past the last clamp to the text's end, as LSP
    /// specifies. A column inside a character clamps to the character's start.
    #[must_use]
    pub fn offset(&self, position: Position, encoding: Encoding) -> u32 {
        if position.line as usize >= self.starts.len() {
            return self.len;
        }
        let start = self.starts[position.line as usize];
        let width = self.line_end(position.line) - start;
        let bytes = self.bytes_of(position.line, position.column, encoding);
        start + bytes.min(width)
    }

    /// Converts a byte range.
    #[must_use]
    pub fn range(&self, start: u32, end: u32, encoding: Encoding) -> Range {
        Range::new(self.position(start, encoding), self.position(end, encoding))
    }

    /// The width of a line's content in code units.
    #[must_use]
    pub fn line_width(&self, line: u32, encoding: Encoding) -> u32 {
        if line as usize >= self.starts.len() {
            return 0;
        }
        let bytes = self.line_end(line) - self.starts[line as usize];
        self.column_of(line, bytes, encoding)
    }

    fn column_of(&self, line: u32, bytes: u32, encoding: Encoding) -> u32 {
        let wide = &self.wide[line as usize];
        if encoding == Encoding::Utf8 || wide.is_empty() {
            return bytes;
        }
        let before = wide.partition_point(|character| character.byte < bytes);
        let Some(previous) = before.checked_sub(1).map(|index| wide[index]) else {
            return bytes;
        };
        let (base, units) = match encoding {
            Encoding::Utf16 => (previous.utf16, if previous.len == 4 { 2 } else { 1 }),
            Encoding::Utf32 | Encoding::Utf8 => (previous.utf32, 1),
        };
        let after = previous.byte + u32::from(previous.len);
        if bytes < after {
            return base;
        }
        base + units + (bytes - after)
    }

    fn bytes_of(&self, line: u32, column: u32, encoding: Encoding) -> u32 {
        let wide = &self.wide[line as usize];
        if encoding == Encoding::Utf8 || wide.is_empty() {
            return column;
        }
        let unit = |character: &Wide| match encoding {
            Encoding::Utf16 => character.utf16,
            Encoding::Utf32 | Encoding::Utf8 => character.utf32,
        };
        let before = wide.partition_point(|character| unit(character) < column);
        let Some(previous) = before.checked_sub(1).map(|index| wide[index]) else {
            return column;
        };
        let units = match encoding {
            Encoding::Utf16 if previous.len == 4 => 2,
            _ => 1,
        };
        let end = unit(&previous) + units;
        if column < end {
            return previous.byte;
        }
        previous.byte + u32::from(previous.len) + (column - end)
    }
}

/// A text with its line index, edited the way an editor edits it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Document {
    text: String,
    index: LineIndex,
}

impl Document {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let index = LineIndex::new(&text);
        Self { text, index }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn index(&self) -> &LineIndex {
        &self.index
    }

    /// Replaces `range` with `text`, or the whole document when `range` is
    /// absent. Positions are clamped as [`LineIndex::offset`] clamps them; a
    /// range whose end precedes its start is an error and changes nothing.
    ///
    /// # Errors
    ///
    /// Returns an error when the range's end comes before its start.
    pub fn edit(
        &mut self,
        range: Option<Range>,
        text: &str,
        encoding: Encoding,
    ) -> Result<(), EditError> {
        let Some(range) = range else {
            *self = Self::new(text);
            return Ok(());
        };
        let start = self.index.offset(range.start, encoding) as usize;
        let end = self.index.offset(range.end, encoding) as usize;
        if end < start {
            return Err(EditError::Reversed);
        }
        self.text.replace_range(start..end, text);
        self.index = LineIndex::new(&self.text);
        Ok(())
    }
}

/// Why an edit could not be applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditError {
    Reversed,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reversed => formatter.write_str("edit range ends before it starts"),
        }
    }
}

impl std::error::Error for EditError {}

fn utf8_width(byte: u8) -> u8 {
    match byte {
        0xf0..=0xff => 4,
        0xe0..=0xef => 3,
        0xc0..=0xdf => 2,
        _ => 1,
    }
}

fn offset(value: usize) -> u32 {
    u32::try_from(value).expect("text exceeds 4 GiB")
}

#[cfg(test)]
mod tests;
