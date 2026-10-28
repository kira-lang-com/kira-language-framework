//! The `TextMate` grammar (`KLF-EDITOR.md` section 19.3 item 1).

use crate::Language;
use crate::regex::{alternation, literal, translate};
use crate::scopes;
use klf_facts::TokenClass;
use klf_facts::pattern::Engine;
use klf_facts::spec::EditorSpec;
use serde_json::{Map, Value, json};

/// The repository of a grammar, in the order its rules are tried.
struct Rules {
    suffix: String,
    order: Vec<String>,
    repository: Map<String, Value>,
}

impl Rules {
    fn add(&mut self, name: String, rule: Value) {
        self.order.push(name.clone());
        self.repository.insert(name, rule);
    }

    fn scope(&self, name: &str) -> String {
        format!("{name}.{}", self.suffix)
    }
}

pub(crate) fn grammar(language: &Language) -> Result<Value, String> {
    let spec = &language.spec;
    let mut rules = Rules {
        suffix: language.identifier(),
        order: Vec::new(),
        repository: Map::new(),
    };
    comments(&mut rules, spec);
    strings(&mut rules, spec)?;
    words(&mut rules, spec)?;
    let patterns: Vec<Value> = rules
        .order
        .iter()
        .map(|name| json!({ "include": format!("#{name}") }))
        .collect();
    let mut grammar = json!({
        "$schema": "https://raw.githubusercontent.com/martinring/tmlanguage/master/tmlanguage.json",
        "name": spec.display_name,
        "scopeName": spec.scope,
        "fileTypes": language.extensions,
        "patterns": patterns,
        "repository": rules.repository,
    });
    if let Some(first_line) = &spec.first_line {
        grammar["firstLineMatch"] = json!(translate("firstLine", first_line, Engine::Oniguruma)?);
    }
    Ok(grammar)
}

fn comments(rules: &mut Rules, spec: &EditorSpec) {
    for (index, prefix) in spec.doc_comments.iter().enumerate() {
        let rule = json!({
            "name": rules.scope("comment.line.documentation"),
            "match": format!("{}.*$", literal(prefix)),
        });
        rules.add(format!("doc_comment_{index}"), rule);
    }
    for (index, prefix) in spec.line_comments.iter().enumerate() {
        let rule = json!({ "name": rules.scope("comment.line"), "match": format!("{}.*$", literal(prefix)) });
        rules.add(format!("line_comment_{index}"), rule);
    }
    for (index, comment) in spec.block_comments.iter().enumerate() {
        let name = format!("block_comment_{index}");
        let mut rule = json!({
            "name": rules.scope("comment.block"),
            "begin": literal(&comment.open),
            "end": literal(&comment.close),
        });
        if comment.nests {
            rule["patterns"] = json!([{ "include": format!("#{name}") }]);
        }
        rules.add(name, rule);
    }
}

fn strings(rules: &mut Rules, spec: &EditorSpec) -> Result<(), String> {
    for (index, form) in spec.strings.iter().enumerate() {
        let mut patterns = Vec::new();
        if let Some(escape) = form.escape.as_ref().filter(|_| !form.raw) {
            patterns.push(json!({
                "name": rules.scope(scopes::textmate(TokenClass::Escape, false)),
                "match": translate(&format!("strings[{index}].escape"), escape, Engine::Oniguruma)?,
            }));
        }
        if let Some(pair) = &form.interpolation {
            patterns.push(json!({
                "name": rules.scope("meta.interpolation"),
                "begin": literal(&pair.open),
                "end": literal(&pair.close),
                "patterns": [{ "include": "$self" }],
            }));
        }
        let close = literal(&form.close);
        let end = if form.multiline {
            close
        } else {
            format!("{close}|$")
        };
        let rule = json!({
            "name": rules.scope(scopes::textmate(form.class, false)),
            "begin": literal(&form.open),
            "end": end,
            "patterns": patterns,
        });
        rules.add(format!("string_{index}"), rule);
    }
    Ok(())
}

/// Annotations, preprocessor lines, keywords, numbers, calls, operators and
/// brackets.
fn words(rules: &mut Rules, spec: &EditorSpec) -> Result<(), String> {
    if let Some(annotation) = &spec.annotation {
        let rule = json!({
            "name": rules.scope("storage.type.annotation"),
            "match": translate("annotation", annotation, Engine::Oniguruma)?,
        });
        rules.add("annotation".to_owned(), rule);
    }
    if let Some(preprocessor) = &spec.preprocessor {
        let rule = json!({
            "name": rules.scope("meta.preprocessor"),
            "match": format!("^\\s*(?:{}).*$", translate("preprocessor", preprocessor, Engine::Oniguruma)?),
        });
        rules.add("preprocessor".to_owned(), rule);
    }
    for (index, group) in spec.keywords.iter().enumerate() {
        let words = alternation(&group.words);
        let pattern = if scopes::is_wordlike(group) {
            format!("\\b(?:{words})\\b")
        } else {
            format!("(?:{words})")
        };
        let rule = json!({
            "name": rules.scope(scopes::textmate(group.class, scopes::is_control(group))),
            "match": pattern,
        });
        rules.add(format!("keywords_{index}"), rule);
    }
    if !spec.numbers.is_empty() {
        let mut alternatives = Vec::new();
        for (index, number) in spec.numbers.iter().enumerate() {
            let pattern = translate(&format!("numbers[{index}]"), number, Engine::Oniguruma)?;
            alternatives.push(format!("(?:{pattern})"));
        }
        let rule = json!({
            "name": rules.scope("constant.numeric"),
            "match": format!("\\b(?:{})", alternatives.join("|")),
        });
        rules.add("numbers".to_owned(), rule);
    }
    if spec.brackets.iter().any(|pair| pair.open == "(") {
        let identifier = translate("identifier", &spec.identifier, Engine::Oniguruma)?;
        let rule = json!({
            "match": format!("({identifier})(?=\\s*\\()"),
            "captures": { "1": { "name": rules.scope("entity.name.function") } },
        });
        rules.add("call".to_owned(), rule);
    }
    if !spec.operators.is_empty() {
        let rule = json!({ "name": rules.scope("keyword.operator"), "match": alternation(&spec.operators) });
        rules.add("operators".to_owned(), rule);
    }
    let brackets: Vec<String> = spec
        .brackets
        .iter()
        .flat_map(|pair| [pair.open.clone(), pair.close.clone()])
        .collect();
    if !brackets.is_empty() {
        let rule =
            json!({ "name": rules.scope("punctuation.section"), "match": alternation(&brackets) });
        rules.add("brackets".to_owned(), rule);
    }
    Ok(())
}
