//! Indexed package metadata over one source set and one compilation unit.

use crate::{DefinitionPackage, DefinitionSource, DefinitionUnit, Session, SessionHandle};
use klf_core::FileId;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PackageId(usize);

impl PackageId {
    #[must_use]
    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageNode {
    pub id: PackageId,
    pub name: Arc<str>,
    pub module_root: Arc<str>,
    pub dependencies: Vec<String>,
    pub sources: Range<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphError {
    DuplicatePackage(String),
    DuplicateModule {
        module: String,
        first_package: String,
    },
    DuplicateSource(String),
    UnknownPackage(String),
    UnknownSource(String),
    DependenciesNotResolved,
    MissingDependency {
        package: String,
        dependency: String,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicatePackage(name) => write!(formatter, "duplicate package `{name}`"),
            Self::DuplicateModule {
                module,
                first_package,
            } => write!(
                formatter,
                "module `{module}` is already owned by package `{first_package}`"
            ),
            Self::DuplicateSource(name) => write!(formatter, "duplicate source identity `{name}`"),
            Self::UnknownPackage(name) => write!(formatter, "unknown package `{name}`"),
            Self::UnknownSource(name) => write!(formatter, "unknown source identity `{name}`"),
            Self::DependenciesNotResolved => {
                formatter.write_str("package dependencies have not been resolved")
            }
            Self::MissingDependency {
                package,
                dependency,
            } => write!(
                formatter,
                "package `{package}` depends on missing package `{dependency}`"
            ),
        }
    }
}

impl std::error::Error for GraphError {}

/// Package identities, module roots, and dependency names indexed over one
/// persistent source package. Registering it yields one stable source unit,
/// regardless of how many logical packages contributed the files.
pub struct DefinitionGraph {
    packages: Vec<PackageNode>,
    package_ids: HashMap<Arc<str>, PackageId>,
    module_owners: HashMap<Arc<str>, PackageId>,
    source_indices: HashMap<Arc<str>, usize>,
    source_packages: Vec<PackageId>,
    source_package: DefinitionPackage,
    file_ids: Vec<Option<FileId>>,
    dirty_sources: Vec<usize>,
    dirty_index: HashSet<usize>,
    dependency_ranges: Vec<Range<usize>>,
    dependency_count: usize,
    resolved_dependencies: Option<Vec<PackageId>>,
    session: Option<SessionHandle>,
    unit: Option<DefinitionUnit>,
}

impl Default for DefinitionGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl DefinitionGraph {
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(0, 0)
    }

    /// Creates an empty graph with capacity for the indexed project inputs.
    #[must_use]
    pub fn with_capacity(packages: usize, sources: usize) -> Self {
        Self {
            packages: Vec::with_capacity(packages),
            package_ids: HashMap::with_capacity(packages),
            module_owners: HashMap::with_capacity(packages),
            source_indices: HashMap::with_capacity(sources),
            source_packages: Vec::with_capacity(sources),
            source_package: DefinitionPackage::with_capacity(sources),
            file_ids: Vec::with_capacity(sources),
            dirty_sources: Vec::with_capacity(sources),
            dirty_index: HashSet::with_capacity(sources),
            dependency_ranges: Vec::with_capacity(packages),
            dependency_count: 0,
            resolved_dependencies: None,
            session: None,
            unit: None,
        }
    }

    /// Adds one logical package and appends its sources to the shared graph.
    /// The package name namespaces every source identity.
    pub fn add_package(
        &mut self,
        name: impl Into<String>,
        module_root: impl Into<String>,
        dependencies: Vec<String>,
        package: &DefinitionPackage,
    ) -> Result<PackageId, GraphError> {
        let name: Arc<str> = Arc::from(name.into());
        let module_root: Arc<str> = Arc::from(module_root.into());
        if self.package_ids.contains_key(name.as_ref()) {
            return Err(GraphError::DuplicatePackage(name.to_string()));
        }
        if let Some(owner) = self.module_owners.get(module_root.as_ref()) {
            let first_package = self.packages[owner.index()].name.to_string();
            return Err(GraphError::DuplicateModule {
                module: module_root.to_string(),
                first_package,
            });
        }

        let source_count = package.sources().len();
        let mut identities = Vec::with_capacity(source_count);
        let mut local_identities = (source_count > 1).then(|| HashSet::with_capacity(source_count));
        for source in package.sources() {
            let identity = source_identity(&name, &source.name);
            if self.source_indices.contains_key(&identity)
                || local_identities
                    .as_mut()
                    .is_some_and(|identities| !identities.insert(identity.clone()))
            {
                return Err(GraphError::DuplicateSource(identity.to_string()));
            }
            identities.push(identity);
        }

        let id = PackageId(self.packages.len());
        let source_start = self.source_package.sources().len();
        let dependency_start = self.dependency_count;
        self.dependency_count += dependencies.len();
        for (source, identity) in package.sources().iter().zip(identities) {
            let index = self.source_package.sources().len();
            self.source_package.append_source(DefinitionSource::new(
                identity.clone(),
                Arc::clone(&source.text),
            ));
            self.source_indices.insert(identity.clone(), index);
            self.source_packages.push(id);
            self.file_ids.push(None);
            self.mark_source_dirty(index);
        }
        self.packages.push(PackageNode {
            id,
            name: Arc::clone(&name),
            module_root: Arc::clone(&module_root),
            dependencies,
            sources: source_start..self.source_package.sources().len(),
        });
        self.dependency_ranges
            .push(dependency_start..self.dependency_count);
        self.package_ids.insert(name, id);
        self.module_owners.insert(module_root, id);
        self.resolved_dependencies = None;
        self.unit = None;
        Ok(id)
    }

