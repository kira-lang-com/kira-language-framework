//! The fact store: what each phase recorded, per file.

use crate::{FactPhase, FoldKind, HintKind, ReferenceRole, RunnableKind, SymbolKind, TokenClass};
use klf_core::FileId;
use std::collections::{BTreeMap, HashMap};

/// What facts name: a kernel entity, or any key a definition uses
/// consistently for one declaration (`KLF-EDITOR.md` section 2).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Entity(pub u64);

/// One classified token or name occurrence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token {
    pub start: u32,
    pub end: u32,
    pub class: TokenClass,
    pub modifiers: u32,
    pub phase: FactPhase,
    /// Recording order across the whole store, so a later call wins.
    pub sequence: u32,
}

/// A span that names an entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reference {
    pub start: u32,
    pub end: u32,
    pub entity: Entity,
    pub role: ReferenceRole,
}

/// A declared symbol (section 6.4).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: Option<SymbolKind>,
    pub file: Option<FileId>,
    pub start: u32,
    pub end: u32,
    pub selection_start: u32,
    pub selection_end: u32,
    pub container: Option<Entity>,
    pub signature: String,
    pub documentation: String,
    pub detail: String,
    pub parameters: Vec<String>,
    pub deprecated: bool,
    pub exported: bool,
    pub local: bool,
}

impl Symbol {
    #[must_use]
    pub fn kind(&self) -> SymbolKind {
        self.kind.unwrap_or(SymbolKind::Variable)
    }
}

/// Text recorded for a span: a type or a compile-time value (section 6.6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpanText {
    pub start: u32,
    pub end: u32,
    pub text: String,
    pub entity: Option<Entity>,
}

/// An inlay hint (section 6.6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hint {
    pub at: u32,
    pub label: String,
    pub kind: HintKind,
    pub padding_left: bool,
    pub padding_right: bool,
    pub target: Option<Entity>,
    /// Text that makes the hint explicit when the user accepts it.
    pub insert: Option<String>,
}

/// Where a link goes (section 6.8).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkTarget {
    File(String),
    Url(String),
    Entity(Entity),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Link {
    pub start: u32,
    pub end: u32,
    pub target: LinkTarget,
}

/// Something an editor can run (section 6.8).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Runnable {
    pub label: String,
    pub kind: RunnableKind,
    pub start: u32,
    pub end: u32,
    pub arguments: Vec<String>,
}

/// Every fact recorded about one file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileFacts {
    pub tokens: Vec<Token>,
    pub references: Vec<Reference>,
    pub types: Vec<SpanText>,
    pub values: Vec<SpanText>,
    pub hints: Vec<Hint>,
    pub folds: Vec<(u32, u32, FoldKind)>,
    pub nodes: Vec<(u32, u32)>,
    pub links: Vec<Link>,
    pub runnables: Vec<Runnable>,
    pub inactive: Vec<(u32, u32)>,
    pub unresolved: Vec<(u32, u32)>,
    /// Non-defining declarations: entity, whole span, name span.
    pub declarations: Vec<(Entity, u32, u32, u32, u32)>,
    /// Lexical scopes: start, end, and the definition's scope key.
    pub scopes: Vec<(u32, u32, u64)>,
}

impl FileFacts {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A call from one entity to another, at a span of a file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallEdge {
    pub caller: Entity,
    pub callee: Entity,
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

/// All facts of an analysis.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Facts {
    pub(crate) files: BTreeMap<FileId, FileFacts>,
    pub(crate) symbols: HashMap<Entity, Symbol>,
    /// Entities in the order their symbols were declared.
    pub(crate) order: Vec<Entity>,
    pub(crate) calls: Vec<CallEdge>,
    pub(crate) supertypes: Vec<(Entity, Entity)>,
    pub(crate) phase: FactPhase,
    pub(crate) sequence: u32,
}

impl Default for Facts {
    fn default() -> Self {
        Self {
            files: BTreeMap::new(),
            symbols: HashMap::new(),
            order: Vec::new(),
            calls: Vec::new(),
            supertypes: Vec::new(),
            phase: FactPhase::Parse,
            sequence: 0,
        }
    }
}

impl Facts {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The phase whose facts are recorded next.
    pub fn set_phase(&mut self, phase: FactPhase) {
        self.phase = phase;
    }

    #[must_use]
    pub const fn phase(&self) -> FactPhase {
        self.phase
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.values().all(FileFacts::is_empty)
            && self.symbols.is_empty()
            && self.calls.is_empty()
            && self.supertypes.is_empty()
    }

    fn file_mut(&mut self, file: FileId) -> &mut FileFacts {
        self.files.entry(file).or_default()
    }

    #[must_use]
    pub fn file(&self, file: FileId) -> Option<&FileFacts> {
        self.files.get(&file)
    }

