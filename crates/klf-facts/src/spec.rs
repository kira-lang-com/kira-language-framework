//! A definition's editor spec (`KLF-EDITOR.md` section 5.2): the lexical facts
//! editors need before, and without, a running server.

use crate::pattern::{Pattern, PatternError};
use crate::{TokenClass, TokenModifier};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockComment {
    pub open: String,
    pub close: String,
    pub nests: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BracketPair {
    pub open: String,
    pub close: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StringForm {
    pub open: String,
    pub close: String,
    /// One escape sequence, as a pattern.
    pub escape: Option<String>,
    pub multiline: bool,
    pub raw: bool,
    pub interpolation: Option<BracketPair>,
    pub class: TokenClass,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeywordGroup {
    pub class: TokenClass,
    pub modifiers: Vec<TokenModifier>,
    pub words: Vec<String>,
}

impl KeywordGroup {
    #[must_use]
    pub fn modifier_bits(&self) -> u32 {
        self.modifiers
            .iter()
            .fold(0, |bits, modifier| bits | modifier.bit())
    }

    #[must_use]
    pub fn contextual(&self) -> bool {
        self.modifiers.contains(&TokenModifier::Contextual)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutoCloseContext {
    String,
    Comment,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutoClosePair {
    pub open: String,
    pub close: String,
    pub not_in: Vec<AutoCloseContext>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndentRules {
    pub unit: String,
    pub increase: Option<String>,
    pub decrease: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnterAction {
    None,
    Indent,
    IndentOutdent,
    Outdent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OnEnterRule {
    pub before: String,
    pub after: Option<String>,
    pub action: EnterAction,
    pub append: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrammarRef {
    pub repository: String,
    pub revision: String,
    pub path: String,
}

/// The editor spec of one language.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorSpec {
    pub display_name: String,
    pub scope: String,
    pub file_names: Vec<String>,
    /// File names whose directory is a project root (section 9.2).
    pub root_markers: Vec<String>,
    pub first_line: Option<String>,
    pub line_comments: Vec<String>,
    pub block_comments: Vec<BlockComment>,
    pub doc_comments: Vec<String>,
    pub strings: Vec<StringForm>,
    pub numbers: Vec<String>,
    pub identifier: String,
    pub keywords: Vec<KeywordGroup>,
    pub operators: Vec<String>,
    pub brackets: Vec<BracketPair>,
    pub auto_close: Vec<AutoClosePair>,
    pub indent: IndentRules,
    pub on_enter: Vec<OnEnterRule>,
    pub region_markers: Option<(String, String)>,
    pub annotation: Option<String>,
    pub preprocessor: Option<String>,
    pub grammar: Option<GrammarRef>,
    pub icon: String,
}

impl EditorSpec {
    /// The spec a definition gets when it declares none (section 5.2 rule 5).
    #[must_use]
    pub fn minimal(name: &str) -> Self {
        let pair = |open: &str, close: &str| BracketPair {
            open: open.to_owned(),
            close: close.to_owned(),
        };
        let auto = |open: &str, close: &str, not_in: Vec<AutoCloseContext>| AutoClosePair {
            open: open.to_owned(),
            close: close.to_owned(),
            not_in,
        };
        let mut display = name.to_owned();
        if let Some(first) = display.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        Self {
            display_name: display,
            scope: format!("source.{name}"),
            file_names: Vec::new(),
            root_markers: Vec::new(),
            first_line: None,
            line_comments: vec!["//".to_owned()],
            block_comments: vec![BlockComment {
                open: "/*".to_owned(),
                close: "*/".to_owned(),
                nests: false,
            }],
            doc_comments: Vec::new(),
            strings: vec![StringForm {
                open: "\"".to_owned(),
                close: "\"".to_owned(),
                escape: Some("\\\\.".to_owned()),
                multiline: false,
                raw: false,
                interpolation: None,
                class: TokenClass::String,
            }],
            numbers: vec!["\\d+(?:\\.\\d+)?".to_owned()],
            identifier: "[A-Za-z_][A-Za-z0-9_]*".to_owned(),
            keywords: Vec::new(),
            operators: Vec::new(),
            brackets: vec![pair("{", "}"), pair("(", ")"), pair("[", "]")],
            auto_close: vec![
                auto("{", "}", Vec::new()),
                auto("(", ")", Vec::new()),
                auto("[", "]", Vec::new()),
                auto(
                    "\"",
                    "\"",
                    vec![AutoCloseContext::String, AutoCloseContext::Comment],
                ),
            ],
            indent: IndentRules {
                unit: "    ".to_owned(),
                increase: Some("[{(\\[]\\s*$".to_owned()),
                decrease: Some("^\\s*[})\\]]".to_owned()),
            },
            on_enter: Vec::new(),
            region_markers: None,
            annotation: None,
            preprocessor: None,
            grammar: None,
            icon: String::new(),
        }
    }

    /// Every pattern of the spec with the member it belongs to.
    #[must_use]
    pub fn patterns(&self) -> Vec<(String, &str)> {
        let mut patterns: Vec<(String, &str)> = Vec::new();
        patterns.push(("identifier".to_owned(), &self.identifier));
        for (index, number) in self.numbers.iter().enumerate() {
            patterns.push((format!("numbers[{index}]"), number));
        }
        for (index, form) in self.strings.iter().enumerate() {
            if let Some(escape) = &form.escape {
                patterns.push((format!("strings[{index}].escape"), escape));
            }
        }
        let optional = [
            ("firstLine", &self.first_line),
            ("annotation", &self.annotation),
            ("preprocessor", &self.preprocessor),
            ("indent.increase", &self.indent.increase),
            ("indent.decrease", &self.indent.decrease),
        ];
        for (member, value) in optional {
            if let Some(value) = value {
                patterns.push((member.to_owned(), value));
            }
        }
        if let Some((start, end)) = &self.region_markers {
            patterns.push(("regionMarkers.start".to_owned(), start));
            patterns.push(("regionMarkers.end".to_owned(), end));
        }
        for (index, rule) in self.on_enter.iter().enumerate() {
            patterns.push((format!("onEnter[{index}].before"), &rule.before));
            if let Some(after) = &rule.after {
                patterns.push((format!("onEnter[{index}].after"), after));
            }
        }
        patterns
    }

    /// Checks the spec: every pattern in the dialect (`KLF-DEF-0040`) and no
    /// inconsistency (`KLF-DEF-0041`). Returns `(code, message)` per problem.
    #[must_use]
    pub fn validate(&self) -> Vec<(&'static str, String)> {
        let mut problems = Vec::new();
        for (member, source) in self.patterns() {
            if let Err(PatternError { offset, message }) = Pattern::parse(source) {
                problems.push((
                    "KLF-DEF-0040",
                    format!("editor.{member} `{source}`: at byte {offset}: {message}"),
                ));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for group in &self.keywords {
            for word in &group.words {
                if !seen.insert(word.as_str()) {
                    problems.push(("KLF-DEF-0041", format!("keyword `{word}` is listed twice")));
                }
            }
        }
        for pair in &self.brackets {
            if pair.open.is_empty() || pair.close.is_empty() || pair.open == pair.close {
                problems.push((
                    "KLF-DEF-0041",
                    format!(
                        "bracket pair `{}` `{}` needs two different sides",
                        pair.open, pair.close
                    ),
                ));
            }
        }
        for pair in &self.auto_close {
            let known = self
                .brackets
                .iter()
                .any(|bracket| bracket.open == pair.open && bracket.close == pair.close)
                || self
                    .strings
                    .iter()
                    .any(|form| form.open == pair.open && form.close == pair.close)
                || self
                    .block_comments
                    .iter()
                    .any(|comment| comment.open == pair.open && comment.close == pair.close);
            if !known {
                problems.push((
                    "KLF-DEF-0041",
                    format!(
                        "auto-close pair `{}` `{}` matches no bracket pair, string form or block comment",
                        pair.open, pair.close
                    ),
                ));
            }
        }
        if self.identifier.is_empty() {
            problems.push(("KLF-DEF-0041", "identifier pattern is empty".to_owned()));
        }
        problems
    }

    /// The keyword group a word belongs to.
    #[must_use]
    pub fn keyword(&self, word: &str) -> Option<&KeywordGroup> {
        self.keywords
            .iter()
            .find(|group| group.words.iter().any(|known| known == word))
    }

    /// Operators flagged as member access: the completion triggers (section
    /// 11.6 rule 1).
    #[must_use]
    pub fn access_operators(&self) -> Vec<String> {
        let mut triggers: Vec<String> = self
            .keywords
            .iter()
            .filter(|group| {
                group.class == TokenClass::Operator
                    && group.modifiers.contains(&TokenModifier::Access)
            })
            .flat_map(|group| group.words.iter().cloned())
            .collect();
        if triggers.is_empty() {
            triggers.push(".".to_owned());
        }
        triggers
    }
}