    /// Updates source contents while preserving its indexed identity and file
    /// ID. Returns `false` when the supplied text is unchanged.
    pub fn replace_source(
        &mut self,
        package: &str,
        source: &str,
        text: impl Into<Arc<str>>,
    ) -> Result<bool, GraphError> {
        let package_id = self
            .package_ids
            .get(package)
            .copied()
            .ok_or_else(|| GraphError::UnknownPackage(package.to_owned()))?;
        let identity = source_identity(package, source);
        let index = self
            .source_indices
            .get(&identity)
            .copied()
            .ok_or_else(|| GraphError::UnknownSource(identity.to_string()))?;
        if self.source_packages.get(index) != Some(&package_id) {
            return Err(GraphError::DuplicateSource(identity.to_string()));
        }
        let text = text.into();
        let current = &self.source_package.sources()[index].text;
        if Arc::ptr_eq(current, &text) || current.as_ref() == text.as_ref() {
            return Ok(false);
        }
        self.source_package.replace_source_text(index, text);
        self.mark_source_dirty(index);
        Ok(true)
    }

    #[must_use]
    pub fn package_id(&self, name: &str) -> Option<PackageId> {
        self.package_ids.get(name).copied()
    }

    #[must_use]
    pub fn module_owner(&self, module_root: &str) -> Option<PackageId> {
        self.module_owners.get(module_root).copied()
    }

    #[must_use]
    pub fn source_owner(&self, identity: &str) -> Option<PackageId> {
        self.source_indices
            .get(identity)
            .and_then(|index| self.source_packages.get(*index))
            .copied()
    }

    #[must_use]
    pub fn package(&self, id: PackageId) -> Option<&PackageNode> {
        self.packages.get(id.index())
    }

    #[must_use]
    pub fn packages(&self) -> &[PackageNode] {
        &self.packages
    }

    #[must_use]
    pub fn source_count(&self) -> usize {
        self.source_package.sources().len()
    }

    /// Resolves all dependency constraints through the package-name index once.
    /// Later dependency lookups read the flat persistent index.
    pub fn resolve_dependencies(&mut self) -> Result<(), GraphError> {
        if self.resolved_dependencies.is_some() {
            return Ok(());
        }
        let mut resolved = Vec::with_capacity(self.dependency_count);
        for package in &self.packages {
            for dependency in &package.dependencies {
                resolved.push(
                    self.package_ids
                        .get(dependency.as_str())
                        .copied()
                        .ok_or_else(|| GraphError::MissingDependency {
                            package: package.name.to_string(),
                            dependency: dependency.clone(),
                        })?,
                );
            }
        }
        self.resolved_dependencies = Some(resolved);
        Ok(())
    }

    /// Returns the already-resolved dependencies of one package without
    /// scanning package nodes or allocating a result vector.
    pub fn dependencies(&self, id: PackageId) -> Result<&[PackageId], GraphError> {
        if self.package(id).is_none() {
            return Err(GraphError::UnknownPackage(id.index().to_string()));
        }
        let resolved = self
            .resolved_dependencies
            .as_ref()
            .ok_or(GraphError::DependenciesNotResolved)?;
        let range = &self.dependency_ranges[id.index()];
        Ok(&resolved[range.clone()])
    }

