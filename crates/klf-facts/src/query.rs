//! The questions editor features ask of the fact store (`KLF-EDITOR.md`
//! section 11).

use crate::store::{CallEdge, Entity, Facts, Hint, Link, Runnable, SpanText, Symbol, Token};
use crate::{FoldKind, ReferenceRole, TokenModifier};
use klf_core::FileId;

/// A span of one file.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Location {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

/// A symbol and the symbols it contains, for an outline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolTree {
    pub entity: Entity,
    pub children: Vec<SymbolTree>,
}

impl Facts {
    /// The file's tokens as a sorted, non-overlapping sequence (section 11.2):
    /// for one span the latest phase wins and within a phase the latest call;
    /// where spans overlap the innermost wins, splitting the outer one.
    #[must_use]
    pub fn tokens(&self, file: FileId) -> Vec<Token> {
        self.tokens_over(file, Vec::new())
    }

    /// [`Self::tokens`] painted over `base`, the tokens the editor spec's
    /// lexical classifier found. Any recorded token wins over a base token.
    #[must_use]
    pub fn tokens_over(&self, file: FileId, base: Vec<Token>) -> Vec<Token> {
        let mut tokens = base;
        let empty = crate::store::FileFacts::default();
        let facts = self.files.get(&file).unwrap_or(&empty);
        tokens.extend(facts.tokens.iter().copied());
        for &(start, end) in &facts.unresolved {
            if let Some(token) = tokens
                .iter_mut()
                .rev()
                .find(|token| token.start == start && token.end == end)
            {
                token.modifiers |= TokenModifier::Unresolved.bit();
            }
        }
        for &(start, end) in &facts.inactive {
            for token in &mut tokens {
                if token.start >= start && token.end <= end {
                    token.modifiers |= TokenModifier::Inactive.bit();
                }
            }
        }
        if tokens.is_empty() {
            return tokens;
        }
        let low = tokens.iter().map(|token| token.start).min().unwrap_or(0);
        let high = tokens.iter().map(|token| token.end).max().unwrap_or(0);
        // Paint widest first so narrower tokens land on top; among equal
        // widths the later phase and the later call paint last.
        let mut order: Vec<usize> = (0..tokens.len()).collect();
        order.sort_by_key(|&index| {
            let token = &tokens[index];
            (
                std::cmp::Reverse(token.end - token.start),
                token.phase,
                token.sequence,
            )
        });
        let mut canvas: Vec<Option<u32>> = vec![None; (high - low) as usize];
        for index in order {
            let token = &tokens[index];
            let painted = u32::try_from(index).unwrap_or(u32::MAX);
            for slot in &mut canvas[(token.start - low) as usize..(token.end - low) as usize] {
                *slot = Some(painted);
            }
        }
        let mut resolved = Vec::new();
        let mut cursor = 0usize;
        while cursor < canvas.len() {
            let Some(index) = canvas[cursor] else {
                cursor += 1;
                continue;
            };
            let mut end = cursor + 1;
            while end < canvas.len() && canvas[end] == Some(index) {
                end += 1;
            }
            let token = tokens[index as usize];
            resolved.push(Token {
                start: low + u32::try_from(cursor).unwrap_or(0),
                end: low + u32::try_from(end).unwrap_or(0),
                ..token
            });
            cursor = end;
        }
        resolved
    }

    #[must_use]
    pub fn symbol(&self, entity: Entity) -> Option<&Symbol> {
        self.symbols.get(&entity)
    }

