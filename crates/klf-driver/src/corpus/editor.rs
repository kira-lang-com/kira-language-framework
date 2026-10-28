//! The editor corpus runner (`KLF-EDITOR.md` Appendix A and section 24.4
//! item 2): each case's files are written to a scratch directory with their
//! markers removed, opened in the definition's own `<driver> lsp`, and each
//! check asks the server over the protocol.
//!
//! Section 31.1's `"<code>:<line>"` convention, one-based lines, is the one
//! `.Diagnostics` uses. A `.Tokens` golden file holds one line per token,
//! `line:col len class modifiers`, with the one-based line and byte column
//! of that convention, the server's token type and its modifiers separated
//! by spaces. `.Symbols` and `.Hints` fail: Appendix A gives their golden
//! files no format.

use super::CaseResult;
use super::editor_manifest::{EditorCase, EditorCheck, EditorFile, EditorManifest};
use super::editor_markers::{Mark, strip};
use super::lsp_client::{Client, uri};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;

/// An open document.
struct Document {
    path: String,
    uri: String,
    text: String,
    version: i64,
}

/// A case's open documents and its markers.
struct Case<'a> {
    client: &'a mut Client,
    documents: Vec<Document>,
    marks: HashMap<String, Mark>,
    directory: &'a Path,
}

/// Runs every case of `manifest` against `driver lsp`, writing each case's
/// files under `scratch`.
#[must_use]
pub fn run_editor_corpus(
    driver: &Path,
    manifest: &EditorManifest,
    scratch: &Path,
) -> Vec<CaseResult> {
    manifest
        .cases
        .iter()
        .map(|case| {
            let directory = scratch.join("editor").join(&manifest.name).join(&case.name);
            let mismatches =
                run_case(driver, manifest, case, &directory).unwrap_or_else(|error| vec![error]);
            CaseResult {
                name: format!("{}::{}", manifest.name, case.name),
                mismatches,
                skipped: false,
            }
        })
        .collect()
}

fn run_case(
    driver: &Path,
    manifest: &EditorManifest,
    case: &EditorCase,
    directory: &Path,
) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let directory = std::path::absolute(directory).map_err(|error| error.to_string())?;
    let mut marks = HashMap::new();
    let mut documents = Vec::new();
    for (index, file) in case.files.iter().enumerate() {
        let text = strip(&file.text, index, &mut marks)?;
        let path = directory.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(&path, &text)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        documents.push(Document {
            path: file.path.clone(),
            uri: uri(&path),
            text,
            version: 1,
        });
    }
    let mut client = Client::start(driver, &directory)?;
    for document in &documents {
        let language = Path::new(&document.path)
            .extension()
            .map_or_else(String::new, |extension| {
                extension.to_string_lossy().into_owned()
            });
        client.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": document.uri, "languageId": language,
                "version": document.version, "text": document.text
            }}),
        )?;
    }
    let uris: Vec<String> = documents
        .iter()
        .map(|document| document.uri.clone())
        .collect();
    client.await_diagnostics(&uris)?;
    let mut state = Case {
        client: &mut client,
        documents,
        marks,
        directory: &manifest.directory,
    };
    let mut mismatches = Vec::new();
    for check in &case.checks {
        if let Err(mismatch) = state.check(check) {
            mismatches.push(mismatch);
        }
    }
    client.shut_down();
    Ok(mismatches)
}

/// The LSP position of byte `offset` in `text`, with UTF-8 columns.
fn position(text: &str, offset: usize) -> Value {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count();
    let column = before.len() - before.rfind('\n').map_or(0, |index| index + 1);
    json!({ "line": line, "character": column })
}

