//! A line-based text form of [`EditorSpec`] (in the format of
//! [`crate::codec`]), which a native frontend carries in its metadata and the
//! editor package generator reads back.

use crate::codec::{Records, boolean, flag, optional, present, unescape};
use crate::spec::{
    AutoCloseContext, AutoClosePair, BlockComment, BracketPair, EditorSpec, EnterAction,
    GrammarRef, IndentRules, KeywordGroup, OnEnterRule, StringForm,
};
use crate::{TokenClass, TokenModifier};

impl EditorSpec {
    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = Records::default();
        out.line(&["display_name", &self.display_name]);
        out.line(&["scope", &self.scope]);
        out.line(&["identifier", &self.identifier]);
        out.line(&["icon", &self.icon]);
        for (key, values) in [
            ("file_name", &self.file_names),
            ("root_marker", &self.root_markers),
            ("line_comment", &self.line_comments),
            ("doc_comment", &self.doc_comments),
            ("number", &self.numbers),
            ("operator", &self.operators),
        ] {
            for value in values {
                out.line(&[key, value]);
            }
        }
        for (key, value) in [
            ("first_line", &self.first_line),
            ("annotation", &self.annotation),
            ("preprocessor", &self.preprocessor),
        ] {
            out.line(&[key, &optional(value.as_deref())]);
        }
        for comment in &self.block_comments {
            out.line(&[
                "block_comment",
                &comment.open,
                &comment.close,
                flag(comment.nests),
            ]);
        }
        for form in &self.strings {
            let (open, close) = form
                .interpolation
                .as_ref()
                .map_or((String::new(), ""), |pair| {
                    (format!("+{}", pair.open), pair.close.as_str())
                });
            out.line(&[
                "string",
                &form.open,
                &form.close,
                &optional(form.escape.as_deref()),
                flag(form.multiline),
                flag(form.raw),
                &open,
                close,
                form.class.name(),
            ]);
        }
        for group in &self.keywords {
            let modifiers: Vec<&str> = group
                .modifiers
                .iter()
                .map(|modifier| modifier.name())
                .collect();
            let joined = modifiers.join(",");
            let mut fields = vec!["keywords", group.class.name(), joined.as_str()];
            fields.extend(group.words.iter().map(String::as_str));
            out.line(&fields);
        }
        for pair in &self.brackets {
            out.line(&["bracket", &pair.open, &pair.close]);
        }
        for pair in &self.auto_close {
            let mut fields = vec!["auto_close", pair.open.as_str(), pair.close.as_str()];
            fields.extend(pair.not_in.iter().map(|context| match context {
                AutoCloseContext::String => "String",
                AutoCloseContext::Comment => "Comment",
            }));
            out.line(&fields);
        }
        out.line(&[
            "indent",
            &self.indent.unit,
            &optional(self.indent.increase.as_deref()),
            &optional(self.indent.decrease.as_deref()),
        ]);
        for rule in &self.on_enter {
            out.line(&[
                "on_enter",
                &rule.before,
                &optional(rule.after.as_deref()),
                enter_action_name(rule.action),
                &rule.append,
            ]);
        }
        if let Some((start, end)) = &self.region_markers {
            out.line(&["region", start, end]);
        }
        if let Some(grammar) = &self.grammar {
            out.line(&[
                "grammar",
                &grammar.repository,
                &grammar.revision,
                &grammar.path,
            ]);
        }
        out.text
    }

    /// Reads back what [`Self::encode`] wrote.
    ///
    /// # Errors
    /// Returns the first malformed line.
    pub fn decode(text: &str) -> Result<Self, String> {
        let mut spec = Self::empty();
        for line in text.lines() {
            let fields: Vec<String> = line.split('\t').map(unescape).collect();
            decode_line(&mut spec, &fields)
                .ok_or_else(|| format!("malformed editor spec line `{line}`"))?;
        }
        Ok(spec)
    }

    fn empty() -> Self {
        Self {
            display_name: String::new(),
            scope: String::new(),
            file_names: Vec::new(),
            root_markers: Vec::new(),
            first_line: None,
            line_comments: Vec::new(),
            block_comments: Vec::new(),
            doc_comments: Vec::new(),
            strings: Vec::new(),
            numbers: Vec::new(),
            identifier: String::new(),
            keywords: Vec::new(),
            operators: Vec::new(),
            brackets: Vec::new(),
            auto_close: Vec::new(),
            indent: IndentRules {
                unit: String::new(),
                increase: None,
                decrease: None,
            },
            on_enter: Vec::new(),
            region_markers: None,
            annotation: None,
            preprocessor: None,
            grammar: None,
            icon: String::new(),
        }
    }
}

