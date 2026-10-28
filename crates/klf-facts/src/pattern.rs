//! The editor spec's pattern dialect (`KLF-EDITOR.md` Appendix C): a parser,
//! a translator to each editor's regular expression engine, and a matcher the
//! kernel uses for lexical classification.
//!
//! The dialect is the subset on which Oniguruma, JavaScript, Rust's `regex`
//! and Emacs agree once class escapes are spelled out, so a pattern means the
//! same thing in every editor a package is generated for.

use std::fmt::Write as _;

/// A parsed pattern.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pattern {
    source: String,
    root: Node,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Node {
    Empty,
    Literal(char),
    Any,
    Class {
        negated: bool,
        items: Vec<ClassItem>,
    },
    Concat(Vec<Node>),
    Alternation(Vec<Node>),
    Group(Box<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
    },
    LineStart,
    LineEnd,
    WordBoundary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClassItem {
    Char(char),
    Range(char, char),
    Digit,
    Word,
    Space,
}

/// The engines patterns are translated for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Engine {
    /// `TextMate` grammars.
    Oniguruma,
    /// VS Code language configuration and tree-sitter `grammar.js`.
    JavaScript,
    /// Zed configuration and the kernel.
    Rust,
    /// Emacs Lisp string regexps, before Lisp string escaping.
    Emacs,
}

/// Why a pattern is outside the dialect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatternError {
    pub offset: usize,
    pub message: String,
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for PatternError {}

const META: &[char] = &[
    '\\', '.', '[', ']', '(', ')', '{', '}', '*', '+', '?', '|', '^', '$',
];

impl Pattern {
    /// Parses a pattern of the dialect.
    ///
    /// # Errors
    ///
    /// Returns the first construct outside the dialect.
    pub fn parse(source: &str) -> Result<Self, PatternError> {
        let mut parser = Parser {
            chars: source.char_indices().collect(),
            position: 0,
            length: source.len(),
        };
        let root = parser.alternation()?;
        if let Some(&(offset, character)) = parser.chars.get(parser.position) {
            return Err(PatternError {
                offset,
                message: format!("unexpected `{character}`"),
            });
        }
        Ok(Self {
            source: source.to_owned(),
            root,
        })
    }

    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The pattern in an engine's syntax, with the same meaning.
    #[must_use]
    pub fn to_engine(&self, engine: Engine) -> String {
        let mut out = String::new();
        print(&self.root, engine, &mut out);
        out
    }

    /// The end of the leftmost-first match starting exactly at `start`.
    #[must_use]
    pub fn match_at(&self, text: &str, start: usize) -> Option<usize> {
        matches(&self.root, text, start, &mut |end| Some(end))
    }

    /// Whether the pattern matches the whole of `text`.
    #[must_use]
    pub fn matches_whole(&self, text: &str) -> bool {
        matches(&self.root, text, 0, &mut |end| {
            (end == text.len()).then_some(end)
        })
        .is_some()
    }

    /// The first match in `text` at or after `from`, as a byte range.
    #[must_use]
    pub fn find(&self, text: &str, from: usize) -> Option<(usize, usize)> {
        let mut start = from;
        loop {
            if let Some(end) = self.match_at(text, start) {
                return Some((start, end));
            }
            let next = text[start..].chars().next()?;
            start += next.len_utf8();
        }
    }
}

