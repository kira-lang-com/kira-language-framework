//! Navigation over facts (`KLF-EDITOR.md` sections 11.3, 11.4, 11.8, 11.9,
//! 11.11 and 11.16).

use crate::snapshot::{FileSpan, Snapshot};
use klf_facts::{Entity, Location, ReferenceRole, Symbol, SymbolKind, SymbolTree};
use std::path::Path;

/// A symbol shown in an outline, a hierarchy or a workspace search.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolItem {
    pub entity: Entity,
    pub name: String,
    pub kind: SymbolKind,
    pub detail: String,
    pub deprecated: bool,
    pub span: FileSpan,
    pub selection: FileSpan,
    pub container: Option<String>,
    pub children: Vec<SymbolItem>,
}

/// One edit of a rename.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenameEdit {
    pub span: FileSpan,
    pub text: String,
}

/// Whether a read or a write, for document highlights.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    Text,
    Read,
    Write,
}

impl Snapshot {
    /// The entity the cursor names and the span naming it.
    #[must_use]
    pub fn entity_at(&self, path: &Path, offset: u32) -> Option<(Location, Entity)> {
        self.analysis.facts.entity_at(self.file(path)?, offset)
    }

    #[must_use]
    pub fn definition(&self, path: &Path, offset: u32) -> Vec<FileSpan> {
        self.entity_at(path, offset)
            .and_then(|(_, entity)| self.analysis.facts.definition(entity))
            .and_then(|location| self.span(location))
            .into_iter()
            .collect()
    }

    #[must_use]
    pub fn declaration(&self, path: &Path, offset: u32) -> Vec<FileSpan> {
        let Some((_, entity)) = self.entity_at(path, offset) else {
            return Vec::new();
        };
        self.analysis
            .facts
            .declarations(entity)
            .into_iter()
            .filter_map(|location| self.span(location))
            .collect()
    }

    /// Where the type of the expression at the cursor is defined.
    #[must_use]
    pub fn type_definition(&self, path: &Path, offset: u32) -> Vec<FileSpan> {
        self.file(path)
            .and_then(|file| self.analysis.facts.type_at_offset(file, offset))
            .and_then(|typed| typed.entity)
            .and_then(|entity| self.analysis.facts.definition(entity))
            .and_then(|location| self.span(location))
            .into_iter()
            .collect()
    }

    #[must_use]
    pub fn implementations(&self, path: &Path, offset: u32) -> Vec<FileSpan> {
        let Some((_, entity)) = self.entity_at(path, offset) else {
            return Vec::new();
        };
        self.analysis
            .facts
            .implementations(entity)
            .into_iter()
            .filter_map(|entity| self.analysis.facts.definition(entity))
            .filter_map(|location| self.span(location))
            .collect()
    }

    /// Every reference to the entity at the cursor, and its definition when
    /// `declaration` is set.
    #[must_use]
    pub fn references(&self, path: &Path, offset: u32, declaration: bool) -> Vec<FileSpan> {
        let Some((_, entity)) = self.entity_at(path, offset) else {
            return Vec::new();
        };
        let facts = &self.analysis.facts;
        let mut spans: Vec<FileSpan> = Vec::new();
        if declaration {
            spans.extend(
                facts
                    .declarations(entity)
                    .into_iter()
                    .filter_map(|location| self.span(location)),
            );
            spans.extend(
                facts
                    .definition(entity)
                    .and_then(|location| self.span(location)),
            );
        }
        spans.extend(
            facts
                .references_to(entity)
                .into_iter()
                .filter_map(|(location, _)| self.span(location)),
        );
        spans.sort_by(|left, right| (&left.path, left.start).cmp(&(&right.path, right.start)));
        spans.dedup();
        spans
    }

    /// The occurrences of the entity at the cursor in this file.
    #[must_use]
    pub fn highlights(&self, path: &Path, offset: u32) -> Vec<(FileSpan, Access)> {
        let Some((_, entity)) = self.entity_at(path, offset) else {
            return Vec::new();
        };
        let facts = &self.analysis.facts;
        let mut found: Vec<(FileSpan, Access)> = facts
            .definition(entity)
            .and_then(|location| self.span(location))
            .map(|span| (span, Access::Write))
            .into_iter()
            .collect();
        for (location, role) in facts.references_to(entity) {
            let access = match role {
                ReferenceRole::Write | ReferenceRole::ReadWrite => Access::Write,
                ReferenceRole::Read | ReferenceRole::Call => Access::Read,
                _ => Access::Text,
            };
            found.extend(self.span(location).map(|span| (span, access)));
        }
        found.retain(|(span, _)| span.path == path);
        found
    }

