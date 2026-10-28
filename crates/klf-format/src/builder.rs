//! Building a layout one token at a time, the way a definition's `format`
//! hook describes it through the `klf.format.*` services.

use crate::trivia::{Comment, Syntax, Trivia, scan};
use crate::{BOM, Doc, FormatError, LineKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Frame {
    Group,
    Indent,
}

/// A layout under construction over one source file.
///
/// Every token is matched against the source at the cursor, so the layout
/// cannot drop, reorder or change a token. The first mismatch is kept and
/// every later call is ignored.
#[derive(Debug)]
pub struct Builder {
    source: String,
    syntax: Syntax,
    cursor: usize,
    started: bool,
    separated: bool,
    root: Vec<Doc>,
    frames: Vec<(Frame, Vec<Doc>)>,
    error: Option<FormatError>,
}

impl Builder {
    #[must_use]
    pub fn new(source: impl Into<String>, syntax: Syntax) -> Self {
        let source = source.into();
        let cursor = if source.starts_with(BOM) {
            BOM.len_utf8()
        } else {
            0
        };
        Self {
            source,
            syntax,
            cursor,
            started: false,
            separated: false,
            root: Vec::new(),
            frames: Vec::new(),
            error: None,
        }
    }

    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn error(&self) -> Option<&FormatError> {
        self.error.as_ref()
    }

    /// Lays out `text`, which must be the next token of the source.
    pub fn text(&mut self, text: &str) -> bool {
        if self.error.is_some() {
            return false;
        }
        let trivia = scan(&self.syntax, &self.source, self.cursor);
        let start = trivia.end;
        if text.is_empty() || !self.source[start..].starts_with(text) {
            return self.fail(FormatError::Mismatch {
                at: start,
                expected: text.to_owned(),
            });
        }
        self.take(&trivia, start, start + text.len());
        true
    }

    /// Lays out the source's `start..end`, which must be the next token.
    pub fn token(&mut self, start: usize, end: usize) -> bool {
        if self.error.is_some() {
            return false;
        }
        let trivia = scan(&self.syntax, &self.source, self.cursor);
        if trivia.end != start
            || start >= end
            || end > self.source.len()
            || !self.source.is_char_boundary(end)
        {
            return self.fail(FormatError::Span { start, end });
        }
        self.take(&trivia, start, end);
        true
    }

    /// Lays out the source's `start..end` as it is, whitespace and comments
    /// included: a region the definition does not format.
    pub fn verbatim(&mut self, start: usize, end: usize) -> bool {
        self.token(start, end)
    }

    pub fn line(&mut self, kind: LineKind) {
        if self.error.is_none() {
            self.trailing();
            self.push(Doc::Line(kind));
        }
    }

    pub fn begin_group(&mut self) {
        self.frames.push((Frame::Group, Vec::new()));
    }

    pub fn begin_indent(&mut self) {
        self.frames.push((Frame::Indent, Vec::new()));
    }

    /// Closes the innermost group or indentation.
    pub fn end(&mut self) -> bool {
        let Some((frame, docs)) = self.frames.pop() else {
            return self.fail(FormatError::Unbalanced);
        };
        let forced = docs.iter().any(Doc::forced);
        self.push(match frame {
            Frame::Group => Doc::Group { docs, forced },
            Frame::Indent => Doc::Indent { docs, forced },
        });
        true
    }

    /// The finished layout, after the comments at the end of the file.
    ///
    /// # Errors
    /// Returns the first mismatch, an unbalanced group, or source text the
    /// layout left out.
    pub fn finish(mut self) -> Result<Doc, FormatError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if !self.frames.is_empty() {
            return Err(FormatError::Unbalanced);
        }
        self.trailing();
        let trivia = scan(&self.syntax, &self.source, self.cursor);
        if trivia.end < self.source.len() {
            return Err(FormatError::Unconsumed { at: trivia.end });
        }
        self.comments(&trivia);
        Ok(Doc::Group {
            docs: self.root,
            forced: true,
        })
    }

    fn fail(&mut self, error: FormatError) -> bool {
        self.error.get_or_insert(error);
        false
    }

    fn push(&mut self, doc: Doc) {
        match self.frames.last_mut() {
            Some((_, docs)) => docs.push(doc),
            None => self.root.push(doc),
        }
    }

    fn take(&mut self, trivia: &Trivia, start: usize, end: usize) {
        let separated = self.separated || !trivia.is_empty(self.cursor);
        self.comments(trivia);
        if self.started && trivia.newlines_after > 1 {
            self.push(Doc::Blank);
        }
        self.push(Doc::Token {
            text: self.source[start..end].to_owned(),
            at: start,
            separated,
        });
        self.cursor = end;
        self.started = true;
        self.separated = false;
    }

    /// Lays out the comments on the line of the last token, before a break.
    fn trailing(&mut self) {
        if !self.started {
            return;
        }
        loop {
            let trivia = scan(&self.syntax, &self.source, self.cursor);
            let Some(comment) = trivia.comments.first().filter(|c| c.newlines_before == 0) else {
                return;
            };
            self.trailing_comment(comment);
            self.cursor = comment.end;
            self.separated = true;
        }
    }

    fn trailing_comment(&mut self, comment: &Comment) {
        let text = format!(" {}", comment.text);
        self.push(if comment.line {
            Doc::Suffix(text)
        } else {
            Doc::Text(text)
        });
    }

    fn comments(&mut self, trivia: &Trivia) {
        for (index, comment) in trivia.comments.iter().enumerate() {
            if self.started && comment.newlines_before == 0 {
                self.trailing_comment(comment);
                continue;
            }
            if self.started && comment.newlines_before > 1 {
                self.push(Doc::Blank);
            }
            self.push(Doc::Leading(comment.text.clone()));
            let next = trivia
                .comments
                .get(index + 1)
                .map_or(trivia.newlines_after, |next| next.newlines_before);
            self.push(if comment.line || next > 0 {
                Doc::Line(LineKind::Hard)
            } else {
                Doc::Text(" ".to_owned())
            });
            self.started = true;
        }
    }
}
