use crate::Session;
use crate::definition::{DefinitionArtifact, DefinitionUnit};
use klf_core::FileId;
use std::collections::{HashMap, HashSet, VecDeque};

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
    context: String,
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
    pub context: String,
    pub dependencies: Vec<QueryDependency>,
    pub valid: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueryStats {
    pub executions: u64,
    pub cache_hits: u64,
    pub invalidated: u64,
    pub invalidation_edges_visited: u64,
}

#[derive(Clone)]
pub(crate) struct QueryStore {
    cache: HashMap<QueryKey, crate::session::ArtifactId>,
    records: Vec<QueryRecord>,
    ids: HashMap<QueryKey, QueryId>,
    active: Vec<QueryId>,
    file_dependents: HashMap<FileId, HashSet<QueryId>>,
    query_dependents: HashMap<QueryId, HashSet<QueryId>>,
    query_edges: HashSet<(QueryId, QueryId)>,
    stats: QueryStats,
}

impl QueryStore {
    pub(crate) fn new() -> Self {
        Self {
            cache: HashMap::new(),
            records: Vec::new(),
            ids: HashMap::new(),
            active: Vec::new(),
            file_dependents: HashMap::new(),
            query_dependents: HashMap::new(),
            query_edges: HashSet::new(),
            stats: QueryStats::default(),
        }
    }

    pub(crate) fn active_query(&self) -> Option<QueryId> {
        self.active.last().copied()
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
        self.begin_query_with_files(language, unit, phase, "", unit.files())
    }

    /// Starts a derived query whose inputs are the queries it reads.
    pub fn begin_derived_query(
        &mut self,
        language: impl Into<String>,
        unit: &DefinitionUnit,
        phase: QueryPhase,
    ) -> QueryId {
        self.begin_derived_query_with_context(language, unit, phase, "")
    }

    /// Starts a derived query distinguished by a stable caller context, such
    /// as a module name used by code generation.
    pub fn begin_derived_query_with_context(
        &mut self,
        language: impl Into<String>,
        unit: &DefinitionUnit,
        phase: QueryPhase,
        context: impl Into<String>,
    ) -> QueryId {
        self.begin_query_with_files(language, unit, phase, context, &[])
    }

