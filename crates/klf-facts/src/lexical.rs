//! Lexical classification from an editor spec alone (`KLF-EDITOR.md` section
//! 19.3): the first paint before a definition's facts arrive, the reference
//! the generated grammars are tested against, and the classes of files a
//! definition records nothing for.

use crate::pattern::{Pattern, PatternError};
use crate::spec::EditorSpec;
use crate::{TokenClass, TokenModifier};

/// One lexically classified token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LexicalToken {
    pub start: u32,
    pub end: u32,
    pub class: TokenClass,
    pub modifiers: u32,
}

/// An editor spec with its patterns compiled.
pub struct Classifier<'a> {
    spec: &'a EditorSpec,
    identifier: Pattern,
    numbers: Vec<Pattern>,
    escapes: Vec<Option<Pattern>>,
    annotation: Option<Pattern>,
    preprocessor: Option<Pattern>,
    operators: Vec<(String, u32)>,
}

impl<'a> Classifier<'a> {
    /// Compiles the spec's patterns.
    ///
    /// # Errors
    ///
    /// Returns the first pattern outside the dialect.
    pub fn new(spec: &'a EditorSpec) -> Result<Self, PatternError> {
        let mut operators: Vec<(String, u32)> = spec
            .operators
            .iter()
            .map(|operator| (operator.clone(), 0))
            .collect();
        for group in &spec.keywords {
            if group.class == TokenClass::Operator {
                for word in &group.words {
                    operators.retain(|(known, _)| known != word);
                    operators.push((word.clone(), group.modifier_bits()));
                }
            }
        }
        operators.sort_by_key(|(operator, _)| std::cmp::Reverse(operator.len()));
        Ok(Self {
            spec,
            identifier: Pattern::parse(&spec.identifier)?,
            numbers: spec
                .numbers
                .iter()
                .map(|number| Pattern::parse(number))
                .collect::<Result<_, _>>()?,
            escapes: spec
                .strings
                .iter()
                .map(|form| form.escape.as_deref().map(Pattern::parse).transpose())
                .collect::<Result<_, _>>()?,
            annotation: spec.annotation.as_deref().map(Pattern::parse).transpose()?,
            preprocessor: spec
                .preprocessor
                .as_deref()
                .map(Pattern::parse)
                .transpose()?,
            operators,
        })
    }

    /// Classifies `text`.
    #[must_use]
    pub fn classify(&self, text: &str) -> Vec<LexicalToken> {
        let mut tokens = Vec::new();
        let mut position = 0usize;
        let bytes = text.as_bytes();
        while position < text.len() {
            if let Some(end) = self.trivia_or_token(text, position, &mut tokens) {
                position = end.max(position + 1);
                continue;
            }
            let character = text[position..].chars().next().map_or(1, char::len_utf8);
            if bytes[position].is_ascii_whitespace() {
                position += 1;
                continue;
            }
            position += character;
        }
        tokens
    }

    fn trivia_or_token(
        &self,
        text: &str,
        position: usize,
        tokens: &mut Vec<LexicalToken>,
    ) -> Option<usize> {
        self.comment_or_string(text, position, tokens)
            .or_else(|| self.word_or_symbol(text, position, tokens))
    }

    fn comment_or_string(
        &self,
        text: &str,
        position: usize,
        tokens: &mut Vec<LexicalToken>,
    ) -> Option<usize> {
        let rest = &text[position..];
        let line_start = position == 0 || matches!(text.as_bytes()[position - 1], b'\n' | b'\r');
        if line_start
            && let Some(preprocessor) = &self.preprocessor
            && let Some(end) = preprocessor.match_at(text, position)
            && end > position
        {
            push(tokens, position, end, TokenClass::Keyword, 0);
            return Some(end);
        }
        let mut docs: Vec<&String> = self.spec.doc_comments.iter().collect();
        docs.sort_by_key(|prefix| std::cmp::Reverse(prefix.len()));
        for prefix in docs {
            if rest.starts_with(prefix.as_str()) {
                let end = line_end(text, position);
                push(
                    tokens,
                    position,
                    end,
                    TokenClass::DocComment,
                    TokenModifier::Documentation.bit(),
                );
                return Some(end);
            }
        }
        for prefix in &self.spec.line_comments {
            if !prefix.is_empty() && rest.starts_with(prefix.as_str()) {
                let end = line_end(text, position);
                push(tokens, position, end, TokenClass::Comment, 0);
                return Some(end);
            }
        }
        for comment in &self.spec.block_comments {
            if !comment.open.is_empty() && rest.starts_with(comment.open.as_str()) {
                let end = block_end(text, position, &comment.open, &comment.close, comment.nests);
                push(tokens, position, end, TokenClass::Comment, 0);
                return Some(end);
            }
        }
        for (index, form) in self.spec.strings.iter().enumerate() {
            if !form.open.is_empty() && rest.starts_with(form.open.as_str()) {
                return Some(self.string(text, position, index, tokens));
            }
        }
        None
    }