    /// The span a rename at the cursor replaces and the current name, when
    /// the cursor names a symbol declared in this project.
    #[must_use]
    pub fn prepare_rename(&self, path: &Path, offset: u32) -> Option<(FileSpan, String)> {
        let (location, entity) = self.entity_at(path, offset)?;
        let symbol = self.analysis.facts.symbol(entity)?;
        symbol.file?;
        Some((self.span(location)?, symbol.name.clone()))
    }

    #[must_use]
    pub fn rename(&self, path: &Path, offset: u32, name: &str) -> Vec<RenameEdit> {
        if self.prepare_rename(path, offset).is_none() {
            return Vec::new();
        }
        self.references(path, offset, true)
            .into_iter()
            .map(|span| RenameEdit {
                span,
                text: name.to_owned(),
            })
            .collect()
    }

    #[must_use]
    pub fn document_symbols(&self, path: &Path) -> Vec<SymbolItem> {
        let Some(file) = self.file(path) else {
            return Vec::new();
        };
        self.analysis
            .facts
            .document_symbols(file)
            .iter()
            .filter_map(|tree| self.tree_item(tree))
            .collect()
    }

    fn tree_item(&self, tree: &SymbolTree) -> Option<SymbolItem> {
        let mut item = self.symbol_item(tree.entity)?;
        item.children = tree
            .children
            .iter()
            .filter_map(|child| self.tree_item(child))
            .collect();
        Some(item)
    }

    #[must_use]
    pub fn symbol_item(&self, entity: Entity) -> Option<SymbolItem> {
        let facts = &self.analysis.facts;
        let symbol: &Symbol = facts.symbol(entity)?;
        let path = self.path(symbol.file?)?.clone();
        let detail = if symbol.detail.is_empty() {
            symbol.signature.clone()
        } else {
            symbol.detail.clone()
        };
        Some(SymbolItem {
            entity,
            name: symbol.name.clone(),
            kind: symbol.kind(),
            detail,
            deprecated: symbol.deprecated,
            span: FileSpan {
                path: path.clone(),
                start: symbol.start,
                end: symbol.end,
            },
            selection: FileSpan {
                path,
                start: symbol.selection_start,
                end: symbol.selection_end,
            },
            container: symbol
                .container
                .and_then(|container| facts.symbol(container))
                .map(|container| container.name.clone()),
            children: Vec::new(),
        })
    }

    #[must_use]
    pub fn workspace_symbols(&self, query: &str, limit: usize) -> Vec<SymbolItem> {
        self.analysis
            .facts
            .workspace_symbols(query, limit)
            .into_iter()
            .filter_map(|entity| self.symbol_item(entity))
            .collect()
    }

    /// Callers of `entity`, each with the spans of its calls.
    #[must_use]
    pub fn incoming_calls(&self, entity: Entity) -> Vec<(SymbolItem, Vec<FileSpan>)> {
        let edges: Vec<_> = self
            .analysis
            .facts
            .incoming_calls(entity)
            .map(|edge| {
                (
                    edge.caller,
                    Location {
                        file: edge.file,
                        start: edge.start,
                        end: edge.end,
                    },
                )
            })
            .collect();
        self.group_calls(edges)
    }

    /// Callees of `entity`, each with the spans of the calls to it.
    #[must_use]
    pub fn outgoing_calls(&self, entity: Entity) -> Vec<(SymbolItem, Vec<FileSpan>)> {
        let edges: Vec<_> = self
            .analysis
            .facts
            .outgoing_calls(entity)
            .map(|edge| {
                (
                    edge.callee,
                    Location {
                        file: edge.file,
                        start: edge.start,
                        end: edge.end,
                    },
                )
            })
            .collect();
        self.group_calls(edges)
    }

    fn group_calls(&self, edges: Vec<(Entity, Location)>) -> Vec<(SymbolItem, Vec<FileSpan>)> {
        let mut grouped: Vec<(Entity, Vec<FileSpan>)> = Vec::new();
        for (entity, location) in edges {
            let Some(span) = self.span(location) else {
                continue;
            };
            match grouped.iter_mut().find(|(known, _)| *known == entity) {
                Some((_, spans)) => spans.push(span),
                None => grouped.push((entity, vec![span])),
            }
        }
        grouped
            .into_iter()
            .filter_map(|(entity, spans)| Some((self.symbol_item(entity)?, spans)))
            .collect()
    }

    #[must_use]
    pub fn supertypes(&self, entity: Entity) -> Vec<SymbolItem> {
        self.analysis
            .facts
            .supertypes(entity)
            .into_iter()
            .filter_map(|entity| self.symbol_item(entity))
            .collect()
    }

    #[must_use]
    pub fn subtypes(&self, entity: Entity) -> Vec<SymbolItem> {
        self.analysis
            .facts
            .implementations(entity)
            .into_iter()
            .filter_map(|entity| self.symbol_item(entity))
            .collect()
    }
}
