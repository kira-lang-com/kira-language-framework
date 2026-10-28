//! The optional `editor` member of a `Language` declaration (`KLF-EDITOR.md`
//! section 5.2), read from its `EditorSpec` literal and checked.

use klf_diag::Diagnostic;
use klf_facts::spec::{
    AutoCloseContext, AutoClosePair, BlockComment, BracketPair, EditorSpec, GrammarRef,
    IndentRules, KeywordGroup, OnEnterRule, StringForm,
};
use klf_facts::{TokenClass, TokenModifier, enter_action_of};
use klf_kira_syntax::ast::{Expr, ExprKind, TypeKind};
use klf_services::Session;
use klf_source::Span;
use std::collections::HashMap;

/// Reads `editor` if the declaration has it. `None` is a reported error;
/// `Some(None)` means the member is absent and the language gets
/// `EditorSpec.minimal`.
#[allow(clippy::option_option)]
pub(crate) fn optional_editor(
    session: &mut Session,
    value: Option<&Expr>,
) -> Option<Option<EditorSpec>> {
    let Some(value) = value else {
        return Some(None);
    };
    let mut reader = Reader { session };
    let spec = reader.spec(value)?;
    let problems = spec.validate();
    for (code, message) in &problems {
        reader.session.report(
            Diagnostic::error_at(
                code,
                value.span,
                format!("Language metadata field `editor`: {message}"),
            )
            .with_label(value.span, message.clone()),
        );
    }
    problems.is_empty().then_some(Some(spec))
}

struct Reader<'a> {
    session: &'a mut Session,
}

type Fields<'e> = HashMap<&'e str, &'e Expr>;

