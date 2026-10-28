//! Whitespace and comments between tokens, read with the comment syntax a
//! definition declares in its `EditorSpec`.

use klf_facts::spec::EditorSpec;

/// The comment forms of a language.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Syntax {
    pub line_comments: Vec<String>,
    /// `(open, close, nests)`.
    pub block_comments: Vec<(String, String, bool)>,
}

impl Syntax {
    #[must_use]
    pub fn from_spec(spec: &EditorSpec) -> Self {
        Self {
            line_comments: spec.line_comments.clone(),
            block_comments: spec
                .block_comments
                .iter()
                .map(|comment| (comment.open.clone(), comment.close.clone(), comment.nests))
                .collect(),
        }
    }

    /// The end of the comment starting at `at`, and whether it runs to the
    /// end of its line.
    fn comment(&self, source: &str, at: usize) -> Option<(usize, bool)> {
        let rest = &source[at..];
        if let Some((open, close, nests)) = self
            .block_comments
            .iter()
            .filter(|(open, _, _)| rest.starts_with(open.as_str()))
            .max_by_key(|(open, _, _)| open.len())
        {
            return Some((
                block_end(source, at + open.len(), open, close, *nests),
                false,
            ));
        }
        self.line_comments
            .iter()
            .any(|open| !open.is_empty() && rest.starts_with(open.as_str()))
            .then(|| (rest.find('\n').map_or(source.len(), |end| at + end), true))
    }
}

fn block_end(source: &str, mut at: usize, open: &str, close: &str, nests: bool) -> usize {
    let mut depth = 1usize;
    while at < source.len() {
        let rest = &source[at..];
        if rest.starts_with(close) {
            depth -= 1;
            at += close.len();
            if depth == 0 {
                return at;
            }
        } else if nests && rest.starts_with(open) {
            depth += 1;
            at += open.len();
        } else {
            at += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    source.len()
}

/// One comment between two tokens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Comment {
    pub text: String,
    /// Line breaks between the previous token or comment and this one.
    pub newlines_before: usize,
    pub line: bool,
    pub end: usize,
}

/// The trivia before a token.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Trivia {
    pub comments: Vec<Comment>,
    /// Line breaks between the last comment, or the previous token, and the
    /// token.
    pub newlines_after: usize,
    pub end: usize,
}

impl Trivia {
    pub fn is_empty(&self, start: usize) -> bool {
        self.comments.is_empty() && self.end == start
    }
}

pub(crate) fn scan(syntax: &Syntax, source: &str, start: usize) -> Trivia {
    let mut trivia = Trivia::default();
    let mut newlines = 0;
    let mut at = start;
    while at < source.len() {
        let rest = &source[at..];
        let character = rest.chars().next().unwrap_or(' ');
        if character == '\n' {
            newlines += 1;
            at += 1;
        } else if character.is_whitespace() {
            at += character.len_utf8();
        } else if let Some((end, line)) = syntax.comment(source, at) {
            trivia.comments.push(Comment {
                text: source[at..end].trim_end().to_owned(),
                newlines_before: newlines,
                line,
                end,
            });
            newlines = 0;
            at = end;
        } else {
            break;
        }
    }
    trivia.newlines_after = newlines;
    trivia.end = at;
    trivia
}