fn decode_line(spec: &mut EditorSpec, fields: &[String]) -> Option<()> {
    let (key, rest) = fields.split_first()?;
    let one = || rest.first().cloned();
    match key.as_str() {
        "display_name" => spec.display_name = one()?,
        "scope" => spec.scope = one()?,
        "identifier" => spec.identifier = one()?,
        "icon" => spec.icon = one()?,
        "file_name" => spec.file_names.push(one()?),
        "root_marker" => spec.root_markers.push(one()?),
        "line_comment" => spec.line_comments.push(one()?),
        "doc_comment" => spec.doc_comments.push(one()?),
        "number" => spec.numbers.push(one()?),
        "operator" => spec.operators.push(one()?),
        "first_line" => spec.first_line = present(rest.first()?)?,
        "annotation" => spec.annotation = present(rest.first()?)?,
        "preprocessor" => spec.preprocessor = present(rest.first()?)?,
        "block_comment" => {
            let [open, close, nests] = rest else {
                return None;
            };
            spec.block_comments.push(BlockComment {
                open: open.clone(),
                close: close.clone(),
                nests: boolean(nests)?,
            });
        }
        "string" => spec.strings.push(string_form(rest)?),
        "keywords" => spec.keywords.push(keyword_group(rest)?),
        "bracket" => {
            let [open, close] = rest else { return None };
            spec.brackets.push(BracketPair {
                open: open.clone(),
                close: close.clone(),
            });
        }
        "auto_close" => {
            let [open, close, contexts @ ..] = rest else {
                return None;
            };
            let not_in = contexts
                .iter()
                .map(|context| match context.as_str() {
                    "String" => Some(AutoCloseContext::String),
                    "Comment" => Some(AutoCloseContext::Comment),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            spec.auto_close.push(AutoClosePair {
                open: open.clone(),
                close: close.clone(),
                not_in,
            });
        }
        "indent" => {
            let [unit, increase, decrease] = rest else {
                return None;
            };
            spec.indent = IndentRules {
                unit: unit.clone(),
                increase: present(increase)?,
                decrease: present(decrease)?,
            };
        }
        "on_enter" => {
            let [before, after, action, append] = rest else {
                return None;
            };
            spec.on_enter.push(OnEnterRule {
                before: before.clone(),
                after: present(after)?,
                action: enter_action_of(action)?,
                append: append.clone(),
            });
        }
        "region" => {
            let [start, end] = rest else { return None };
            spec.region_markers = Some((start.clone(), end.clone()));
        }
        "grammar" => {
            let [repository, revision, path] = rest else {
                return None;
            };
            spec.grammar = Some(GrammarRef {
                repository: repository.clone(),
                revision: revision.clone(),
                path: path.clone(),
            });
        }
        _ => {}
    }
    Some(())
}

fn string_form(rest: &[String]) -> Option<StringForm> {
    let [
        open,
        close,
        escape,
        multiline,
        raw,
        interpolation,
        interpolation_close,
        class,
    ] = rest
    else {
        return None;
    };
    Some(StringForm {
        open: open.clone(),
        close: close.clone(),
        escape: present(escape)?,
        multiline: boolean(multiline)?,
        raw: boolean(raw)?,
        interpolation: present(interpolation)?.map(|open| BracketPair {
            open,
            close: interpolation_close.clone(),
        }),
        class: TokenClass::from_name(class)?,
    })
}

fn keyword_group(rest: &[String]) -> Option<KeywordGroup> {
    let [class, modifiers, words @ ..] = rest else {
        return None;
    };
    let modifiers = modifiers
        .split(',')
        .filter(|name| !name.is_empty())
        .map(TokenModifier::from_name)
        .collect::<Option<Vec<_>>>()?;
    Some(KeywordGroup {
        class: TokenClass::from_name(class)?,
        modifiers,
        words: words.to_vec(),
    })
}

#[must_use]
pub const fn enter_action_name(action: EnterAction) -> &'static str {
    match action {
        EnterAction::None => "None",
        EnterAction::Indent => "Indent",
        EnterAction::IndentOutdent => "IndentOutdent",
        EnterAction::Outdent => "Outdent",
    }
}

#[must_use]
pub fn enter_action_of(name: &str) -> Option<EnterAction> {
    [
        EnterAction::None,
        EnterAction::Indent,
        EnterAction::IndentOutdent,
        EnterAction::Outdent,
    ]
    .into_iter()
    .find(|action| enter_action_name(*action) == name)
}