struct Parser {
    chars: Vec<(usize, char)>,
    position: usize,
    length: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars
            .get(self.position)
            .map(|(_, character)| *character)
    }

    fn offset(&self) -> usize {
        self.chars
            .get(self.position)
            .map_or(self.length, |(offset, _)| *offset)
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, PatternError> {
        Err(PatternError {
            offset: self.offset(),
            message: message.into(),
        })
    }

    fn alternation(&mut self) -> Result<Node, PatternError> {
        let mut branches = vec![self.concatenation()?];
        while self.peek() == Some('|') {
            self.position += 1;
            branches.push(self.concatenation()?);
        }
        Ok(if branches.len() == 1 {
            branches.pop().unwrap_or(Node::Empty)
        } else {
            Node::Alternation(branches)
        })
    }

    fn concatenation(&mut self) -> Result<Node, PatternError> {
        let mut items = Vec::new();
        while let Some(character) = self.peek() {
            if character == '|' || character == ')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.quantified(atom)?);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap_or(Node::Empty),
            _ => Node::Concat(items),
        })
    }

    fn quantified(&mut self, atom: Node) -> Result<Node, PatternError> {
        let (min, max) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => return self.counted(atom),
            _ => return Ok(atom),
        };
        if matches!(atom, Node::LineStart | Node::LineEnd | Node::WordBoundary) {
            return self.error("an anchor cannot repeat");
        }
        self.position += 1;
        if matches!(self.peek(), Some('?' | '+')) {
            return self.error("lazy and possessive quantifiers are not in the dialect");
        }
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
        })
    }

    fn counted(&mut self, atom: Node) -> Result<Node, PatternError> {
        self.position += 1;
        let min = self.number()?;
        let max = if self.peek() == Some(',') {
            self.position += 1;
            if self.peek() == Some('}') {
                None
            } else {
                Some(self.number()?)
            }
        } else {
            Some(min)
        };
        if self.peek() != Some('}') {
            return self.error("expected `}`");
        }
        self.position += 1;
        if max.is_some_and(|max| max < min) {
            return self.error("repetition maximum is below its minimum");
        }
        if matches!(self.peek(), Some('?' | '+')) {
            return self.error("lazy and possessive quantifiers are not in the dialect");
        }
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
        })
    }

    fn number(&mut self) -> Result<u32, PatternError> {
        let mut value: Option<u32> = None;
        while let Some(digit) = self.peek().and_then(|character| character.to_digit(10)) {
            value = Some(value.unwrap_or(0).saturating_mul(10).saturating_add(digit));
            self.position += 1;
        }
        value.map_or_else(|| self.error("expected a number"), Ok)
    }

    fn atom(&mut self) -> Result<Node, PatternError> {
        let Some(character) = self.peek() else {
            return self.error("unexpected end");
        };
        self.position += 1;
        match character {
            '.' => Ok(Node::Any),
            '^' => Ok(Node::LineStart),
            '$' => Ok(Node::LineEnd),
            '[' => self.class(),
            '(' => {
                if self.peek() != Some('?') {
                    return self.error("capturing groups are not in the dialect; write `(?:...)`");
                }
                self.position += 1;
                if self.peek() != Some(':') {
                    return self.error("lookaround and flags are not in the dialect");
                }
                self.position += 1;
                let inner = self.alternation()?;
                if self.peek() != Some(')') {
                    return self.error("expected `)`");
                }
                self.position += 1;
                Ok(Node::Group(Box::new(inner)))
            }
            '*' | '+' | '?' | '{' => self.error(format!("`{character}` has nothing to repeat")),
            ')' | ']' | '}' => self.error(format!("unbalanced `{character}`")),
            '\\' => self.escape(false).map(|item| match item {
                Escape::Item(ClassItem::Char(character)) => Node::Literal(character),
                Escape::Item(item) => Node::Class {
                    negated: false,
                    items: vec![item],
                },
                Escape::Boundary => Node::WordBoundary,
            }),
            other => Ok(Node::Literal(other)),
        }
    }

    fn escape(&mut self, in_class: bool) -> Result<Escape, PatternError> {
        let Some(character) = self.peek() else {
            return self.error("a pattern cannot end with `\\`");
        };
        self.position += 1;
        Ok(match character {
            'd' => Escape::Item(ClassItem::Digit),
            'w' => Escape::Item(ClassItem::Word),
            's' => Escape::Item(ClassItem::Space),
            'n' => Escape::Item(ClassItem::Char('\n')),
            't' => Escape::Item(ClassItem::Char('\t')),
            'b' if !in_class => Escape::Boundary,
            other if META.contains(&other) || other == '-' || other == '/' => {
                Escape::Item(ClassItem::Char(other))
            }
            other => return self.error(format!("`\\{other}` is not in the dialect")),
        })
    }

    fn class(&mut self) -> Result<Node, PatternError> {
        let negated = self.peek() == Some('^');
        if negated {
            self.position += 1;
        }
        let mut items = Vec::new();
        loop {
            let Some(character) = self.peek() else {
                return self.error("unclosed `[`");
            };
            if character == ']' && !items.is_empty() {
                self.position += 1;
                break;
            }
            self.position += 1;
            let first = if character == '\\' {
                match self.escape(true)? {
                    Escape::Item(item) => item,
                    Escape::Boundary => return self.error("`\\b` is not allowed in a class"),
                }
            } else {
                ClassItem::Char(character)
            };
            if let ClassItem::Char(low) = first
                && self.peek() == Some('-')
                && self
                    .chars
                    .get(self.position + 1)
                    .is_some_and(|(_, next)| *next != ']')
            {
                self.position += 1;
                let Some(high) = self.peek() else {
                    return self.error("unclosed `[`");
                };
                self.position += 1;
                let high = if high == '\\' {
                    match self.escape(true)? {
                        Escape::Item(ClassItem::Char(high)) => high,
                        _ => return self.error("a range must end at a character"),
                    }
                } else {
                    high
                };
                if high < low {
                    return self.error("a range must not run backwards");
                }
                items.push(ClassItem::Range(low, high));
            } else {
                items.push(first);
            }
        }
        Ok(Node::Class { negated, items })
    }
}

