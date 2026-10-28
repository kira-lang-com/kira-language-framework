//! The lexical tree-sitter grammar and its queries (`KLF-EDITOR.md`
//! section 19.3 item 2). `parser.c` is generated in process, so neither
//! Node.js nor the tree-sitter CLI is needed.

use crate::regex::translate;
use crate::scanner::{self, Delimited, LONGEST};
use crate::{File, Language, scopes};
use klf_facts::TokenClass;
use klf_facts::pattern::Engine;
use klf_facts::spec::EditorSpec;
use serde_json::{Map, Value, json};
use std::fmt::Write;

fn string(value: &str) -> Value {
    json!({ "type": "STRING", "value": value })
}

fn pattern(value: &str) -> Value {
    json!({ "type": "PATTERN", "value": value })
}

fn symbol(name: &str) -> Value {
    json!({ "type": "SYMBOL", "name": name })
}

fn choice(members: Vec<Value>) -> Value {
    let mut rule = Map::new();
    rule.insert("type".to_owned(), json!("CHOICE"));
    rule.insert("members".to_owned(), Value::Array(members));
    Value::Object(rule)
}

fn alias(name: &str, value: &str) -> Value {
    json!({ "type": "ALIAS", "content": symbol(name), "named": true, "value": value })
}

/// The node an external token is shown as, by class.
fn node_of(class: TokenClass) -> &'static str {
    match class {
        TokenClass::Character => "character",
        TokenClass::DocComment => "doc_comment",
        TokenClass::Comment => "comment",
        _ => "string",
    }
}

/// The character a string's escape pattern starts with.
fn escape_char(escape: &str) -> Option<char> {
    let mut characters = escape.chars();
    match characters.next()? {
        '\\' => characters.next(),
        '.' | '[' | '(' | '^' => None,
        other => Some(other),
    }
}