    /// The files any fact was recorded for.
    pub fn files(&self) -> impl Iterator<Item = FileId> + '_ {
        self.files.keys().copied()
    }

    pub fn classify(
        &mut self,
        file: FileId,
        start: u32,
        end: u32,
        class: TokenClass,
        modifiers: u32,
    ) {
        if end <= start {
            return;
        }
        let token = Token {
            start,
            end,
            class,
            modifiers,
            phase: self.phase,
            sequence: self.sequence,
        };
        self.sequence += 1;
        self.file_mut(file).tokens.push(token);
    }

    /// Records a symbol's defining occurrence. A second declaration of the
    /// same entity replaces the first, keeping its place in the order.
    pub fn declare_symbol(&mut self, entity: Entity, symbol: Symbol) {
        if self.symbols.insert(entity, symbol).is_none() {
            self.order.push(entity);
        }
    }

    /// Updates a declared symbol in place, for facts recorded after it.
    pub fn symbol_mut(&mut self, entity: Entity) -> Option<&mut Symbol> {
        self.symbols.get_mut(&entity)
    }

    pub fn declaration(
        &mut self,
        entity: Entity,
        file: FileId,
        span: (u32, u32),
        selection: (u32, u32),
    ) {
        self.file_mut(file)
            .declarations
            .push((entity, span.0, span.1, selection.0, selection.1));
    }

    pub fn reference(
        &mut self,
        file: FileId,
        start: u32,
        end: u32,
        entity: Entity,
        role: ReferenceRole,
    ) {
        self.file_mut(file).references.push(Reference {
            start,
            end,
            entity,
            role,
        });
    }

    pub fn type_at(
        &mut self,
        file: FileId,
        start: u32,
        end: u32,
        text: String,
        entity: Option<Entity>,
    ) {
        self.file_mut(file).types.push(SpanText {
            start,
            end,
            text,
            entity,
        });
    }

    pub fn value_at(&mut self, file: FileId, start: u32, end: u32, text: String) {
        self.file_mut(file).values.push(SpanText {
            start,
            end,
            text,
            entity: None,
        });
    }

    pub fn hint(&mut self, file: FileId, hint: Hint) {
        self.file_mut(file).hints.push(hint);
    }

    pub fn fold(&mut self, file: FileId, start: u32, end: u32, kind: FoldKind) {
        if end > start {
            self.file_mut(file).folds.push((start, end, kind));
        }
    }

    pub fn syntax_node(&mut self, file: FileId, start: u32, end: u32) {
        if end >= start {
            self.file_mut(file).nodes.push((start, end));
        }
    }

    pub fn link(&mut self, file: FileId, start: u32, end: u32, target: LinkTarget) {
        self.file_mut(file).links.push(Link { start, end, target });
    }

    pub fn runnable(&mut self, file: FileId, runnable: Runnable) {
        self.file_mut(file).runnables.push(runnable);
    }

    pub fn inactive(&mut self, file: FileId, start: u32, end: u32) {
        self.file_mut(file).inactive.push((start, end));
    }

    pub fn unresolved(&mut self, file: FileId, start: u32, end: u32) {
        self.file_mut(file).unresolved.push((start, end));
    }

    pub fn scope(&mut self, file: FileId, start: u32, end: u32, scope: u64) {
        if end >= start {
            self.file_mut(file).scopes.push((start, end, scope));
        }
    }

    pub fn call_edge(&mut self, edge: CallEdge) {
        self.calls.push(edge);
    }

    pub fn type_edge(&mut self, subtype: Entity, supertype: Entity) {
        self.supertypes.push((subtype, supertype));
    }

    /// Moves every fact of `other` into this store, after this store's own.
    pub fn absorb(&mut self, other: Self) {
        let offset = self.sequence;
        for (file, mut facts) in other.files {
            for token in &mut facts.tokens {
                token.sequence += offset;
            }
            let target = self.file_mut(file);
            target.tokens.append(&mut facts.tokens);
            target.references.append(&mut facts.references);
            target.types.append(&mut facts.types);
            target.values.append(&mut facts.values);
            target.hints.append(&mut facts.hints);
            target.folds.append(&mut facts.folds);
            target.nodes.append(&mut facts.nodes);
            target.links.append(&mut facts.links);
            target.runnables.append(&mut facts.runnables);
            target.inactive.append(&mut facts.inactive);
            target.unresolved.append(&mut facts.unresolved);
            target.declarations.append(&mut facts.declarations);
            target.scopes.append(&mut facts.scopes);
        }
        for entity in other.order {
            if let Some(symbol) = other.symbols.get(&entity) {
                self.declare_symbol(entity, symbol.clone());
            }
        }
        self.calls.extend(other.calls);
        self.supertypes.extend(other.supertypes);
        self.sequence += other.sequence;
    }
}