    /// Every declared symbol, in declaration order.
    pub fn symbols(&self) -> impl Iterator<Item = (Entity, &Symbol)> + '_ {
        self.order
            .iter()
            .filter_map(|entity| self.symbols.get(entity).map(|symbol| (*entity, symbol)))
    }

    /// The entity named at `offset`: the narrowest reference containing it, or
    /// the name of a declaration (section 11.3). An offset at a span's end
    /// counts, so a cursor just after a name finds it.
    #[must_use]
    pub fn entity_at(&self, file: FileId, offset: u32) -> Option<(Location, Entity)> {
        let mut best: Option<(Location, Entity)> = None;
        let mut consider = |start: u32, end: u32, entity: Entity| {
            if start <= offset && offset <= end {
                let narrower =
                    best.is_none_or(|(location, _)| end - start <= location.end - location.start);
                if narrower {
                    best = Some((Location { file, start, end }, entity));
                }
            }
        };
        for (entity, symbol) in self.symbols() {
            if symbol.file == Some(file) {
                consider(symbol.selection_start, symbol.selection_end, entity);
            }
        }
        if let Some(facts) = self.files.get(&file) {
            for declaration in &facts.declarations {
                consider(declaration.3, declaration.4, declaration.0);
            }
            for reference in &facts.references {
                consider(reference.start, reference.end, reference.entity);
            }
        }
        best
    }

    /// Where an entity is defined: its symbol's name.
    #[must_use]
    pub fn definition(&self, entity: Entity) -> Option<Location> {
        let symbol = self.symbols.get(&entity)?;
        Some(Location {
            file: symbol.file?,
            start: symbol.selection_start,
            end: symbol.selection_end,
        })
    }

    /// The non-defining declarations of an entity, or its definition.
    #[must_use]
    pub fn declarations(&self, entity: Entity) -> Vec<Location> {
        let mut found: Vec<Location> = self
            .files
            .iter()
            .flat_map(|(file, facts)| {
                facts
                    .declarations
                    .iter()
                    .filter(move |declaration| declaration.0 == entity)
                    .map(move |declaration| Location {
                        file: *file,
                        start: declaration.3,
                        end: declaration.4,
                    })
            })
            .collect();
        if found.is_empty() {
            found.extend(self.definition(entity));
        }
        found
    }

    /// Every reference to an entity, in file and source order.
    #[must_use]
    pub fn references_to(&self, entity: Entity) -> Vec<(Location, ReferenceRole)> {
        let mut found = Vec::new();
        for (file, facts) in &self.files {
            for reference in &facts.references {
                if reference.entity == entity {
                    found.push((
                        Location {
                            file: *file,
                            start: reference.start,
                            end: reference.end,
                        },
                        reference.role,
                    ));
                }
            }
        }
        found.sort_by_key(|(location, _)| (location.file, location.start));
        found.dedup_by_key(|(location, _)| *location);
        found
    }

    /// The outline of a file (section 11.8): its non-local symbols nested by
    /// container, in source order.
    #[must_use]
    pub fn document_symbols(&self, file: FileId) -> Vec<SymbolTree> {
        fn build(
            parent: Option<Entity>,
            entities: &[(Entity, &Symbol)],
            present: &std::collections::HashSet<Entity>,
        ) -> Vec<SymbolTree> {
            entities
                .iter()
                .filter(|(_, symbol)| {
                    let container = symbol
                        .container
                        .filter(|container| present.contains(container));
                    container == parent
                })
                .map(|(entity, _)| SymbolTree {
                    entity: *entity,
                    children: build(Some(*entity), entities, present),
                })
                .collect()
        }
        let mut entities: Vec<(Entity, &Symbol)> = self
            .symbols()
            .filter(|(_, symbol)| symbol.file == Some(file) && !symbol.local)
            .collect();
        entities.sort_by_key(|(_, symbol)| (symbol.start, std::cmp::Reverse(symbol.end)));
        let present: std::collections::HashSet<Entity> =
            entities.iter().map(|(entity, _)| *entity).collect();
        build(None, &entities, &present)
    }

    /// The narrowest recorded type containing `offset`.
    #[must_use]
    pub fn type_at_offset(&self, file: FileId, offset: u32) -> Option<&SpanText> {
        narrowest(self.files.get(&file)?.types.iter(), offset)
    }

    /// The innermost recorded scope containing `offset`.
    #[must_use]
    pub fn scope_at(&self, file: FileId, offset: u32) -> Option<u64> {
        self.files
            .get(&file)?
            .scopes
            .iter()
            .filter(|(start, end, _)| *start <= offset && offset <= *end)
            .min_by_key(|(start, end, _)| end - start)
            .map(|(_, _, scope)| *scope)
    }

    /// The narrowest recorded value containing `offset`.
    #[must_use]
    pub fn value_at_offset(&self, file: FileId, offset: u32) -> Option<&SpanText> {
        narrowest(self.files.get(&file)?.values.iter(), offset)
    }

    /// Recorded folds of a file.
    #[must_use]
    pub fn folds(&self, file: FileId) -> Vec<(u32, u32, FoldKind)> {
        let Some(facts) = self.files.get(&file) else {
            return Vec::new();
        };
        let mut folds = facts.folds.clone();
        folds.extend(
            facts
                .inactive
                .iter()
                .map(|(start, end)| (*start, *end, FoldKind::Region)),
        );
        folds
    }

    /// The syntax nodes containing `offset`, innermost first (section 11.14).
    #[must_use]
    pub fn enclosing_nodes(&self, file: FileId, offset: u32) -> Vec<(u32, u32)> {
        let Some(facts) = self.files.get(&file) else {
            return Vec::new();
        };
        let mut nodes: Vec<(u32, u32)> = facts
            .nodes
            .iter()
            .copied()
            .filter(|(start, end)| *start <= offset && offset <= *end)
            .collect();
        nodes.sort_by_key(|(start, end)| (end - start, std::cmp::Reverse(*start)));
        nodes.dedup();
        nodes
    }

    /// Hints whose position lies in `start..=end`.
    #[must_use]
    pub fn hints(&self, file: FileId, start: u32, end: u32) -> Vec<&Hint> {
        self.files.get(&file).map_or_else(Vec::new, |facts| {
            facts
                .hints
                .iter()
                .filter(|hint| hint.at >= start && hint.at <= end)
                .collect()
        })
    }

    #[must_use]
    pub fn links(&self, file: FileId) -> &[Link] {
        self.files
            .get(&file)
            .map_or(&[], |facts| facts.links.as_slice())
    }

    #[must_use]
    pub fn runnables(&self, file: FileId) -> &[Runnable] {
        self.files
            .get(&file)
            .map_or(&[], |facts| facts.runnables.as_slice())
    }

    /// Entities recorded as subtypes or implementations of `entity`.
    #[must_use]
    pub fn implementations(&self, entity: Entity) -> Vec<Entity> {
        let mut found: Vec<Entity> = self
            .supertypes
            .iter()
            .filter(|(_, supertype)| *supertype == entity)
            .map(|(subtype, _)| *subtype)
            .collect();
        found.dedup();
        found
    }

    /// Entities recorded as supertypes of `entity`.
    #[must_use]
    pub fn supertypes(&self, entity: Entity) -> Vec<Entity> {
        self.supertypes
            .iter()
            .filter(|(subtype, _)| *subtype == entity)
            .map(|(_, supertype)| *supertype)
            .collect()
    }

    /// Calls made to `entity`.
    pub fn incoming_calls(&self, entity: Entity) -> impl Iterator<Item = &CallEdge> + '_ {
        self.calls.iter().filter(move |call| call.callee == entity)
    }

    /// Calls `entity` makes.
    pub fn outgoing_calls(&self, entity: Entity) -> impl Iterator<Item = &CallEdge> + '_ {
        self.calls.iter().filter(move |call| call.caller == entity)
    }

    /// Workspace symbols matching `query` (section 11.9): its characters in
    /// order, case-insensitively, best matches first, at most `limit`.
    #[must_use]
    pub fn workspace_symbols(&self, query: &str, limit: usize) -> Vec<Entity> {
        let mut scored: Vec<(u32, usize, Entity)> = self
            .symbols()
            .enumerate()
            .filter(|(_, (_, symbol))| !symbol.local)
            .filter_map(|(order, (entity, symbol))| {
                fuzzy_score(query, &symbol.name).map(|score| {
                    let bonus = u32::from(!symbol.exported);
                    (score * 2 + bonus, order, entity)
                })
            })
            .collect();
        scored.sort_unstable();
        scored
            .into_iter()
            .take(limit)
            .map(|(_, _, entity)| entity)
            .collect()
    }
}

fn narrowest<'a>(items: impl Iterator<Item = &'a SpanText>, offset: u32) -> Option<&'a SpanText> {
    items
        .filter(|item| item.start <= offset && offset <= item.end)
        .min_by_key(|item| item.end - item.start)
}

/// Lower is better; `None` when `query` is not a subsequence of `name`.
/// Matches at word starts (after `_`, at a case change, at the start) cost
/// nothing; others cost one, and the distance skipped costs one per gap.
fn fuzzy_score(query: &str, name: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }
    let name: Vec<char> = name.chars().collect();
    let mut score = 0u32;
    let mut cursor = 0usize;
    for wanted in query.chars() {
        let wanted = wanted.to_ascii_lowercase();
        let found =
            (cursor..name.len()).find(|&index| name[index].to_ascii_lowercase() == wanted)?;
        let word_start = found == 0
            || name[found - 1] == '_'
            || (name[found].is_uppercase() && name[found - 1].is_lowercase());
        if !word_start {
            score += 1;
        }
        if found > cursor {
            score += 1;
        }
        cursor = found + 1;
    }
    Some(score)
}