    fn word_or_symbol(
        &self,
        text: &str,
        position: usize,
        tokens: &mut Vec<LexicalToken>,
    ) -> Option<usize> {
        let rest = &text[position..];
        if let Some(annotation) = &self.annotation
            && let Some(end) = annotation.match_at(text, position)
            && end > position
        {
            push(tokens, position, end, TokenClass::Annotation, 0);
            return Some(end);
        }
        let after_word = text[..position]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_');
        if !after_word {
            for number in &self.numbers {
                if let Some(end) = number.match_at(text, position)
                    && end > position
                {
                    push(tokens, position, end, TokenClass::Number, 0);
                    return Some(end);
                }
            }
            if let Some(end) = self.identifier.match_at(text, position)
                && end > position
            {
                if let Some(group) = self.spec.keyword(&text[position..end]) {
                    push(tokens, position, end, group.class, group.modifier_bits());
                }
                return Some(end);
            }
        }
        for (operator, modifiers) in &self.operators {
            if !operator.is_empty() && rest.starts_with(operator.as_str()) {
                push(
                    tokens,
                    position,
                    position + operator.len(),
                    TokenClass::Operator,
                    *modifiers,
                );
                return Some(position + operator.len());
            }
        }
        for pair in &self.spec.brackets {
            for side in [&pair.open, &pair.close] {
                if !side.is_empty() && rest.starts_with(side.as_str()) {
                    push(
                        tokens,
                        position,
                        position + side.len(),
                        TokenClass::Punctuation,
                        0,
                    );
                    return Some(position + side.len());
                }
            }
        }
        None
    }

    fn string(
        &self,
        text: &str,
        start: usize,
        form_index: usize,
        tokens: &mut Vec<LexicalToken>,
    ) -> usize {
        let form = &self.spec.strings[form_index];
        let escape = self.escapes[form_index].as_ref().filter(|_| !form.raw);
        let mut segment = start;
        let mut cursor = start + form.open.len();
        let close = |tokens: &mut Vec<LexicalToken>, from: usize, to: usize| {
            if to > from {
                tokens.push(LexicalToken {
                    start: offset(from),
                    end: offset(to),
                    class: form.class,
                    modifiers: 0,
                });
            }
        };
        while cursor < text.len() {
            let rest = &text[cursor..];
            if rest.starts_with(form.close.as_str()) {
                cursor += form.close.len();
                close(tokens, segment, cursor);
                return cursor;
            }
            if !form.multiline && matches!(text.as_bytes()[cursor], b'\n' | b'\r') {
                close(tokens, segment, cursor);
                return cursor;
            }
            if let Some(escape) = escape
                && let Some(end) = escape.match_at(text, cursor)
                && end > cursor
            {
                close(tokens, segment, cursor);
                tokens.push(LexicalToken {
                    start: offset(cursor),
                    end: offset(end),
                    class: TokenClass::Escape,
                    modifiers: 0,
                });
                cursor = end;
                segment = cursor;
                continue;
            }
            if let Some(pair) = &form.interpolation
                && rest.starts_with(pair.open.as_str())
            {
                close(tokens, segment, cursor);
                let inner = cursor + pair.open.len();
                let end = block_end(text, cursor, &pair.open, &pair.close, true);
                tokens.push(LexicalToken {
                    start: offset(cursor),
                    end: offset(inner),
                    class: TokenClass::Punctuation,
                    modifiers: 0,
                });
                let mut inner_position = inner;
                let inner_end = end.saturating_sub(pair.close.len()).max(inner);
                while inner_position < inner_end {
                    match self.trivia_or_token(&text[..inner_end], inner_position, tokens) {
                        Some(next) => inner_position = next.max(inner_position + 1),
                        None => {
                            inner_position += text[inner_position..]
                                .chars()
                                .next()
                                .map_or(1, char::len_utf8);
                        }
                    }
                }
                if end >= inner_end + pair.close.len() {
                    tokens.push(LexicalToken {
                        start: offset(inner_end),
                        end: offset(end),
                        class: TokenClass::Punctuation,
                        modifiers: 0,
                    });
                }
                cursor = end;
                segment = cursor;
                continue;
            }
            cursor += rest.chars().next().map_or(1, char::len_utf8);
        }
        close(tokens, segment, cursor);
        cursor
    }
}

fn line_end(text: &str, position: usize) -> usize {
    text[position..]
        .find(['\n', '\r'])
        .map_or(text.len(), |found| position + found)
}

/// The end of a bracketed region opened at `position`, past its closer, or
/// the end of the text when it is never closed.
fn block_end(text: &str, position: usize, open: &str, close: &str, nests: bool) -> usize {
    let mut depth = 1usize;
    let mut cursor = position + open.len();
    while cursor < text.len() {
        let rest = &text[cursor..];
        if rest.starts_with(close) {
            depth -= 1;
            cursor += close.len();
            if depth == 0 {
                return cursor;
            }
            continue;
        }
        if nests && rest.starts_with(open) {
            depth += 1;
            cursor += open.len();
            continue;
        }
        cursor += rest.chars().next().map_or(1, char::len_utf8);
    }
    text.len()
}

fn offset(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn push(
    tokens: &mut Vec<LexicalToken>,
    start: usize,
    end: usize,
    class: TokenClass,
    modifiers: u32,
) {
    if end > start {
        tokens.push(LexicalToken {
            start: offset(start),
            end: offset(end),
            class,
            modifiers,
        });
    }
}