enum Escape {
    Item(ClassItem),
    Boundary,
}

fn print(node: &Node, engine: Engine, out: &mut String) {
    let emacs = engine == Engine::Emacs;
    match node {
        Node::Empty => {}
        Node::Literal(character) => print_literal(*character, engine, out),
        Node::Any => out.push('.'),
        Node::Class { negated, items } => print_class(*negated, items, engine, out),
        Node::Concat(items) => {
            for item in items {
                print(item, engine, out);
            }
        }
        Node::Alternation(branches) => {
            for (index, branch) in branches.iter().enumerate() {
                if index > 0 {
                    out.push_str(if emacs { "\\|" } else { "|" });
                }
                print(branch, engine, out);
            }
        }
        Node::Group(inner) => {
            out.push_str(if emacs { "\\(?:" } else { "(?:" });
            print(inner, engine, out);
            out.push_str(if emacs { "\\)" } else { ")" });
        }
        Node::Repeat { node, min, max } => {
            let simple = matches!(
                **node,
                Node::Literal(_) | Node::Any | Node::Class { .. } | Node::Group(_)
            );
            if !simple {
                out.push_str(if emacs { "\\(?:" } else { "(?:" });
            }
            print(node, engine, out);
            if !simple {
                out.push_str(if emacs { "\\)" } else { ")" });
            }
            match (min, max) {
                (0, None) => out.push('*'),
                (1, None) => out.push('+'),
                (0, Some(1)) => out.push('?'),
                (min, max) => {
                    out.push_str(if emacs { "\\{" } else { "{" });
                    let _ = write!(out, "{min}");
                    if *max != Some(*min) {
                        out.push(',');
                        if let Some(max) = max {
                            let _ = write!(out, "{max}");
                        }
                    }
                    out.push_str(if emacs { "\\}" } else { "}" });
                }
            }
        }
        Node::LineStart => out.push('^'),
        Node::LineEnd => out.push('$'),
        Node::WordBoundary => out.push_str(match engine {
            Engine::Rust => "(?-u:\\b)",
            Engine::Oniguruma | Engine::JavaScript | Engine::Emacs => "\\b",
        }),
    }
}

fn print_literal(character: char, engine: Engine, out: &mut String) {
    match character {
        '\n' => out.push_str(if engine == Engine::Emacs { "\n" } else { "\\n" }),
        '\t' => out.push_str(if engine == Engine::Emacs { "\t" } else { "\\t" }),
        _ if engine == Engine::Emacs => {
            if matches!(character, '.' | '*' | '+' | '?' | '[' | '^' | '$' | '\\') {
                out.push('\\');
            }
            out.push(character);
        }
        _ => {
            if META.contains(&character) || (engine == Engine::JavaScript && character == '/') {
                out.push('\\');
            }
            out.push(character);
        }
    }
}

fn print_class(negated: bool, items: &[ClassItem], engine: Engine, out: &mut String) {
    if engine == Engine::Emacs {
        print_emacs_class(negated, items, out);
        return;
    }
    out.push('[');
    if negated {
        out.push('^');
    }
    let push = |character: char, out: &mut String| match character {
        '\n' => out.push_str("\\n"),
        '\t' => out.push_str("\\t"),
        '\r' => out.push_str("\\r"),
        '\\' | ']' | '[' | '^' | '-' => {
            out.push('\\');
            out.push(character);
        }
        '/' if engine == Engine::JavaScript => out.push_str("\\/"),
        other => out.push(other),
    };
    for item in items {
        match *item {
            ClassItem::Char(character) => push(character, out),
            ClassItem::Range(low, high) => {
                push(low, out);
                out.push('-');
                push(high, out);
            }
            ClassItem::Digit => out.push_str("0-9"),
            ClassItem::Word => out.push_str("A-Za-z0-9_"),
            ClassItem::Space => out.push_str(" \\t\\r\\n"),
        }
    }
    out.push(']');
}

/// Emacs classes have no escapes: `]` goes first, `-` last, `^` not first.
fn print_emacs_class(negated: bool, items: &[ClassItem], out: &mut String) {
    let mut characters = Vec::new();
    let mut ranges = String::new();
    for item in items {
        match *item {
            ClassItem::Char(character) => characters.push(character),
            ClassItem::Range(low, high) => {
                ranges.push(low);
                ranges.push('-');
                ranges.push(high);
            }
            ClassItem::Digit => ranges.push_str("0-9"),
            ClassItem::Word => ranges.push_str("A-Za-z0-9_"),
            ClassItem::Space => characters.extend([' ', '\t', '\r', '\n']),
        }
    }
    out.push('[');
    if negated {
        out.push('^');
    }
    if characters.contains(&']') {
        out.push(']');
    }
    out.push_str(&ranges);
    for character in &characters {
        if !matches!(character, ']' | '-' | '^') {
            out.push(*character);
        }
    }
    if characters.contains(&'^') {
        out.push('^');
    }
    if characters.contains(&'-') {
        out.push('-');
    }
    out.push(']');
}

