//! Lexical scopes, the entities declared in them and the name lookup over
//! both (section 8.4). The `Session` owns the tables; the queries that walk
//! them live here.

use super::Session;
use klf_core::Name;
use klf_source::Span;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScopeId(u32);

impl ScopeId {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    #[must_use]
    const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EntityId(u32);

impl EntityId {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    #[must_use]
    const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Namespace {
    Value,
    Type,
    Tag,
    Label,
    Macro,
    Custom(Name),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entity {
    pub payload: i64,
    pub kind: Name,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub(super) struct Scope {
    parent: Option<ScopeId>,
    kind: Name,
    declarations: HashMap<(Name, Namespace), Vec<EntityId>>,
    imports: Vec<ImportEdge>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ImportEdge {
    target: ScopeId,
    transitive: bool,
}

impl Session {
    /// Creates a lexical scope with an optional parent scope.
    ///
    /// # Panics
    /// Panics when `parent` does not identify a scope in this session.
    pub fn scope(&mut self, parent: Option<ScopeId>, kind: Name) -> ScopeId {
        if let Some(parent) = parent {
            self.scope_ref(parent);
        }
        let id = ScopeId(u32::try_from(self.scopes.len()).expect("scope count fits u32"));
        self.scopes.push(Scope {
            parent,
            kind,
            declarations: HashMap::new(),
            imports: Vec::new(),
        });
        id
    }

    #[must_use]
    pub fn scope_kind(&self, scope: ScopeId) -> &Name {
        &self.scope_ref(scope).kind
    }

    /// Allocates a session-owned entity that can be declared in one or more
    /// namespace scopes.
    ///
    /// # Panics
    /// Panics if more than `u32::MAX` entities are stored in this session.
    pub fn entity(&mut self, payload: i64, kind: Name, span: Span) -> EntityId {
        let id = EntityId(u32::try_from(self.entities.len()).expect("entity count fits u32"));
        self.entities.push(Entity {
            payload,
            kind,
            span,
        });
        id
    }

    #[must_use]
    pub fn entity_data(&self, entity: EntityId) -> Option<&Entity> {
        self.entities.get(entity.index())
    }

    /// Declares a name in one namespace of a scope.
    ///
    /// A scope has at most one declaration set for a name and namespace. The
    /// returned `false` lets a definition choose its redeclaration policy.
    ///
    /// # Panics
    /// Panics when `scope` or `entity` does not identify a handle in this
    /// session.
    pub fn declare(
        &mut self,
        scope: ScopeId,
        name: Name,
        namespace: Namespace,
        entity: EntityId,
    ) -> bool {
        assert!(self.entity_data(entity).is_some(), "invalid entity");
        let declarations = &mut self.scope_mut(scope).declarations;
        if declarations.contains_key(&(name.clone(), namespace.clone())) {
            return false;
        }
        declarations.insert((name, namespace), vec![entity]);
        true
    }

    /// Returns declarations in one scope without consulting parents/imports.
    #[must_use]
    pub fn lookup_local(
        &self,
        scope: ScopeId,
        name: &Name,
        namespace: &Namespace,
    ) -> Vec<EntityId> {
        self.scope_ref(scope)
            .declarations
            .get(&(name.clone(), namespace.clone()))
            .cloned()
            .unwrap_or_default()
    }

    /// Adds an import edge to a scope. A transitive edge also exposes the
    /// target scope's own imports during lookup.
    pub fn import_edge(&mut self, from: ScopeId, to: ScopeId, transitive: bool) {
        self.scope_ref(from);
        self.scope_ref(to);
        self.scope_mut(from).imports.push(ImportEdge {
            target: to,
            transitive,
        });
    }

    /// Looks up the nearest matching declaration through local, parent and
    /// imported scopes. All declarations at the winning scope are returned.
    #[must_use]
    pub fn lookup(&self, from: ScopeId, name: &Name, namespace: &Namespace) -> Vec<EntityId> {
        self.scope_ref(from);
        let mut visiting = HashSet::new();
        self.lookup_scope(from, name, namespace, true, &mut visiting)
    }

    fn lookup_scope(
        &self,
        scope: ScopeId,
        name: &Name,
        namespace: &Namespace,
        follow_edges: bool,
        visiting: &mut HashSet<ScopeId>,
    ) -> Vec<EntityId> {
        if !visiting.insert(scope) {
            return Vec::new();
        }
        let current = self.scope_ref(scope);
        if let Some(entities) = current.declarations.get(&(name.clone(), namespace.clone())) {
            visiting.remove(&scope);
            return entities.clone();
        }
        if follow_edges {
            let mut imported = Vec::new();
            for import in &current.imports {
                let entities =
                    self.lookup_scope(import.target, name, namespace, import.transitive, visiting);
                for entity in entities {
                    if !imported.contains(&entity) {
                        imported.push(entity);
                    }
                }
            }
            if !imported.is_empty() {
                visiting.remove(&scope);
                return imported;
            }
        }
        let parent = current.parent;
        visiting.remove(&scope);
        parent.map_or_else(Vec::new, |parent| {
            self.lookup_scope(parent, name, namespace, true, visiting)
        })
    }

    fn scope_ref(&self, scope: ScopeId) -> &Scope {
        self.scopes
            .get(scope.index())
            .unwrap_or_else(|| panic!("invalid scope {}", scope.as_u32()))
    }

    fn scope_mut(&mut self, scope: ScopeId) -> &mut Scope {
        self.scopes
            .get_mut(scope.index())
            .unwrap_or_else(|| panic!("invalid scope {}", scope.as_u32()))
    }
}
