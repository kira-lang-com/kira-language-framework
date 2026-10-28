use crate::Session;
use crate::definition::{DefinitionArtifact, DefinitionUnit};
use klf_core::FileId;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum QueryPhase {
    Parse,
    Elaborate,
    Lower,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct QueryKey {
    language: String,
    unit: DefinitionUnit,
    phase: QueryPhase,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct QueryId(u32);

impl QueryId {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum QueryDependency {
    File(FileId),
    Query(QueryId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryRecord {
    pub id: QueryId,
    pub language: String,
    pub unit: DefinitionUnit,
    pub phase: QueryPhase,
    pub dependencies: Vec<QueryDependency>,
    pub valid: bool,
}

#[derive(Clone)]
pub(crate) struct QueryStore {
    cache: HashMap<QueryKey, crate::session::ArtifactId>,
    records: Vec<QueryRecord>,
    ids: HashMap<QueryKey, QueryId>,
    active: Vec<QueryId>,
}

impl QueryStore {
    pub(crate) fn new() -> Self {
        Self {
            cache: HashMap::new(),
            records: Vec::new(),
            ids: HashMap::new(),
            active: Vec::new(),
        }
    }
}

impl Session {
    /// Starts recording one language query and returns its stable session id.
    /// A query depends on every source file in its unit. If another query is
    /// active, the new query is also recorded as a dependency of its caller.
    ///
    /// # Panics
    /// Panics if the session has more than `u32::MAX` query records.
    pub fn begin_query(
        &mut self,
        language: impl Into<String>,
        unit: &DefinitionUnit,
        phase: QueryPhase,
    ) -> QueryId {
        let language = language.into();
        let key = QueryKey {
            language: language.clone(),
            unit: unit.clone(),
            phase,
        };
        let id = if let Some(id) = self.queries.ids.get(&key).copied() {
            id
        } else {
            let id =
                QueryId(u32::try_from(self.queries.records.len()).expect("query count fits u32"));
            self.queries.ids.insert(key, id);
            self.queries.records.push(QueryRecord {
                id,
                language,
                unit: unit.clone(),
                phase,
                dependencies: Vec::new(),
                valid: true,
            });
            id
        };
        let record = self
            .queries
            .records
            .get_mut(id.index())
            .unwrap_or_else(|| panic!("invalid query {}", id.as_u32()));
        record.dependencies = unit
            .files()
            .iter()
            .copied()
            .map(QueryDependency::File)
            .collect();
        record.valid = true;
        if let Some(parent) = self.queries.active.last().copied()
            && parent != id
        {
            self.record_query_dependency_id_from(parent, id);
        }
        self.queries.active.push(id);
        id
    }

    /// Finishes the innermost active query.
    ///
    /// # Panics
    /// Panics if `id` is not the innermost active query.
    pub fn finish_query(&mut self, id: QueryId) {
        let active = self
            .queries
            .active
            .pop()
            .unwrap_or_else(|| panic!("query {} is not active", id.as_u32()));
        assert_eq!(active, id, "queries must finish in stack order");
    }

    /// Records a dependency on another query from the innermost active query.
    /// Returns `false` when the dependency is not known or no query is active.
    pub fn record_query_dependency(
        &mut self,
        language: impl Into<String>,
        unit: &DefinitionUnit,
        phase: QueryPhase,
    ) -> bool {
        let key = QueryKey {
            language: language.into(),
            unit: unit.clone(),
            phase,
        };
        let Some(id) = self.queries.ids.get(&key).copied() else {
            return false;
        };
        self.record_query_dependency_id_from_active(id)
    }

    /// Records a dependency on a known query from the innermost active query.
    pub fn record_query_dependency_id(&mut self, dependency: QueryId) -> bool {
        self.record_query_dependency_id_from_active(dependency)
    }

    fn record_query_dependency_id_from(&mut self, parent: QueryId, dependency: QueryId) -> bool {
        if parent == dependency {
            return false;
        }
        let Some(record) = self.queries.records.get_mut(parent.index()) else {
            return false;
        };
        if !record
            .dependencies
            .contains(&QueryDependency::Query(dependency))
        {
            record.dependencies.push(QueryDependency::Query(dependency));
        }
        true
    }

    fn record_query_dependency_id_from_active(&mut self, dependency: QueryId) -> bool {
        let Some(parent) = self.queries.active.last().copied() else {
            return false;
        };
        self.record_query_dependency_id_from(parent, dependency)
    }

    #[must_use]
    pub fn query_record(&self, id: QueryId) -> Option<&QueryRecord> {
        self.queries.records.get(id.index())
    }

    #[must_use]
    pub fn query_records(&self) -> &[QueryRecord] {
        &self.queries.records
    }

    #[must_use]
    pub fn cached_artifact(
        &mut self,
        language: impl Into<String>,
        unit: &DefinitionUnit,
        phase: QueryPhase,
    ) -> Option<DefinitionArtifact> {
        let key = QueryKey {
            language: language.into(),
            unit: unit.clone(),
            phase,
        };
        let id = *self.queries.cache.get(&key)?;
        if let Some(query_id) = self.queries.ids.get(&key).copied() {
            if !self
                .query_record(query_id)
                .is_some_and(|record| record.valid)
            {
                return None;
            }
            self.record_query_dependency_id_from_active(query_id);
        }
        Some(DefinitionArtifact::from_id(self, unit.clone(), id))
    }

    pub fn cache_artifact(
        &mut self,
        language: impl Into<String>,
        phase: QueryPhase,
        artifact: &DefinitionArtifact,
    ) {
        self.queries.cache.insert(
            QueryKey {
                language: language.into(),
                unit: artifact.unit(),
                phase,
            },
            artifact.id(),
        );
    }

    pub fn invalidate_queries(&mut self) {
        for record in &mut self.queries.records {
            record.valid = false;
        }
        self.queries.cache.clear();
    }

    /// Invalidates every query that reads `file`, including queries that
    /// depend on those queries. Cached artifacts for the invalid set are
    /// removed while unrelated query results remain reusable.
    pub fn invalidate_file(&mut self, file: FileId) {
        let mut invalid = HashSet::new();
        for record in &mut self.queries.records {
            if record.dependencies.contains(&QueryDependency::File(file)) {
                record.valid = false;
                invalid.insert(record.id);
            }
        }
        loop {
            let mut changed = false;
            for record in &mut self.queries.records {
                if record.valid
                    && record.dependencies.iter().any(|dependency| {
                        matches!(dependency, QueryDependency::Query(id) if invalid.contains(id))
                    })
                {
                    record.valid = false;
                    changed = invalid.insert(record.id) || changed;
                }
            }
            if !changed {
                break;
            }
        }
        self.queries.cache.retain(|key, _| {
            self.queries
                .ids
                .get(key)
                .is_none_or(|id| !invalid.contains(id))
        });
    }
}