/// The byte offset of the LSP position `value` in `text`.
fn offset(text: &str, value: &Value) -> usize {
    let line = usize::try_from(value["line"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
    let column = usize::try_from(value["character"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
    let start = text
        .split_inclusive('\n')
        .take(line)
        .map(str::len)
        .sum::<usize>();
    (start + column).min(text.len())
}

/// Applies LSP text edits to `text`.
fn apply(text: &str, edits: &[Value]) -> String {
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|edit| {
            (
                offset(text, &edit["range"]["start"]),
                offset(text, &edit["range"]["end"]),
                edit["newText"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    spans.sort_by_key(|(start, end, _)| (*start, *end));
    let mut out = text.to_owned();
    for (start, end, new) in spans.into_iter().rev() {
        out.replace_range(start..end, new);
    }
    out
}

/// The text `expect` holds, without markers.
fn expected(file: &EditorFile) -> Result<String, String> {
    strip(&file.text, 0, &mut HashMap::new())
}

/// Every location of a definition or references answer, as a URI and a
/// range.
fn locations(answer: &Value) -> Vec<(String, Value)> {
    let items = match answer {
        Value::Array(items) => items.clone(),
        Value::Null => Vec::new(),
        single => vec![single.clone()],
    };
    items
        .into_iter()
        .map(|item| match item.get("targetUri") {
            Some(target) => (
                target.as_str().unwrap_or_default().to_owned(),
                item["targetSelectionRange"].clone(),
            ),
            None => (
                item["uri"].as_str().unwrap_or_default().to_owned(),
                item["range"].clone(),
            ),
        })
        .collect()
}

fn names(items: &[Value], key: &str) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| item[key].as_str().map(str::to_owned))
        .collect()
}

impl Case<'_> {
    fn mark(&self, name: &str) -> Result<Mark, String> {
        self.marks
            .get(name)
            .copied()
            .ok_or_else(|| format!("no marker `{name}` in the case"))
    }

    /// The document and position parameters for the marker `name`.
    fn at(&self, name: &str) -> Result<Value, String> {
        let mark = self.mark(name)?;
        let document = &self.documents[mark.file];
        Ok(json!({
            "textDocument": { "uri": document.uri },
            "position": position(&document.text, mark.start)
        }))
    }

    /// Whether a location is what the marker `name` marks: a position's
    /// start, or a range's start and end.
    fn lands_on(&self, location: &(String, Value), name: &str) -> Result<bool, String> {
        let mark = self.mark(name)?;
        let document = &self.documents[mark.file];
        let (uri, range) = location;
        let start = offset(&document.text, &range["start"]);
        let end = offset(&document.text, &range["end"]);
        Ok(*uri == document.uri
            && start == mark.start
            && (mark.start == mark.end || end == mark.end))
    }

    fn describe(&self, locations: &[(String, Value)]) -> String {
        let text: Vec<String> = locations
            .iter()
            .map(|(uri, range)| {
                let path = self
                    .documents
                    .iter()
                    .find(|document| document.uri == *uri)
                    .map_or(uri.as_str(), |document| document.path.as_str());
                format!(
                    "{path}:{}:{}",
                    range["start"]["line"].as_u64().unwrap_or(0) + 1,
                    range["start"]["character"].as_u64().unwrap_or(0) + 1
                )
            })
            .collect();
        format!("[{}]", text.join(", "))
    }

    fn document(&self, path: &str) -> Result<usize, String> {
        self.documents
            .iter()
            .position(|document| document.path == path)
            .ok_or_else(|| format!("no file `{path}` in the case"))
    }

    /// Compares each file of `expect` with the text `edited` gives its path.
    fn compare_files(
        &self,
        what: &str,
        expect: &[EditorFile],
        edited: &HashMap<String, String>,
    ) -> Result<(), String> {
        for file in expect {
            let index = self.document(&file.path)?;
            let uri = &self.documents[index].uri;
            let got = edited.get(uri).unwrap_or(&self.documents[index].text);
            let want = expected(file)?;
            if *got != want {
                return Err(format!(
                    "{what} `{}`: expected {want:?}, got {got:?}",
                    file.path
                ));
            }
        }
        Ok(())
    }

    /// Each document's text after `edit`, a workspace edit.
    fn edited(&self, edit: &Value) -> HashMap<String, String> {
        let mut texts = HashMap::new();
        let mut change = |uri: &str, edits: &[Value]| {
            if let Some(document) = self.documents.iter().find(|document| document.uri == uri) {
                let base = texts
                    .get(uri)
                    .cloned()
                    .unwrap_or_else(|| document.text.clone());
                texts.insert(uri.to_owned(), apply(&base, edits));
            }
        };
        if let Some(changes) = edit["changes"].as_object() {
            for (uri, edits) in changes {
                change(uri, edits.as_array().map_or(&[], Vec::as_slice));
            }
        }
        for item in edit["documentChanges"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
        {
            if let Some(uri) = item["textDocument"]["uri"].as_str() {
                change(uri, item["edits"].as_array().map_or(&[], Vec::as_slice));
            }
        }
        texts
    }

    fn check(&mut self, check: &EditorCheck) -> Result<(), String> {
        match check {
            EditorCheck::Diagnostics(expected) => self.diagnostics(expected)?,
            EditorCheck::Definition { at, target } => {
                let answer = self
                    .client
                    .request("textDocument/definition", self.at(at)?)?;
                let found = locations(&answer);
                if found.len() != 1 || !self.lands_on(&found[0], target)? {
                    return Err(format!(
                        "definition at `{at}`: expected `{target}`, got {}",
                        self.describe(&found)
                    ));
                }
            }
            EditorCheck::References {
                at,
                targets,
                include_declaration,
            } => {
                self.references(at, targets, *include_declaration)?;
            }
            EditorCheck::Hover { at, contains } => {
                let answer = self.client.request("textDocument/hover", self.at(at)?)?;
                let text = hover_text(&answer["contents"]);
                if let Some(missing) = contains
                    .iter()
                    .find(|wanted| !text.contains(wanted.as_str()))
                {
                    return Err(format!("hover at `{at}`: {missing:?} is not in {text:?}"));
                }
            }
            EditorCheck::Completes {
                at,
                includes,
                excludes,
            } => {
                let answer = self
                    .client
                    .request("textDocument/completion", self.at(at)?)?;
                let items = answer.get("items").unwrap_or(&answer);
                let labels = names(items.as_array().map_or(&[], Vec::as_slice), "label");
                let missing: Vec<&String> = includes
                    .iter()
                    .filter(|label| !labels.contains(label))
                    .collect();
                let present: Vec<&String> = excludes
                    .iter()
                    .filter(|label| labels.contains(label))
                    .collect();
                if !missing.is_empty() || !present.is_empty() {
                    return Err(format!(
                        "completion at `{at}`: missing {missing:?}, unwanted {present:?}, got {labels:?}"
                    ));
                }
            }
            EditorCheck::Signature {
                at,
                label,
                active_parameter,
            } => {
                let answer = self
                    .client
                    .request("textDocument/signatureHelp", self.at(at)?)?;
                let active =
                    usize::try_from(answer["activeSignature"].as_u64().unwrap_or(0)).unwrap_or(0);
                let signature = &answer["signatures"][active];
                let got_label = signature["label"].as_str().unwrap_or_default();
                let got_parameter = signature["activeParameter"]
                    .as_i64()
                    .or_else(|| answer["activeParameter"].as_i64())
                    .unwrap_or(-1);
                if got_label != label || got_parameter != *active_parameter {
                    return Err(format!(
                        "signature at `{at}`: expected {label:?} parameter {active_parameter}, got {got_label:?} parameter {got_parameter}"
                    ));
                }
            }
            EditorCheck::Rename {
                at,
                new_name,
                expect,
            } => {
                let mut params = self.at(at)?;
                params["newName"] = json!(new_name);
                let edit = self.client.request("textDocument/rename", params)?;
                self.compare_files("rename", expect, &self.edited(&edit))?;
            }
            EditorCheck::Tokens(golden) => self.tokens(golden)?,
            EditorCheck::Symbols(_) => {
                return Err("`.Symbols`: Appendix A gives the golden outline no format".into());
            }
            EditorCheck::Hints(_) => {
                return Err("`.Hints`: Appendix A gives the golden hints no format".into());
            }
            EditorCheck::Folds(ranges) => self.folds(ranges)?,
            EditorCheck::Action { at, title, expect } => self.action(at, title, expect)?,
            EditorCheck::Format(expect) => self.format(expect)?,
            EditorCheck::Edit { at, insert, then } => self.edit(at, insert, then)?,
        }
        Ok(())
    }

    fn diagnostics(&self, expected: &[String]) -> Result<(), String> {
        let mut got = Vec::new();
        for document in &self.documents {
            let mut published = self
                .client
                .diagnostics
                .get(&document.uri)
                .cloned()
                .unwrap_or_default();
            published.sort_by_key(|diagnostic| {
                let start = &diagnostic["range"]["start"];
                (start["line"].as_u64(), start["character"].as_u64())
            });
            for diagnostic in published {
                let code = match &diagnostic["code"] {
                    Value::String(code) => code.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                let line = diagnostic["range"]["start"]["line"].as_u64().unwrap_or(0) + 1;
                got.push(format!("{code}:{line}"));
            }
        }
        if got != *expected {
            return Err(format!("diagnostics: expected {expected:?}, got {got:?}"));
        }
        Ok(())
    }

    fn references(
        &mut self,
        at: &str,
        targets: &[String],
        include_declaration: bool,
    ) -> Result<(), String> {
        let mut params = self.at(at)?;
        params["context"] = json!({ "includeDeclaration": include_declaration });
        let found = locations(&self.client.request("textDocument/references", params)?);
        let mut matched = found.len() == targets.len();
        for target in targets {
            let mut hit = false;
            for location in &found {
                hit |= self.lands_on(location, target)?;
            }
            matched &= hit;
        }
        if !matched {
            return Err(format!(
                "references at `{at}`: expected {targets:?}, got {}",
                self.describe(&found)
            ));
        }
        Ok(())
    }

    fn action(&mut self, at: &str, title: &str, expect: &[EditorFile]) -> Result<(), String> {
        let mark = self.mark(at)?;
        let document = &self.documents[mark.file];
        let range = json!({
            "start": position(&document.text, mark.start),
            "end": position(&document.text, mark.end)
        });
        let answer = self.client.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": document.uri },
                "range": range,
                "context": { "diagnostics": [] }
            }),
        )?;
        let actions = answer.as_array().cloned().unwrap_or_default();
        let mut action = actions
            .iter()
            .find(|action| action["title"] == title)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "no action {title:?} at `{at}`, got {:?}",
                    names(&actions, "title")
                )
            })?;
        if action.get("edit").is_none() {
            action = self.client.request("codeAction/resolve", action)?;
        }
        self.compare_files("action", expect, &self.edited(&action["edit"]))?;
        Ok(())
    }

    fn format(&mut self, expect: &[EditorFile]) -> Result<(), String> {
        let mut edited = HashMap::new();
        for file in expect {
            let document = &self.documents[self.document(&file.path)?];
            let edits = self.client.request(
                "textDocument/formatting",
                json!({
                    "textDocument": { "uri": document.uri },
                    "options": { "tabSize": 4, "insertSpaces": true }
                }),
            )?;
            let edits = edits.as_array().cloned().unwrap_or_default();
            edited.insert(document.uri.clone(), apply(&document.text, &edits));
        }
        self.compare_files("format", expect, &edited)?;
        Ok(())
    }

    fn tokens(&mut self, golden: &str) -> Result<(), String> {
        let path = self.directory.join(golden);
        let want = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let document = &self.documents[0];
        let answer = self.client.request(
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": document.uri } }),
        )?;
        let data: Vec<u64> = answer["data"]
            .as_array()
            .map(|items| items.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        let (types, modifiers) = &self.client.legend;
        let (mut line, mut column) = (0, 0);
        let mut got = String::new();
        for token in data.chunks(5) {
            let [delta_line, delta_column, length, kind, bits] = token else {
                break;
            };
            if *delta_line > 0 {
                column = 0;
            }
            line += delta_line;
            column += delta_column;
            let kind = types
                .get(usize::try_from(*kind).unwrap_or(usize::MAX))
                .map_or("?", String::as_str);
            let mut fields = vec![format!("{}:{} {length} {kind}", line + 1, column + 1)];
            fields.extend(
                modifiers
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| bits & (1 << index) != 0)
                    .map(|(_, name)| name.clone()),
            );
            got.push_str(&fields.join(" "));
            got.push('\n');
        }
        let normal = |text: &str| -> Vec<String> {
            text.lines()
                .map(|line| line.trim_end().to_owned())
                .filter(|line| !line.is_empty())
                .collect()
        };
        if normal(&got) != normal(&want) {
            return Err(format!(
                "tokens: expected {:?}, got {:?}",
                normal(&want),
                normal(&got)
            ));
        }
        Ok(())
    }

    fn folds(&mut self, ranges: &[String]) -> Result<(), String> {
        let mut files: Vec<usize> = Vec::new();
        let mut want = Vec::new();
        for name in ranges {
            let mark = self.mark(name)?;
            let text = &self.documents[mark.file].text;
            want.push((
                mark.file,
                position(text, mark.start)["line"].clone(),
                position(text, mark.end)["line"].clone(),
            ));
            if !files.contains(&mark.file) {
                files.push(mark.file);
            }
        }
        if files.is_empty() {
            files.push(0);
        }
        let mut got = Vec::new();
        for file in files {
            let uri = self.documents[file].uri.clone();
            let answer = self.client.request(
                "textDocument/foldingRange",
                json!({ "textDocument": { "uri": uri } }),
            )?;
            for range in answer.as_array().map_or(&[][..], Vec::as_slice) {
                got.push((file, range["startLine"].clone(), range["endLine"].clone()));
            }
        }
        let key = |item: &(usize, Value, Value)| (item.0, item.1.as_u64(), item.2.as_u64());
        want.sort_by_key(key);
        got.sort_by_key(key);
        if got != want {
            return Err(format!("folds: expected lines {want:?}, got {got:?}"));
        }
        Ok(())
    }

    /// Inserts `insert` at the marker `at`, runs `then`, and takes the
    /// insertion out again, so later checks see the case as written.
    fn edit(&mut self, at: &str, insert: &str, then: &[EditorCheck]) -> Result<(), String> {
        let mark = self.mark(at)?;
        let mut added = HashMap::new();
        let inserted = strip(insert, mark.file, &mut added)?;
        let saved = self.marks.clone();
        for other in self.marks.values_mut() {
            if other.file == mark.file {
                if other.start >= mark.start {
                    other.start += inserted.len();
                }
                if other.end >= mark.start {
                    other.end += inserted.len();
                }
            }
        }
        for (name, mut new) in added {
            new.start += mark.start;
            new.end += mark.start;
            self.marks.insert(name, new);
        }
        let original = self.documents[mark.file].text.clone();
        let mut text = original.clone();
        text.insert_str(mark.start, &inserted);
        self.replace(mark.file, text)?;
        let mut result = Ok(());
        for check in then {
            if let Err(mismatch) = self.check(check) {
                result = Err(format!("after inserting {insert:?} at `{at}`: {mismatch}"));
                break;
            }
        }
        self.marks = saved;
        self.replace(mark.file, original)?;
        result
    }

    /// Sends `text` as the new content of document `file` and waits for
    /// its diagnostics.
    fn replace(&mut self, file: usize, text: String) -> Result<(), String> {
        let document = &mut self.documents[file];
        document.version += 1;
        document.text = text;
        let uri = document.uri.clone();
        self.client.diagnostics.remove(&uri);
        self.client.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": document.version },
                "contentChanges": [{ "text": document.text }]
            }),
        )?;
        self.client.await_diagnostics(&[uri])
    }
}

/// The text of a hover's contents, whichever form the server sent.
fn hover_text(contents: &Value) -> String {
    match contents {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(hover_text).collect::<Vec<_>>().join("\n"),
        Value::Object(object) => object
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}