fn is_word(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn class_matches(items: &[ClassItem], character: char) -> bool {
    items.iter().any(|item| match *item {
        ClassItem::Char(expected) => character == expected,
        ClassItem::Range(low, high) => (low..=high).contains(&character),
        ClassItem::Digit => character.is_ascii_digit(),
        ClassItem::Word => is_word(character),
        ClassItem::Space => matches!(character, ' ' | '\t' | '\r' | '\n'),
    })
}

/// How a node matches one character.
enum Single {
    /// The node is not a single-character node.
    No,
    Fails,
    Width(usize),
}

impl Single {
    fn of(matched: Option<char>) -> Self {
        matched.map_or(Self::Fails, |character| Self::Width(character.len_utf8()))
    }
}

/// Matches one character for a single-character node.
fn single(node: &Node, text: &str, position: usize) -> Single {
    let next = text[position..].chars().next();
    match node {
        Node::Literal(expected) => Single::of(next.filter(|character| character == expected)),
        Node::Any => Single::of(next.filter(|character| *character != '\n')),
        Node::Class { negated, items } => {
            Single::of(next.filter(|character| class_matches(items, *character) != *negated))
        }
        _ => Single::No,
    }
}

fn matches(
    node: &Node,
    text: &str,
    position: usize,
    rest: &mut dyn FnMut(usize) -> Option<usize>,
) -> Option<usize> {
    match single(node, text, position) {
        Single::Fails => return None,
        Single::Width(width) => return rest(position + width),
        Single::No => {}
    }
    match node {
        Node::Empty => rest(position),
        Node::Concat(items) => concat(items, text, position, rest),
        Node::Alternation(branches) => branches
            .iter()
            .find_map(|branch| matches(branch, text, position, rest)),
        Node::Group(inner) => matches(inner, text, position, rest),
        Node::Repeat { node, min, max } => repeat(node, *min, *max, text, position, rest),
        Node::LineStart => {
            let at_start = position == 0 || text.as_bytes()[position - 1] == b'\n';
            if at_start { rest(position) } else { None }
        }
        Node::LineEnd => {
            let at_end =
                position == text.len() || matches!(text.as_bytes()[position], b'\n' | b'\r');
            if at_end { rest(position) } else { None }
        }
        Node::WordBoundary => {
            let before = text[..position].chars().next_back().is_some_and(is_word);
            let after = text[position..].chars().next().is_some_and(is_word);
            if before == after {
                None
            } else {
                rest(position)
            }
        }
        Node::Literal(_) | Node::Any | Node::Class { .. } => None,
    }
}

fn concat(
    items: &[Node],
    text: &str,
    position: usize,
    rest: &mut dyn FnMut(usize) -> Option<usize>,
) -> Option<usize> {
    let Some((first, tail)) = items.split_first() else {
        return rest(position);
    };
    matches(first, text, position, &mut |next| {
        concat(tail, text, next, rest)
    })
}

fn repeat(
    node: &Node,
    min: u32,
    max: Option<u32>,
    text: &str,
    position: usize,
    rest: &mut dyn FnMut(usize) -> Option<usize>,
) -> Option<usize> {
    // Single-character repetitions are matched iteratively, longest first, so
    // a long run does not recurse once per character.
    if !matches!(single(node, text, position), Single::No) {
        let mut ends = vec![position];
        let mut cursor = position;
        while max.is_none_or(|max| ends.len() <= max as usize) {
            match single(node, text, cursor) {
                Single::Width(width) => {
                    cursor += width;
                    ends.push(cursor);
                }
                Single::No | Single::Fails => break,
            }
        }
        return ends
            .iter()
            .enumerate()
            .rev()
            .filter(|(count, _)| *count >= min as usize)
            .find_map(|(_, end)| rest(*end));
    }
    repeat_general(node, min, max, 0, text, position, rest)
}

fn repeat_general(
    node: &Node,
    min: u32,
    max: Option<u32>,
    count: u32,
    text: &str,
    position: usize,
    rest: &mut dyn FnMut(usize) -> Option<usize>,
) -> Option<usize> {
    if max.is_none_or(|max| count < max) {
        let more = matches(node, text, position, &mut |next| {
            if next == position {
                return None;
            }
            repeat_general(node, min, max, count + 1, text, next, rest)
        });
        if more.is_some() {
            return more;
        }
    }
    if count >= min { rest(position) } else { None }
}