impl Reader<'_> {
    fn report(&mut self, span: Span, message: &str) {
        self.session.report(
            Diagnostic::error_at(
                "KLF-DEF-0002",
                span,
                format!("Language metadata field `editor` {message}"),
            )
            .with_label(span, message.to_owned()),
        );
    }

    /// The fields of a `Type { ... }` literal, all of them among `known`.
    fn record<'e>(&mut self, value: &'e Expr, ty: &str, known: &[&str]) -> Option<Fields<'e>> {
        let ExprKind::StructLit {
            ty: written,
            fields,
        } = &value.kind
        else {
            self.report(value.span, &format!("expected a {ty} literal"));
            return None;
        };
        let named = matches!(&written.kind, TypeKind::Named { path, args }
            if args.is_empty() && path.segments.len() == 1 && path.segments[0].name.as_str() == ty);
        if !named {
            self.report(value.span, &format!("expected a {ty} literal"));
            return None;
        }
        let mut found = Fields::new();
        for (name, init) in fields {
            if !known.contains(&name.name.as_str()) {
                self.report(name.span, &format!("has no {ty} field `{}`", name.name));
                return None;
            }
            if found.insert(name.name.as_str(), init).is_some() {
                self.report(name.span, &format!("sets {ty} field `{}` twice", name.name));
                return None;
            }
        }
        Some(found)
    }

    fn field<'e, T>(
        &mut self,
        fields: &Fields<'e>,
        name: &str,
        span: Span,
        read: impl FnOnce(&mut Self, &'e Expr) -> Option<T>,
    ) -> Option<T> {
        let value = self.required(fields, name, span)?;
        read(self, value)
    }

    fn required<'e>(&mut self, fields: &Fields<'e>, name: &str, span: Span) -> Option<&'e Expr> {
        let value = fields.get(name).copied();
        if value.is_none() {
            self.report(span, &format!("needs `{name}`"));
        }
        value
    }

    fn string(&mut self, value: &Expr) -> Option<String> {
        if let ExprKind::Str(text) = &value.kind {
            return Some(text.clone());
        }
        self.report(value.span, "expected a string literal");
        None
    }

    fn boolean(&mut self, value: &Expr) -> Option<bool> {
        if let ExprKind::Bool(flag) = &value.kind {
            return Some(*flag);
        }
        self.report(value.span, "expected `true` or `false`");
        None
    }

    fn array<T>(
        &mut self,
        value: &Expr,
        mut item: impl FnMut(&mut Self, &Expr) -> Option<T>,
    ) -> Option<Vec<T>> {
        let ExprKind::ArrayLit(items) = &value.kind else {
            self.report(value.span, "expected an array literal");
            return None;
        };
        items.iter().map(|value| item(self, value)).collect()
    }

    fn strings(&mut self, value: &Expr) -> Option<Vec<String>> {
        self.array(value, Self::string)
    }

    /// An `Optional<T>` written `.None` or `.Some(value)`.
    #[allow(clippy::option_option)]
    fn optional<T>(
        &mut self,
        value: &Expr,
        item: impl FnOnce(&mut Self, &Expr) -> Option<T>,
    ) -> Option<Option<T>> {
        match &value.kind {
            ExprKind::EnumLit { name, args: None } if name.name.as_str() == "None" => Some(None),
            ExprKind::EnumLit {
                name,
                args: Some(args),
            } if name.name.as_str() == "Some" && args.len() == 1 => {
                item(self, &args[0].value).map(Some)
            }
            _ => {
                self.report(value.span, "expected `.None` or `.Some(value)`");
                None
            }
        }
    }

    fn case<T>(
        &mut self,
        value: &Expr,
        what: &str,
        parse: impl Fn(&str) -> Option<T>,
    ) -> Option<T> {
        if let ExprKind::EnumLit { name, args: None } = &value.kind
            && let Some(parsed) = parse(name.name.as_str())
        {
            return Some(parsed);
        }
        self.report(value.span, &format!("expected a {what} case"));
        None
    }

    fn token_class(&mut self, value: &Expr) -> Option<TokenClass> {
        self.case(value, "TokenClass", TokenClass::from_name)
    }

    fn spec(&mut self, value: &Expr) -> Option<EditorSpec> {
        let fields = self.record(
            value,
            "EditorSpec",
            &[
                "displayName",
                "scope",
                "fileNames",
                "rootMarkers",
                "firstLine",
                "lineComments",
                "blockComments",
                "docComments",
                "strings",
                "numbers",
                "identifier",
                "keywords",
                "operators",
                "brackets",
                "autoClose",
                "indent",
                "onEnter",
                "regionMarkers",
                "annotation",
                "preprocessor",
                "grammar",
                "icon",
            ],
        )?;
        let span = value.span;
        let mut spec = EditorSpec::minimal("");
        spec.display_name = self.field(&fields, "displayName", span, Self::string)?;
        spec.scope = self.field(&fields, "scope", span, Self::string)?;
        spec.line_comments = self.field(&fields, "lineComments", span, Self::strings)?;
        spec.block_comments = self.field(&fields, "blockComments", span, |reader, value| {
            reader.array(value, Self::block_comment)
        })?;
        spec.strings = self.field(&fields, "strings", span, |reader, value| {
            reader.array(value, Self::string_form)
        })?;
        spec.numbers = self.field(&fields, "numbers", span, Self::strings)?;
        spec.identifier = self.field(&fields, "identifier", span, Self::string)?;
        spec.keywords = self.field(&fields, "keywords", span, |reader, value| {
            reader.array(value, Self::keyword_group)
        })?;
        spec.operators = self.field(&fields, "operators", span, Self::strings)?;
        spec.brackets = self.field(&fields, "brackets", span, |reader, value| {
            reader.array(value, Self::bracket_pair)
        })?;
        spec.auto_close = self.field(&fields, "autoClose", span, |reader, value| {
            reader.array(value, Self::auto_close)
        })?;
        spec.indent = self.field(&fields, "indent", span, Self::indent)?;
        spec.file_names = self
            .defaulted(&fields, "fileNames", Self::strings)?
            .unwrap_or_default();
        spec.root_markers = self
            .defaulted(&fields, "rootMarkers", Self::strings)?
            .unwrap_or_default();
        spec.doc_comments = self
            .defaulted(&fields, "docComments", Self::strings)?
            .unwrap_or_default();
        spec.on_enter = self
            .defaulted(&fields, "onEnter", |reader, value| {
                reader.array(value, Self::on_enter)
            })?
            .unwrap_or_default();
        spec.icon = self
            .defaulted(&fields, "icon", Self::string)?
            .unwrap_or_default();
        spec.first_line = self.optional_member(&fields, "firstLine", Self::string)?;
        spec.annotation = self.optional_member(&fields, "annotation", Self::string)?;
        spec.preprocessor = self.optional_member(&fields, "preprocessor", Self::string)?;
        spec.region_markers =
            self.optional_member(&fields, "regionMarkers", Self::region_markers)?;
        spec.grammar = self.optional_member(&fields, "grammar", Self::grammar)?;
        Some(spec)
    }

    /// A member with a default: `Some(None)` when it is left out.
    #[allow(clippy::option_option)]
    fn defaulted<T>(
        &mut self,
        fields: &Fields<'_>,
        name: &str,
        read: impl FnOnce(&mut Self, &Expr) -> Option<T>,
    ) -> Option<Option<T>> {
        match fields.get(name) {
            None => Some(None),
            Some(value) => read(self, value).map(Some),
        }
    }

    /// An `Optional<T>` member that defaults to `.None`.
    #[allow(clippy::option_option)]
    fn optional_member<T>(
        &mut self,
        fields: &Fields<'_>,
        name: &str,
        read: impl FnOnce(&mut Self, &Expr) -> Option<T>,
    ) -> Option<Option<T>> {
        match fields.get(name) {
            None => Some(None),
            Some(value) => self.optional(value, read),
        }
    }

    fn block_comment(&mut self, value: &Expr) -> Option<BlockComment> {
        let fields = self.record(value, "BlockComment", &["open", "close", "nests"])?;
        Some(BlockComment {
            open: self.field(&fields, "open", value.span, Self::string)?,
            close: self.field(&fields, "close", value.span, Self::string)?,
            nests: self.field(&fields, "nests", value.span, Self::boolean)?,
        })
    }

    fn bracket_pair(&mut self, value: &Expr) -> Option<BracketPair> {
        let fields = self.record(value, "BracketPair", &["open", "close"])?;
        Some(BracketPair {
            open: self.field(&fields, "open", value.span, Self::string)?,
            close: self.field(&fields, "close", value.span, Self::string)?,
        })
    }

    fn string_form(&mut self, value: &Expr) -> Option<StringForm> {
        let span = value.span;
        let fields = self.record(
            value,
            "StringForm",
            &[
                "open",
                "close",
                "escape",
                "multiline",
                "raw",
                "interpolation",
                "class",
            ],
        )?;
        Some(StringForm {
            open: self.field(&fields, "open", span, Self::string)?,
            close: self.field(&fields, "close", span, Self::string)?,
            escape: self.field(&fields, "escape", span, |reader, value| {
                reader.optional(value, Self::string)
            })?,
            multiline: self.field(&fields, "multiline", span, Self::boolean)?,
            raw: self.field(&fields, "raw", span, Self::boolean)?,
            interpolation: self.field(&fields, "interpolation", span, |reader, value| {
                reader.optional(value, Self::bracket_pair)
            })?,
            class: self.field(&fields, "class", span, Self::token_class)?,
        })
    }

    fn keyword_group(&mut self, value: &Expr) -> Option<KeywordGroup> {
        let span = value.span;
        let fields = self.record(value, "KeywordGroup", &["class", "modifiers", "words"])?;
        Some(KeywordGroup {
            class: self.field(&fields, "class", span, Self::token_class)?,
            modifiers: self.field(&fields, "modifiers", span, |reader, value| {
                reader.array(value, |reader, value| {
                    reader.case(value, "TokenModifier", TokenModifier::from_name)
                })
            })?,
            words: self.field(&fields, "words", span, Self::strings)?,
        })
    }

    fn auto_close(&mut self, value: &Expr) -> Option<AutoClosePair> {
        let span = value.span;
        let fields = self.record(value, "AutoClosePair", &["open", "close", "notIn"])?;
        Some(AutoClosePair {
            open: self.field(&fields, "open", span, Self::string)?,
            close: self.field(&fields, "close", span, Self::string)?,
            not_in: self.field(&fields, "notIn", span, |reader, value| {
                reader.array(value, |reader, value| {
                    reader.case(value, "AutoCloseContext", |name| match name {
                        "String" => Some(AutoCloseContext::String),
                        "Comment" => Some(AutoCloseContext::Comment),
                        _ => None,
                    })
                })
            })?,
        })
    }

    fn indent(&mut self, value: &Expr) -> Option<IndentRules> {
        let span = value.span;
        let fields = self.record(value, "IndentRules", &["unit", "increase", "decrease"])?;
        Some(IndentRules {
            unit: self.field(&fields, "unit", span, Self::string)?,
            increase: self.field(&fields, "increase", span, |reader, value| {
                reader.optional(value, Self::string)
            })?,
            decrease: self.field(&fields, "decrease", span, |reader, value| {
                reader.optional(value, Self::string)
            })?,
        })
    }

    fn on_enter(&mut self, value: &Expr) -> Option<OnEnterRule> {
        let span = value.span;
        let fields = self.record(
            value,
            "OnEnterRule",
            &["before", "after", "action", "append"],
        )?;
        Some(OnEnterRule {
            before: self.field(&fields, "before", span, Self::string)?,
            after: self.field(&fields, "after", span, |reader, value| {
                reader.optional(value, Self::string)
            })?,
            action: self.field(&fields, "action", span, |reader, value| {
                reader.case(value, "EnterAction", enter_action_of)
            })?,
            append: self.field(&fields, "append", span, Self::string)?,
        })
    }

    fn region_markers(&mut self, value: &Expr) -> Option<(String, String)> {
        let fields = self.record(value, "RegionMarkers", &["start", "end"])?;
        Some((
            self.field(&fields, "start", value.span, Self::string)?,
            self.field(&fields, "end", value.span, Self::string)?,
        ))
    }

    fn grammar(&mut self, value: &Expr) -> Option<GrammarRef> {
        let span = value.span;
        let fields = self.record(value, "GrammarRef", &["repository", "revision", "path"])?;
        Some(GrammarRef {
            repository: self.field(&fields, "repository", span, Self::string)?,
            revision: self.field(&fields, "revision", span, Self::string)?,
            path: self.field(&fields, "path", span, Self::string)?,
        })
    }
}
