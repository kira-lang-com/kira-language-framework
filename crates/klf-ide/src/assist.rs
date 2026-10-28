//! Features that explain and complete code (`KLF-EDITOR.md` sections 11.1,
//! 11.5 to 11.7, 11.12 to 11.15 and 11.18).

use crate::analysis::IdeDiagnostic;
use crate::snapshot::{FileSpan, Snapshot, offset_u32};
use klf_facts::{FoldKind, Hint, LinkTarget, Runnable, SymbolKind, TokenClass};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverText {
    /// Markdown.
    pub text: String,
    pub span: FileSpan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionKind {
    Symbol(SymbolKind),
    Keyword,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: String,
    pub documentation: String,
    pub deprecated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignatureHelp {
    pub label: String,
    pub documentation: String,
    pub parameters: Vec<String>,
    pub active: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkDestination {
    File(PathBuf),
    Url(String),
    Span(FileSpan),
}

impl Snapshot {
    #[must_use]
    pub fn diagnostics(&self, path: &Path) -> Vec<&IdeDiagnostic> {
        let Some(file) = self.file(path) else {
            return Vec::new();
        };
        self.analysis
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.file == file)
            .collect()
    }

    #[must_use]
    pub fn hover(&self, path: &Path, offset: u32) -> Option<HoverText> {
        let file = self.file(path)?;
        let facts = &self.analysis.facts;
        let language = self
            .spec(path)
            .scope
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_owned();
        let mut sections = Vec::new();
        let mut span = None;
        if let Some((location, entity)) = facts.entity_at(file, offset)
            && let Some(symbol) = facts.symbol(entity)
        {
            let heading = if symbol.signature.is_empty() {
                &symbol.name
            } else {
                &symbol.signature
            };
            sections.push(format!("```{language}\n{heading}\n```"));
            if !symbol.documentation.is_empty() {
                sections.push(symbol.documentation.clone());
            }
            span = Some((location.start, location.end));
        }
        if let Some(typed) = facts.type_at_offset(file, offset) {
            if sections.is_empty() {
                sections.push(format!("```{language}\n{}\n```", typed.text));
            } else {
                sections.push(format!("Type: `{}`", typed.text));
            }
            span.get_or_insert((typed.start, typed.end));
        }
        if let Some(value) = facts.value_at_offset(file, offset) {
            sections.push(format!("Value: `{}`", value.text));
            span.get_or_insert((value.start, value.end));
        }
        let (start, end) = span?;
        Some(HoverText {
            text: sections.join("\n\n---\n\n"),
            span: FileSpan {
                path: path.to_path_buf(),
                start,
                end,
            },
        })
    }

    /// Completions at `offset` and the span they replace (section 11.6).
    /// After an access operator they are the members of the receiver's
    /// type; elsewhere the symbols in scope and the keywords.
    #[must_use]
    pub fn completions(&self, path: &Path, offset: u32) -> (Vec<CompletionItem>, FileSpan) {
        let text = self.text(path).unwrap_or_default();
        let (start, end) = self
            .word_at(path, offset)
            .map_or((offset, offset), |(start, _)| (start, offset));
        let replaced = FileSpan {
            path: path.to_path_buf(),
            start,
            end,
        };
        let prefix = text
            .get(start as usize..end as usize)
            .unwrap_or_default()
            .to_owned();
        let facts = &self.analysis.facts;
        let file = self.file(path);
        let before = text.get(..start as usize).unwrap_or_default();
        let spec = self.spec(path);
        let operator = spec
            .access_operators()
            .into_iter()
            .find(|operator| before.trim_end().ends_with(operator.as_str()));
        let mut items = Vec::new();
        if let (Some(operator), Some(file)) = (operator, file) {
            let receiver = offset_u32(before.trim_end().len() - operator.len()).saturating_sub(1);
            let owner = facts
                .type_at_offset(file, receiver)
                .and_then(|typed| typed.entity);
            for (_, symbol) in facts.symbols() {
                if owner.is_some() && symbol.container == owner {
                    items.push(symbol_item(symbol));
                }
            }
        } else {
            for (_, symbol) in facts.symbols() {
                let visible = !symbol.local
                    || (symbol.file == file
                        && symbol.selection_end <= offset
                        && symbol
                            .container
                            .and_then(|container| facts.symbol(container))
                            .is_none_or(|container| {
                                container.start <= offset && offset <= container.end
                            }));
                if visible {
                    items.push(symbol_item(symbol));
                }
            }
            for group in &spec.keywords {
                for word in &group.words {
                    items.push(CompletionItem {
                        label: word.clone(),
                        kind: CompletionKind::Keyword,
                        detail: String::new(),
                        documentation: String::new(),
                        deprecated: false,
                    });
                }
            }
        }
        let lowered = prefix.to_lowercase();
        items
            .retain(|item| item.label.to_lowercase().starts_with(&lowered) && item.label != prefix);
        items.sort_by(|left, right| left.label.cmp(&right.label));
        items.dedup_by(|left, right| left.label == right.label && left.kind == right.kind);
        (items, replaced)
    }

    /// The call around `offset` and which argument the cursor is in.
    #[must_use]
    pub fn signature_help(&self, path: &Path, offset: u32) -> Option<SignatureHelp> {
        let text = self.text(path)?;
        let bytes = text.as_bytes().get(..offset as usize)?;
        let mut depth = 0usize;
        let mut commas = 0u32;
        let mut open = None;
        for (index, byte) in bytes.iter().enumerate().rev() {
            match byte {
                b')' | b']' | b'}' => depth += 1,
                b'(' if depth == 0 => {
                    open = Some(index);
                    break;
                }
                b'(' | b'[' | b'{' => depth = depth.checked_sub(1)?,
                b',' if depth == 0 => commas += 1,
                b'\n' if depth == 0 && bytes[..index].ends_with(b"\n") => return None,
                _ => {}
            }
        }
        let callee_end = bytes[..open?]
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())?
            + 1;
        let (_, entity) = self.entity_at(path, offset_u32(callee_end))?;
        let symbol = self.analysis.facts.symbol(entity)?;
        Some(SignatureHelp {
            label: if symbol.signature.is_empty() {
                symbol.name.clone()
            } else {
                symbol.signature.clone()
            },
            documentation: symbol.documentation.clone(),
            parameters: symbol.parameters.clone(),
            active: commas,
        })
    }

    #[must_use]
    pub fn inlay_hints(&self, path: &Path, start: u32, end: u32) -> Vec<Hint> {
        self.file(path)
            .map(|file| {
                self.analysis
                    .facts
                    .hints(file, start, end)
                    .into_iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Foldable ranges: the recorded folds, bracket pairs and comments that
    /// span lines, and region markers.
    #[must_use]
    pub fn folds(&self, path: &Path) -> Vec<(u32, u32, FoldKind)> {
        let spec = self.spec(path);
        let mut folds = self
            .file(path)
            .map(|file| self.analysis.facts.folds(file))
            .unwrap_or_default();
        let Some(text) = self.text(path) else {
            return folds;
        };
        let spans_lines = |start: u32, end: u32| {
            text.get(start as usize..end as usize)
                .is_some_and(|text| text.contains('\n'))
        };
        let mut open: Vec<(String, u32)> = Vec::new();
        for token in self.lexical_tokens(path) {
            let lexeme = text
                .get(token.start as usize..token.end as usize)
                .unwrap_or_default();
            if token.class == TokenClass::Comment || token.class == TokenClass::DocComment {
                if spans_lines(token.start, token.end) {
                    folds.push((token.start, token.end, FoldKind::Comment));
                }
                continue;
            }
            if token.class != TokenClass::Punctuation {
                continue;
            }
            if let Some(pair) = spec.brackets.iter().find(|pair| pair.open == lexeme) {
                open.push((pair.close.clone(), token.start));
            } else if let Some(index) = open.iter().rposition(|(close, _)| close == lexeme) {
                let (_, start) = open.remove(index);
                open.truncate(index);
                if spans_lines(start, token.end) {
                    folds.push((start, token.end, FoldKind::Other));
                }
            }
        }
        if let Some((start, end)) = &spec.region_markers
            && let (Ok(start), Ok(end)) = (
                klf_facts::pattern::Pattern::parse(start),
                klf_facts::pattern::Pattern::parse(end),
            )
        {
            let mut regions = Vec::new();
            let mut offset = 0;
            for line in text.split_inclusive('\n') {
                let trimmed = line.trim_start();
                let at = offset + (line.len() - trimmed.len());
                if start.match_at(&text, at).is_some() {
                    regions.push(offset_u32(offset));
                } else if end.match_at(&text, at).is_some()
                    && let Some(open) = regions.pop()
                {
                    folds.push((
                        open,
                        offset_u32(offset + line.trim_end().len()),
                        FoldKind::Region,
                    ));
                }
                offset += line.len();
            }
        }
        folds.sort_unstable_by_key(|(start, end, _)| (*start, std::cmp::Reverse(*end)));
        folds.dedup_by_key(|(start, end, _)| (*start, *end));
        folds
    }

    /// Selection ranges at `offset`, innermost first: the word, the recorded
    /// syntax nodes containing it, then the whole file.
    #[must_use]
    pub fn selection(&self, path: &Path, offset: u32) -> Vec<(u32, u32)> {
        let mut ranges: Vec<(u32, u32)> = self.word_at(path, offset).into_iter().collect();
        if let Some(file) = self.file(path) {
            let mut nodes = self.analysis.facts.enclosing_nodes(file, offset);
            nodes.sort_by_key(|(start, end)| end - start);
            ranges.extend(nodes);
        }
        if let Some(text) = self.text(path) {
            ranges.push((0, offset_u32(text.len())));
        }
        let mut chosen: Vec<(u32, u32)> = Vec::new();
        for range in ranges {
            let contains = chosen
                .last()
                .is_none_or(|last| range.0 <= last.0 && last.1 <= range.1 && range != *last);
            if contains {
                chosen.push(range);
            }
        }
        chosen
    }

    #[must_use]
    pub fn links(&self, path: &Path) -> Vec<(u32, u32, LinkDestination)> {
        let Some(file) = self.file(path) else {
            return Vec::new();
        };
        let directory = path.parent().unwrap_or(Path::new(""));
        self.analysis
            .facts
            .links(file)
            .iter()
            .filter_map(|link| {
                let destination = match &link.target {
                    LinkTarget::File(target) => LinkDestination::File(directory.join(target)),
                    LinkTarget::Url(url) => LinkDestination::Url(url.clone()),
                    LinkTarget::Entity(entity) => {
                        LinkDestination::Span(self.span(self.analysis.facts.definition(*entity)?)?)
                    }
                };
                Some((link.start, link.end, destination))
            })
            .collect()
    }

    #[must_use]
    pub fn runnables(&self, path: &Path) -> Vec<Runnable> {
        self.file(path)
            .map(|file| self.analysis.facts.runnables(file).to_vec())
            .unwrap_or_default()
    }
}

fn symbol_item(symbol: &klf_facts::Symbol) -> CompletionItem {
    CompletionItem {
        label: symbol.name.clone(),
        kind: CompletionKind::Symbol(symbol.kind()),
        detail: if symbol.signature.is_empty() {
            symbol.detail.clone()
        } else {
            symbol.signature.clone()
        },
        documentation: symbol.documentation.clone(),
        deprecated: symbol.deprecated,
    }
}