    /// Registers only new or changed files and returns the persistent unified
    /// unit. A no-op request reuses the existing unit without walking packages.
    pub fn register_sources(&mut self, session: &mut Session) -> Option<DefinitionUnit> {
        let handle = session.handle();
        if self.session != Some(handle) {
            self.session = Some(handle);
            self.file_ids.fill(None);
            self.dirty_sources.clear();
            self.dirty_index.clear();
            for index in 0..self.source_count() {
                self.mark_source_dirty(index);
            }
            self.unit = None;
        }

        for index in std::mem::take(&mut self.dirty_sources) {
            self.dirty_index.remove(&index);
            let source = &self.source_package.sources()[index];
            let id = session.upsert_file(&source.name, Arc::clone(&source.text));
            session.register_unit(DefinitionUnit::new(id));
            self.file_ids[index] = Some(id);
        }

        if self.unit.is_none() {
            let files = self.file_ids.iter().copied().collect::<Option<Vec<_>>>()?;
            let unit = DefinitionUnit::from_files(files)?;
            session.register_unit(unit.clone());
            self.unit = Some(unit);
        }
        self.unit.clone()
    }

    fn mark_source_dirty(&mut self, index: usize) {
        if self.dirty_index.insert(index) {
            self.dirty_sources.push(index);
        }
    }
}

fn source_identity(package: &str, source: &str) -> Arc<str> {
    Arc::from(format!("{package}/{source}"))
}

#[cfg(test)]
mod tests {
    use super::{DefinitionGraph, GraphError};
    use crate::{DefinitionPackage, DefinitionSource, Session};

    fn target() -> klf_target::Target {
        "aarch64-apple-darwin".parse().expect("valid target")
    }

    #[test]
    fn package_module_source_and_dependency_indexes_resolve_directly() {
        let mut graph = DefinitionGraph::new();
        let core = graph
            .add_package(
                "core",
                "Core",
                Vec::new(),
                &DefinitionPackage::single("app/core.klfd", "function core() {}"),
            )
            .expect("core package");
        let app = graph
            .add_package(
                "app",
                "App",
                vec!["core".to_owned()],
                &DefinitionPackage::single("app/main.klfd", "function main() {}"),
            )
            .expect("app package");

        assert_eq!(graph.package_id("app"), Some(app));
        assert_eq!(graph.module_owner("Core"), Some(core));
        assert_eq!(graph.source_owner("app/app/main.klfd"), Some(app));
        graph.resolve_dependencies().expect("dependency index");
        assert_eq!(
            graph.dependencies(app).expect("resolved dependency"),
            [core]
        );
    }

    #[test]
    fn source_edits_keep_one_registered_file_and_unified_unit() {
        let mut graph = DefinitionGraph::new();
        graph
            .add_package(
                "app",
                "App",
                Vec::new(),
                &DefinitionPackage::from_sources(vec![
                    DefinitionSource::new("a.klfd", "function a() {}"),
                    DefinitionSource::new("b.klfd", "function b() {}"),
                ]),
            )
            .expect("app package");
        let mut session = Session::new(target());
        let first_unit = graph
            .register_sources(&mut session)
            .expect("registered sources");
        assert_eq!(
            graph.register_sources(&mut session),
            Some(first_unit.clone())
        );
        assert!(
            graph
                .replace_source("app", "a.klfd", "function a() -> Int { return 1 }")
                .expect("replace source")
        );
        let edited_unit = graph
            .register_sources(&mut session)
            .expect("updated sources");
        assert_eq!(edited_unit, first_unit);
        assert_eq!(session.files().len(), 2);
        assert_eq!(
            graph
                .replace_source("app", "a.klfd", "function a() -> Int { return 1 }")
                .expect("same source"),
            false
        );
    }

    #[test]
    fn package_and_module_identity_conflicts_are_rejected() {
        let mut graph = DefinitionGraph::new();
        graph
            .add_package(
                "one",
                "Shared",
                Vec::new(),
                &DefinitionPackage::single("main.klfd", ""),
            )
            .expect("first package");
        assert!(matches!(
            graph.add_package(
                "two",
                "Shared",
                Vec::new(),
                &DefinitionPackage::single("main.klfd", "")
            ),
            Err(GraphError::DuplicateModule { .. })
        ));
        assert!(matches!(
            graph.add_package(
                "one",
                "Other",
                Vec::new(),
                &DefinitionPackage::single("other.klfd", "")
            ),
            Err(GraphError::DuplicatePackage(_))
        ));
    }

    #[test]
    fn dependencies_resolve_forward_references_once_and_track_new_packages() {
        let mut graph = DefinitionGraph::new();
        let app = graph
            .add_package(
                "app",
                "App",
                vec!["core".to_owned()],
                &DefinitionPackage::single("main.klfd", ""),
            )
            .expect("app package with forward dependency");
        assert!(matches!(
            graph.resolve_dependencies(),
            Err(GraphError::MissingDependency { .. })
        ));
        let core = graph
            .add_package(
                "core",
                "Core",
                Vec::new(),
                &DefinitionPackage::single("core.klfd", ""),
            )
            .expect("core package");
        graph.resolve_dependencies().expect("dependency index");
        assert_eq!(
            graph.dependencies(app).expect("resolved dependency"),
            [core]
        );
    }
}