/// What the grammar names, for the queries.
#[derive(Default)]
struct Shape {
    /// The optional nodes present: `comment`, `doc_comment`, `string`,
    /// `character`, `number`, `operator`, `annotation`, `preprocessor`.
    nodes: Vec<&'static str>,
    keywords: Vec<(String, &'static str)>,
    contextual: Vec<(Vec<String>, &'static str)>,
    brackets: Vec<(String, String, String)>,
}

impl Shape {
    fn has(&self, node: &str) -> bool {
        self.nodes.contains(&node)
    }
}

/// A grammar under construction: its rules, the choices of `_item`, and the
/// external tokens with the node each is shown as.
#[derive(Default)]
struct Builder {
    rules: Map<String, Value>,
    items: Vec<Value>,
    forms: Vec<Delimited>,
    externals: Vec<Value>,
    shape: Shape,
}

impl Builder {
    fn rule(&mut self, name: &str, rule: Value, node: Option<&'static str>) {
        self.rules.insert(name.to_owned(), rule);
        match node {
            Some(node) if node != name => self.items.push(alias(name, node)),
            _ => self.items.push(symbol(name)),
        }
        if let Some(node) = node
            && !self.shape.has(node)
        {
            self.shape.nodes.push(node);
        }
    }

    fn delimited(&mut self, form: Delimited, node: &'static str) -> Result<(), String> {
        if form.open.chars().count() > LONGEST || form.close.chars().count() > LONGEST {
            return Err(format!(
                "editor: delimiter `{}`…`{}` is longer than {LONGEST} characters",
                form.open, form.close
            ));
        }
        let name = format!("_delimited_{}", self.forms.len());
        self.externals.push(symbol(&name));
        self.items.push(alias(&name, node));
        if !self.shape.has(node) {
            self.shape.nodes.push(node);
        }
        self.forms.push(form);
        Ok(())
    }
}

fn line_token(prefixes: &[String], precedence: i32) -> Value {
    let members = prefixes
        .iter()
        .map(|prefix| json!({ "type": "SEQ", "members": [string(prefix), pattern(".*")] }))
        .collect();
    json!({ "type": "TOKEN", "content": { "type": "PREC", "value": precedence, "content": choice(members) } })
}

fn grammar(language: &Language, id: &str) -> Result<(Value, Vec<Delimited>, Shape), String> {
    let spec = &language.spec;
    let mut builder = Builder::default();
    for comment in &spec.block_comments {
        let form = Delimited {
            open: comment.open.clone(),
            close: comment.close.clone(),
            nests: comment.nests,
            escape: None,
            multiline: true,
        };
        builder.delimited(form, "comment")?;
    }
    for form in &spec.strings {
        let escape = form
            .escape
            .as_deref()
            .filter(|_| !form.raw)
            .and_then(escape_char);
        let delimited = Delimited {
            open: form.open.clone(),
            close: form.close.clone(),
            nests: false,
            escape,
            multiline: form.multiline,
        };
        builder.delimited(delimited, node_of(form.class))?;
    }
    if !spec.doc_comments.is_empty() {
        builder.rule(
            "doc_comment",
            line_token(&spec.doc_comments, 2),
            Some("doc_comment"),
        );
    }
    if !spec.line_comments.is_empty() {
        builder.rule(
            "line_comment",
            line_token(&spec.line_comments, 1),
            Some("comment"),
        );
    }
    lexemes(&mut builder, spec)?;

    let mut rules = Map::new();
    rules.insert(
        "source_file".to_owned(),
        json!({ "type": "REPEAT", "content": symbol("_item") }),
    );
    rules.insert(
        "_item".to_owned(),
        choice(std::mem::take(&mut builder.items)),
    );
    rules.extend(std::mem::take(&mut builder.rules));
    let grammar = json!({
        "$schema": "https://tree-sitter.github.io/tree-sitter/assets/schemas/grammar.schema.json",
        "name": id,
        "word": "identifier",
        "rules": rules,
        "extras": [pattern("\\s")],
        "conflicts": [],
        "precedences": [],
        "externals": builder.externals,
        "inline": [],
        "supertypes": [],
    });
    Ok((grammar, builder.forms, builder.shape))
}

/// Annotations, preprocessor lines, numbers, keywords, operators,
/// identifiers, bracket groups and other punctuation.
fn lexemes(builder: &mut Builder, spec: &EditorSpec) -> Result<(), String> {
    let js = |member: &str, source: &str| translate(member, source, Engine::JavaScript);
    let as_token = |content: Value| json!({ "type": "TOKEN", "content": content });
    if let Some(annotation) = &spec.annotation {
        builder.rule(
            "annotation",
            as_token(pattern(&js("annotation", annotation)?)),
            Some("annotation"),
        );
    }
    if let Some(preprocessor) = &spec.preprocessor {
        let line = json!({ "type": "SEQ", "members": [pattern(&js("preprocessor", preprocessor)?), pattern(".*")] });
        builder.rule("preprocessor", as_token(line), Some("preprocessor"));
    }
    if !spec.numbers.is_empty() {
        let mut members = Vec::new();
        for (index, number) in spec.numbers.iter().enumerate() {
            members.push(pattern(&js(&format!("numbers[{index}]"), number)?));
        }
        builder.rule("number", as_token(choice(members)), Some("number"));
    }
    let mut taken: Vec<String> = spec
        .brackets
        .iter()
        .flat_map(|pair| [pair.open.clone(), pair.close.clone()])
        .collect();
    for (index, group) in spec.keywords.iter().enumerate() {
        let capture = scopes::capture(group.class, scopes::is_control(group));
        if group.contextual() {
            builder
                .shape
                .contextual
                .push((group.words.clone(), capture));
            continue;
        }
        let words: Vec<String> = group
            .words
            .iter()
            .filter(|word| !taken.contains(word))
            .cloned()
            .collect();
        if words.is_empty() {
            continue;
        }
        let name = format!("keyword_{index}");
        builder.rule(
            &name,
            choice(words.iter().map(|word| string(word)).collect()),
            None,
        );
        builder.shape.keywords.push((name, capture));
        taken.extend(words);
    }
    let operators: Vec<Value> = spec
        .operators
        .iter()
        .filter(|operator| !taken.contains(operator))
        .map(|operator| string(operator))
        .collect();
    if !operators.is_empty() {
        builder.rule("operator", choice(operators), Some("operator"));
    }
    builder.rule(
        "identifier",
        pattern(&js("identifier", &spec.identifier)?),
        None,
    );
    for (index, pair) in spec.brackets.iter().enumerate() {
        let name = format!("bracket_{index}");
        let group = json!({
            "type": "SEQ",
            "members": [string(&pair.open), { "type": "REPEAT", "content": symbol("_item") }, string(&pair.close)],
        });
        builder.rule(&name, group, None);
        builder
            .shape
            .brackets
            .push((name, pair.open.clone(), pair.close.clone()));
    }
    let other = json!({ "type": "PREC", "value": -1, "content": pattern("[^\\s\\w]") });
    builder.rule("punctuation", as_token(other), None);
    Ok(())
}

/// `src/grammar.json`, `src/parser.c`, `src/scanner.c`, the headers and the
/// queries, relative to the tree-sitter directory.
pub(crate) fn generate(language: &Language) -> Result<Vec<File>, String> {
    let id = language.identifier();
    let (grammar, forms, shape) = grammar(language, &id)?;
    let text = serde_json::to_string_pretty(&grammar).map_err(|error| error.to_string())?;
    let version = semantic_version(&language.version);
    let mut diagnostics = Vec::new();
    let (_, parser) = tree_sitter_generate::generate_parser_for_grammar(
        &text,
        Some(version),
        tree_sitter_generate::OptLevel::default(),
        &mut diagnostics,
    )
    .map_err(|error| format!("tree-sitter grammar for `{}`: {error}", language.name))?;
    let mut files = vec![
        File::text("src/grammar.json", text + "\n"),
        File::text("src/parser.c", parser),
        File::text("src/scanner.c", scanner::source(&id, &forms)),
        File::text(
            "src/tree_sitter/parser.h",
            tree_sitter_generate::PARSER_HEADER,
        ),
        File::text(
            "src/tree_sitter/alloc.h",
            tree_sitter_generate::ALLOC_HEADER,
        ),
        File::text(
            "src/tree_sitter/array.h",
            tree_sitter_generate::ARRAY_HEADER,
        ),
    ];
    for (name, text) in queries(&shape, Dialect::Neovim) {
        files.push(File::text(format!("queries/{name}"), text));
    }
    Ok(files)
}

fn semantic_version(version: &str) -> (u8, u8, u8) {
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u8>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// Editors read the same captures but differ in their indentation queries.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Dialect {
    Neovim,
    Helix,
    Zed,
}

/// The queries of a generated grammar for `dialect`, by file name.
fn queries(shape: &Shape, dialect: Dialect) -> Vec<(&'static str, String)> {
    let mut highlights = String::new();
    for node in &shape.nodes {
        let capture = match *node {
            "comment" => "@comment",
            "doc_comment" => "@comment.documentation",
            "character" => "@character",
            "number" => "@number",
            "operator" => "@operator",
            "annotation" => "@attribute",
            "preprocessor" => "@keyword.directive",
            _ => "@string",
        };
        let _ = writeln!(highlights, "({node}) {capture}");
    }
    for (name, capture) in &shape.keywords {
        let _ = writeln!(highlights, "({name}) {capture}");
    }
    for (words, capture) in &shape.contextual {
        let quoted: Vec<String> = words.iter().map(|word| format!("{word:?}")).collect();
        let _ = writeln!(
            highlights,
            "((identifier) {capture} (#any-of? {capture} {}))",
            quoted.join(" ")
        );
    }
    if let Some((call, _, _)) = shape.brackets.iter().find(|(_, open, _)| open == "(") {
        let _ = writeln!(highlights, "((identifier) @function . ({call}))");
    }
    highlights.push_str("(punctuation) @punctuation.delimiter\n");
    let delimiters: Vec<String> = shape
        .brackets
        .iter()
        .flat_map(|(_, open, close)| [format!("{open:?}"), format!("{close:?}")])
        .collect();
    if !delimiters.is_empty() {
        let _ = writeln!(
            highlights,
            "[{}] @punctuation.bracket",
            delimiters.join(" ")
        );
    }

    let mut brackets = String::new();
    let mut indents = String::new();
    let mut folds = String::new();
    for (name, open, close) in &shape.brackets {
        let _ = writeln!(brackets, "({open:?} @open {close:?} @close)");
        let _ = match dialect {
            Dialect::Neovim => writeln!(
                indents,
                "({name}) @indent.begin\n({name} {close:?} @indent.branch)"
            ),
            Dialect::Helix => writeln!(indents, "({name}) @indent\n({name} {close:?} @outdent)"),
            Dialect::Zed => writeln!(indents, "({name} {close:?} @end) @indent"),
        };
        let _ = writeln!(folds, "({name}) @fold");
    }
    if shape.has("comment") {
        folds.push_str("(comment) @fold\n");
    }
    let textobjects = if shape.has("comment") {
        "(comment) @comment.outer\n".to_owned()
    } else {
        String::new()
    };
    if dialect == Dialect::Zed {
        highlights = zed_captures(&highlights);
    }
    vec![
        ("highlights.scm", highlights),
        ("brackets.scm", brackets),
        ("indents.scm", indents),
        ("folds.scm", folds),
        (
            "outline.scm",
            "; A lexical grammar has no outline; the language server provides it.\n".to_owned(),
        ),
        ("textobjects.scm", textobjects),
        ("injections.scm", String::new()),
    ]
}

/// Zed resolves a capture by the longest theme key it starts with, and its
/// themes spell a few of these differently.
fn zed_captures(highlights: &str) -> String {
    [
        ("@comment.documentation", "@comment.doc"),
        ("@character", "@string"),
        ("@keyword.directive", "@preproc"),
        ("@module", "@namespace"),
    ]
    .iter()
    .fold(highlights.to_owned(), |text, (from, to)| {
        text.replace(from, to)
    })
}

/// The queries for an editor other than Neovim.
pub(crate) fn queries_for(
    language: &Language,
    dialect: Dialect,
) -> Result<Vec<(&'static str, String)>, String> {
    let (_, _, shape) = grammar(language, &language.identifier())?;
    Ok(queries(&shape, dialect))
}