    fn begin_query_with_files(
        &mut self,
        language: impl Into<String>,
        unit: &DefinitionUnit,
        phase: QueryPhase,
        context: impl Into<String>,
        files: &[FileId],
    ) -> QueryId {
        let language = language.into();
        let context = context.into();
        let key = QueryKey {
            language: language.clone(),
            unit: unit.clone(),
            phase,
            context: context.clone(),
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
                context,
                dependencies: Vec::new(),
                valid: true,
            });
            id
        };
        self.reset_query_diagnostics(id);
        let old_dependencies = self
            .queries
            .records
            .get(id.index())
            .unwrap_or_else(|| panic!("invalid query {}", id.as_u32()));
        for dependency in old_dependencies.dependencies.clone() {
            match dependency {
                QueryDependency::File(file) => {
                    remove_index_entry(&mut self.queries.file_dependents, file, id);
                }
                QueryDependency::Query(dependency) => {
                    self.queries.query_edges.remove(&(id, dependency));
                    remove_index_entry(&mut self.queries.query_dependents, dependency, id);
                }
            }
        }
        let dependencies = files.iter().copied().map(QueryDependency::File).collect();
        let record = self
            .queries
            .records
            .get_mut(id.index())
            .unwrap_or_else(|| panic!("invalid query {}", id.as_u32()));
        record.dependencies = dependencies;
        record.valid = true;
        for file in files {
            self.queries
                .file_dependents
                .entry(*file)
                .or_default()
                .insert(id);
        }
        self.queries.stats.executions += 1;
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
            context: String::new(),
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
        if parent == dependency || self.queries.query_edges.contains(&(parent, dependency)) {
            return false;
        }
        let Some(record) = self.queries.records.get_mut(parent.index()) else {
            return false;
        };
        record.dependencies.push(QueryDependency::Query(dependency));
        self.queries.query_edges.insert((parent, dependency));
        self.queries
            .query_dependents
            .entry(dependency)
            .or_default()
            .insert(parent);
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
    pub fn query_stats(&self) -> QueryStats {
        self.queries.stats
    }

    #[must_use]
    pub fn cached_artifact(
        &mut self,
        language: impl Into<String>,
        unit: &DefinitionUnit,
        phase: QueryPhase,
    ) -> Option<DefinitionArtifact> {
        self.cached_artifact_with_context(language, unit, phase, "")
    }

    #[must_use]
    pub fn cached_artifact_with_context(
        &mut self,
        language: impl Into<String>,
        unit: &DefinitionUnit,
        phase: QueryPhase,
        context: impl Into<String>,
    ) -> Option<DefinitionArtifact> {
        let key = QueryKey {
            language: language.into(),
            unit: unit.clone(),
            phase,
            context: context.into(),
        };
        let id = *self.queries.cache.get(&key)?;
        if let Some(query_id) = self.queries.ids.get(&key).copied() {
            if !self
                .query_record(query_id)
                .is_some_and(|record| record.valid)
            {
                return None;
            }
            self.queries.stats.cache_hits += 1;
            self.record_query_dependency_id_from_active(query_id);
            self.replay_query_diagnostics(query_id);
        }
        Some(DefinitionArtifact::from_id(self, unit.clone(), id))
    }

    pub fn cache_artifact(
        &mut self,
        language: impl Into<String>,
        phase: QueryPhase,
        artifact: &DefinitionArtifact,
    ) {
        self.cache_artifact_with_context(language, phase, "", artifact);
    }

    pub fn cache_artifact_with_context(
        &mut self,
        language: impl Into<String>,
        phase: QueryPhase,
        context: impl Into<String>,
        artifact: &DefinitionArtifact,
    ) {
        self.queries.cache.insert(
            QueryKey {
                language: language.into(),
                unit: artifact.unit(),
                phase,
                context: context.into(),
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
    pub fn invalidate_file(&mut self, file: FileId) -> Vec<QueryId> {
        let mut invalid = HashSet::new();
        let mut pending = self
            .queries
            .file_dependents
            .get(&file)
            .into_iter()
            .flatten()
            .copied()
            .collect::<VecDeque<_>>();
        while let Some(id) = pending.pop_front() {
            if !invalid.insert(id) {
                continue;
            }
            if let Some(dependents) = self.queries.query_dependents.get(&id) {
                self.queries.stats.invalidation_edges_visited += dependents.len() as u64;
                pending.extend(dependents.iter().copied());
            }
        }
        self.queries.stats.invalidated += invalid.len() as u64;
        let stale_keys = invalid
            .iter()
            .filter_map(|id| {
                let record = self.queries.records.get(id.index())?;
                Some(QueryKey {
                    language: record.language.clone(),
                    unit: record.unit.clone(),
                    phase: record.phase,
                    context: record.context.clone(),
                })
            })
            .collect::<Vec<_>>();
        let mut invalid_ids = invalid.into_iter().collect::<Vec<_>>();
        invalid_ids.sort_unstable();
        for id in &invalid_ids {
            if let Some(record) = self.queries.records.get_mut(id.index()) {
                record.valid = false;
            }
        }
        for key in stale_keys {
            self.queries.cache.remove(&key);
        }
        invalid_ids
    }
}

fn remove_index_entry<K: std::hash::Hash + Eq>(
    index: &mut HashMap<K, HashSet<QueryId>>,
    key: K,
    id: QueryId,
) {
    let remove_key = index.get_mut(&key).is_some_and(|ids| {
        ids.remove(&id);
        ids.is_empty()
    });
    if remove_key {
        index.remove(&key);
    }
}
