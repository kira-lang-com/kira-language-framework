//! The `Session`: the substrate every frontend phase runs against (section 8).
//!
//! This is the kernel-side Rust surface. The current subset carries sources,
//! diagnostics, name interning, lexical scopes, import edges, query handles
//! and dependency tracking; arenas, typed types, solving and speculation are
//! session services for the current definition boundary.

mod artifacts;
mod editor;
mod emit;
mod expansion;
mod flags;
mod query_diagnostics;
mod runtime_diagnostics;
mod scope;

pub use editor::{overlay_key, read_through};
pub use expansion::Expansion;
pub use scope::{Entity, EntityId, Namespace, ScopeId};

use scope::Scope;

use crate::arena::{Arena, ArenaStore};
use crate::definition::{
    DefaultConvention, DefinitionUnit, DriverFlagValue, Exclusivity, MemoryModel,
};
use crate::diagnostics::{DiagnosticSpec, ResolvedRuntimeDiagnostic};
use crate::query::{QueryId, QueryStore};
use crate::service::{KERNEL_SERVICE_MANIFEST, KernelService, KernelServiceManifest};
use crate::shader_family::ShaderSettings;
use crate::solver::TraitStore;
use crate::types::TypeStore;
use klf_core::{FileId, Name};
use klf_diag::{Diagnostic, RuntimeDiagnostic};
use klf_source::{SourceFile, Span};
use klf_target::Target;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SESSION_HANDLE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactId(u32);

impl ArtifactId {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionHandle(u64);

impl SessionHandle {
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    fn fresh() -> Self {
        let value = NEXT_SESSION_HANDLE.fetch_add(1, Ordering::Relaxed);
        assert_ne!(value, 0, "session handle space exhausted");
        Self(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UnitId(u32);

impl UnitId {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Position {
    pub file: FileId,
    pub offset: u32,
}

impl Position {
    #[must_use]
    pub const fn new(file: FileId, offset: u32) -> Self {
        Self { file, offset }
    }
}

/// The language's defaults for its files and the file modes that override
/// them (sections 7.2 and 7.3).
#[derive(Clone, Debug)]
pub struct LanguageDefaults {
    pub memory: MemoryModel,
    pub convention: DefaultConvention,
    pub exclusivity: Exclusivity,
    pub file_modes: Vec<crate::definition::FileMode>,
    /// The driver's `<flag>=<glob>` file-mode selections.
    pub flag_globs: Vec<(String, String)>,
}

pub struct Session {
    handle: SessionHandle,
    target: Target,
    flags: HashMap<String, DriverFlagValue>,
    std_root: String,
    pub(crate) shader: Option<ShaderSettings>,
    files: Vec<SourceFile>,
    file_identities: Vec<Option<crate::FileIdentity>>,
    file_ids: HashMap<Name, FileId>,
    file_modes: Vec<Option<usize>>,
    file_memories: Vec<Option<MemoryModel>>,
    file_exclusivities: Vec<Option<Exclusivity>>,
    file_conventions: Vec<Option<DefaultConvention>>,
    expansions: Vec<Expansion>,
    /// What a file the session registers without the driver's own setup
    /// takes, such as one a phase read (section 7.3).
    language_defaults: Option<LanguageDefaults>,
    units: Vec<DefinitionUnit>,
    unit_ids: HashMap<DefinitionUnit, UnitId>,
    units_by_file: HashMap<FileId, Vec<UnitId>>,
    diagnostics: Vec<Diagnostic>,
    diagnostic_owners: Vec<Option<QueryId>>,
    query_diagnostics: HashMap<QueryId, Vec<Diagnostic>>,
    runtime_diagnostics: Vec<ResolvedRuntimeDiagnostic>,
    diagnostic_specs: HashMap<i64, DiagnosticSpec>,
    symbols: HashMap<Name, Name>,
    arenas: ArenaStore,
    types: TypeStore,
    traits: TraitStore,
    scopes: Vec<Scope>,
    entities: Vec<Entity>,
    artifacts: Vec<Arc<dyn Any + Send + Sync>>,
    services: KernelServiceManifest,
    pub(crate) queries: QueryStore,
    pub(crate) observations: Vec<crate::observe::Observation>,
    editor: editor::EditorState,
}

impl Session {
    /// Creates a session with the target and bundled kernel service registry.
    ///
    /// # Panics
    /// Panics if the bundled kernel service manifest is invalid.
    #[must_use]
    pub fn new(target: Target) -> Self {
        Self {
            handle: SessionHandle::fresh(),
            target,
            flags: emit::initial_flags(),
            std_root: String::new(),
            shader: None,
            files: Vec::new(),
            file_identities: Vec::new(),
            file_ids: HashMap::new(),
            file_modes: Vec::new(),
            file_memories: Vec::new(),
            file_exclusivities: Vec::new(),
            file_conventions: Vec::new(),
            expansions: Vec::new(),
            language_defaults: None,
            units: Vec::new(),
            unit_ids: HashMap::new(),
            units_by_file: HashMap::new(),
            diagnostics: Vec::new(),
            diagnostic_owners: Vec::new(),
            query_diagnostics: HashMap::new(),
            runtime_diagnostics: Vec::new(),
            diagnostic_specs: HashMap::new(),
            symbols: HashMap::new(),
            arenas: ArenaStore::new(),
            types: TypeStore::new(),
            traits: TraitStore::new(),
            scopes: Vec::new(),
            entities: Vec::new(),
            artifacts: Vec::new(),
            services: KernelServiceManifest::parse(KERNEL_SERVICE_MANIFEST)
                .expect("kernel service manifest is valid"),
            queries: QueryStore::new(),
            observations: Vec::new(),
            editor: editor::EditorState::default(),
        }
    }

    #[must_use]
    pub fn handle(&self) -> SessionHandle {
        self.handle
    }

    #[must_use]
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The directory holding the language's std sources (`stdRoot`, section
    /// 7), empty when the driver does not know one.
    #[must_use]
    pub fn std_root(&self) -> &str {
        &self.std_root
    }

    /// Records where the language's std sources are.
    pub fn set_std_root(&mut self, root: impl Into<String>) {
        self.std_root = root.into();
    }
    /// Returns the typed kernel service registry for this session.
    #[must_use]
    pub fn kernel_services(&self) -> &KernelServiceManifest {
        &self.services
    }

    /// Looks up one kernel service by its manifest name.
    #[must_use]
    pub fn kernel_service(&self, name: &str) -> Option<&KernelService> {
        self.services.service(name)
    }

    /// Records the language's defaults and file modes, with the driver's
    /// `<flag>=<glob>` selections, which every file registered from now on
    /// takes.
    pub fn set_language_defaults(&mut self, defaults: LanguageDefaults) {
        self.language_defaults = Some(defaults);
    }

    /// Registers source text and returns its handle. The frontend reads text
    /// back through [`Session::file`]. The file takes the language defaults
    /// and the file mode it selects, when the driver recorded them; a file
    /// two modes select is reported as `KLF-DRV-0022` and takes neither.
    ///
    /// # Panics
    /// Panics if more than `u32::MAX` files are registered in one session.
    pub fn add_file(
        &mut self,
        name: impl Into<Name>,
        text: impl Into<std::sync::Arc<str>>,
    ) -> FileId {
        self.add_file_with_identity(name, text, None)
    }

    /// Registers source text with the filesystem identity captured when it
    /// was read. The identity is used to validate cached query inputs without
    /// rereading unchanged source text.
    pub fn add_file_with_identity(
        &mut self,
        name: impl Into<Name>,
        text: impl Into<std::sync::Arc<str>>,
        identity: Option<crate::FileIdentity>,
    ) -> FileId {
        let name = name.into();
        let id = FileId::from_u32(u32::try_from(self.files.len()).expect("file count fits u32"));
        self.files.push(SourceFile::new(id, name.clone(), text));
        self.file_identities.push(identity);
        self.file_ids.entry(name).or_insert(id);
        self.file_modes.push(None);
        self.file_memories.push(None);
        self.file_exclusivities.push(None);
        self.file_conventions.push(None);
        if let Some(defaults) = &self.language_defaults {
            let file = &self.files[id.index()];
            let path = std::path::Path::new(file.name().as_str());
            match crate::select_file_mode(
                &defaults.file_modes,
                path,
                file.text(),
                &defaults.flag_globs,
            ) {
                Ok(mode) => {
                    let (memory, convention) =
                        crate::effective_file_defaults(defaults.memory, defaults.convention, mode);
                    self.file_memories[id.index()] = Some(memory);
                    self.file_conventions[id.index()] = Some(convention);
                    self.file_exclusivities[id.index()] = Some(defaults.exclusivity);
                    self.file_modes[id.index()] = mode.and_then(|mode| {
                        defaults
                            .file_modes
                            .iter()
                            .position(|known| std::ptr::eq(known, mode))
                    });
                }
                Err(error) => {
                    let message = format!("{}: {error}", file.name());
                    self.report(Diagnostic::error("KLF-DRV-0022", message));
                }
            }
        }
        id
    }

    /// The filesystem identity captured for `file`, when it came from an
    /// unchanged on-disk read.
    #[must_use]
    pub fn file_identity(&self, file: FileId) -> Option<&crate::FileIdentity> {
        self.file_identities
            .get(file.index())
            .and_then(Option::as_ref)
    }

    /// Registers a source by its stable name, replacing its text when it
    /// already belongs to this session.
    pub fn upsert_file(
        &mut self,
        name: impl AsRef<str>,
        text: impl Into<std::sync::Arc<str>>,
    ) -> FileId {
        let name = name.as_ref();
        let text = text.into();
        if let Some(id) = self.file_ids.get(name).copied() {
            if !self.file(id).shares_text_with(&text) && self.file(id).text() != text.as_ref() {
                self.replace_file(id, text);
            }
            return id;
        }
        self.add_file(Name::new(name), text)
    }

    #[must_use]
    pub fn has_file(&self, id: FileId) -> bool {
        self.files
            .get(id.index())
            .is_some_and(|file| file.id() == id)
    }

    /// Selects a language file-mode index for a registered source file.
    ///
    /// # Errors
    /// Returns a message when the file is not registered.
    pub fn set_file_mode(&mut self, file: FileId, index: usize) -> Result<(), String> {
        if !self.has_file(file) {
            return Err(format!(
                "cannot select a file mode for unknown source file {}",
                file.as_u32()
            ));
        }
        if i64::try_from(index).is_err() {
            return Err("file mode index does not fit the Kira Int type".to_owned());
        }
        self.file_modes[file.index()] = Some(index);
        Ok(())
    }

    /// Returns a selected file-mode index for a registered source file.
    #[must_use]
    pub fn file_mode(&self, file: FileId) -> Option<usize> {
        if !self.has_file(file) {
            return None;
        }
        self.file_modes.get(file.index()).copied().flatten()
    }

    /// Sets the effective language defaults for a registered source file.
    ///
    /// # Errors
    /// Returns a message when the file is not registered.
    pub fn set_file_defaults(
        &mut self,
        file: FileId,
        memory: MemoryModel,
        convention: DefaultConvention,
    ) -> Result<(), String> {
        if !self.has_file(file) {
            return Err(format!(
                "cannot set language defaults for unknown source file {}",
                file.as_u32()
            ));
        }
        self.file_memories[file.index()] = Some(memory);
        self.file_conventions[file.index()] = Some(convention);
        Ok(())
    }

    /// Returns the effective memory model configured for a source file.
    #[must_use]
    pub fn file_memory(&self, file: FileId) -> Option<MemoryModel> {
        if !self.has_file(file) {
            return None;
        }
        self.file_memories.get(file.index()).copied().flatten()
    }

    /// Sets the language exclusivity rule for a registered source file.
    ///
    /// # Errors
    /// Returns a message when the file is not registered.
    pub fn set_file_exclusivity(
        &mut self,
        file: FileId,
        exclusivity: Exclusivity,
    ) -> Result<(), String> {
        if !self.has_file(file) {
            return Err(format!(
                "cannot set language exclusivity for unknown source file {}",
                file.as_u32()
            ));
        }
        self.file_exclusivities[file.index()] = Some(exclusivity);
        Ok(())
    }

    /// Returns the language exclusivity rule configured for a source file.
    #[must_use]
    pub fn file_exclusivity(&self, file: FileId) -> Option<Exclusivity> {
        if !self.has_file(file) {
            return None;
        }
        self.file_exclusivities.get(file.index()).copied().flatten()
    }

    /// Returns the effective default convention configured for a source file.
    #[must_use]
    pub fn file_convention(&self, file: FileId) -> Option<DefaultConvention> {
        if !self.has_file(file) {
            return None;
        }
        self.file_conventions.get(file.index()).copied().flatten()
    }

    /// Registers a definition unit and returns its session-local handle.
    ///
    /// # Panics
    /// Panics if more than `u32::MAX` units are registered in one session.
    pub fn register_unit(&mut self, unit: DefinitionUnit) -> UnitId {
        if let Some(id) = self.unit_ids.get(&unit).copied() {
            return id;
        }
        let id = UnitId(u32::try_from(self.units.len()).expect("unit count fits u32"));
        self.unit_ids.insert(unit.clone(), id);
        for file in unit.files() {
            self.units_by_file.entry(*file).or_default().push(id);
        }
        self.units.push(unit);
        id
    }

    #[must_use]
    pub fn unit_id(&self, unit: &DefinitionUnit) -> Option<UnitId> {
        self.unit_ids.get(unit).copied()
    }

    #[must_use]
    pub fn unit(&self, id: UnitId) -> Option<&DefinitionUnit> {
        self.units.get(id.0 as usize)
    }

    /// The first registered unit holding `file`.
    #[must_use]
    pub fn unit_holding(&self, file: FileId) -> Option<UnitId> {
        self.units_by_file.get(&file)?.first().copied()
    }

    /// Replaces a registered file and invalidates queries that read it.
    pub fn replace_file(&mut self, id: FileId, text: impl Into<std::sync::Arc<str>>) {
        let name = self.file(id).name().clone();
        self.files[id.index()] = SourceFile::new(id, name, text);
        let invalidated = self.invalidate_file(id);
        self.invalidate_query_diagnostics(&invalidated);
    }

    #[must_use]
    pub fn file(&self, id: FileId) -> &SourceFile {
        &self.files[id.index()]
    }

    /// Returns a registered source's current text by path/name.
    #[must_use]
    pub fn input(&self, name: &str) -> Option<String> {
        let id = self.file_ids.get(name)?;
        Some(self.file(*id).text().to_owned())
    }

    /// All source files registered in this session, in file-id order.
    #[must_use]
    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }

    /// Resolves a source position through the session-owned source table.
    #[must_use]
    pub fn line_column(&self, position: Position) -> klf_source::LineColumn {
        self.file(position.file).line_column(position.offset)
    }

    /// Interns a definition-owned symbol for the lifetime of this session.
    #[must_use]
    pub fn intern(&mut self, text: impl AsRef<str>) -> Name {
        let candidate = Name::new(text);
        self.symbols
            .entry(candidate.clone())
            .or_insert(candidate)
            .clone()
    }

    /// Returns the text represented by an interned symbol.
    #[must_use]
    pub fn text(&self, symbol: &Name) -> String {
        symbol.as_str().to_owned()
    }

    /// Returns the named typed arena, creating it on first use.
    #[must_use]
    pub fn arena<T: 'static>(&mut self, name: Name) -> Arena<T> {
        self.arenas.get_or_create(name)
    }

    pub(crate) fn arena_store(&self) -> &ArenaStore {
        &self.arenas
    }

    pub(crate) fn arena_store_mut(&mut self) -> &mut ArenaStore {
        &mut self.arenas
    }

    pub(crate) fn type_store(&self) -> &TypeStore {
        &self.types
    }

    pub(crate) fn type_store_mut(&mut self) -> &mut TypeStore {
        &mut self.types
    }

    pub(crate) fn trait_store(&self) -> &TraitStore {
        &self.traits
    }

    pub(crate) fn trait_store_mut(&mut self) -> &mut TraitStore {
        &mut self.traits
    }

    pub fn report_error(&mut self, code: &'static str, span: Span, message: impl Into<String>) {
        self.report(Diagnostic::error_at(code, span, message));
    }

    /// Registers a numeric runtime diagnostic specification.
    ///
    /// Returns `false` when the code is already registered and keeps the
    /// original specification.
    pub fn register_diagnostic(&mut self, spec: DiagnosticSpec) -> bool {
        if self.diagnostic_specs.contains_key(&spec.code) {
            return false;
        }
        self.diagnostic_specs.insert(spec.code, spec);
        true
    }

    /// Registers a definition's diagnostic specifications as one session
    /// operation.
    pub fn register_diagnostics(
        &mut self,
        specs: impl IntoIterator<Item = DiagnosticSpec>,
    ) -> bool {
        let specs: Vec<_> = specs.into_iter().collect();
        let mut codes = HashSet::new();
        if specs
            .iter()
            .any(|spec| self.diagnostic_specs.contains_key(&spec.code) || !codes.insert(spec.code))
        {
            return false;
        }
        for spec in specs {
            self.diagnostic_specs.insert(spec.code, spec);
        }
        true
    }

    #[must_use]
    pub fn diagnostic_spec(&self, code: i64) -> Option<&DiagnosticSpec> {
        self.diagnostic_specs.get(&code)
    }

    #[must_use]
    pub fn runtime_diagnostics(&self) -> &[ResolvedRuntimeDiagnostic] {
        &self.runtime_diagnostics
    }

    /// Runs a tentative definition operation and rolls back its session state
    /// when the operation returns `None`.
    pub fn speculate<T: 'static, F>(&mut self, body: F) -> Option<T>
    where
        F: FnOnce(&mut Session) -> Option<T>,
    {
        let snapshot = SessionSnapshot {
            flags: self.flags.clone(),
            files: self.files.clone(),
            file_identities: self.file_identities.clone(),
            file_ids: self.file_ids.clone(),
            file_modes: self.file_modes.clone(),
            file_memories: self.file_memories.clone(),
            file_exclusivities: self.file_exclusivities.clone(),
            file_conventions: self.file_conventions.clone(),
            expansions: self.expansions.clone(),
            units: self.units.clone(),
            unit_ids: self.unit_ids.clone(),
            units_by_file: self.units_by_file.clone(),
            diagnostics: self.diagnostics.clone(),
            diagnostic_owners: self.diagnostic_owners.clone(),
            query_diagnostics: self.query_diagnostics.clone(),
            runtime_diagnostics: self.runtime_diagnostics.clone(),
            diagnostic_specs: self.diagnostic_specs.clone(),
            symbols: self.symbols.clone(),
            types: self.types.clone(),
            traits: self.traits.clone(),
            scopes: self.scopes.clone(),
            entities: self.entities.clone(),
            artifacts: self.artifacts.clone(),
            queries: self.queries.clone(),
        };
        let marker = self.arenas.begin_speculation();
        let result = body(self);
        if result.is_some() {
            self.arenas.commit_speculation(marker);
            result
        } else {
            self.arenas.rollback_speculation(marker);
            self.flags = snapshot.flags;
            self.files = snapshot.files;
            self.file_identities = snapshot.file_identities;
            self.file_ids = snapshot.file_ids;
            self.file_modes = snapshot.file_modes;
            self.file_memories = snapshot.file_memories;
            self.file_exclusivities = snapshot.file_exclusivities;
            self.file_conventions = snapshot.file_conventions;
            self.expansions = snapshot.expansions;
            self.units = snapshot.units;
            self.unit_ids = snapshot.unit_ids;
            self.units_by_file = snapshot.units_by_file;
            self.diagnostics = snapshot.diagnostics;
            self.diagnostic_owners = snapshot.diagnostic_owners;
            self.query_diagnostics = snapshot.query_diagnostics;
            self.runtime_diagnostics = snapshot.runtime_diagnostics;
            self.diagnostic_specs = snapshot.diagnostic_specs;
            self.symbols = snapshot.symbols;
            self.types = snapshot.types;
            self.traits = snapshot.traits;
            self.scopes = snapshot.scopes;
            self.entities = snapshot.entities;
            self.artifacts = snapshot.artifacts;
            self.queries = snapshot.queries;
            None
        }
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == klf_diag::Severity::Error)
            .count()
    }
}

struct SessionSnapshot {
    flags: HashMap<String, DriverFlagValue>,
    files: Vec<SourceFile>,
    file_identities: Vec<Option<crate::FileIdentity>>,
    file_ids: HashMap<Name, FileId>,
    file_modes: Vec<Option<usize>>,
    file_memories: Vec<Option<MemoryModel>>,
    file_exclusivities: Vec<Option<Exclusivity>>,
    file_conventions: Vec<Option<DefaultConvention>>,
    expansions: Vec<Expansion>,
    units: Vec<DefinitionUnit>,
    unit_ids: HashMap<DefinitionUnit, UnitId>,
    units_by_file: HashMap<FileId, Vec<UnitId>>,
    diagnostics: Vec<Diagnostic>,
    diagnostic_owners: Vec<Option<QueryId>>,
    query_diagnostics: HashMap<QueryId, Vec<Diagnostic>>,
    runtime_diagnostics: Vec<ResolvedRuntimeDiagnostic>,
    diagnostic_specs: HashMap<i64, DiagnosticSpec>,
    symbols: HashMap<Name, Name>,
    types: TypeStore,
    traits: TraitStore,
    scopes: Vec<Scope>,
    entities: Vec<Entity>,
    artifacts: Vec<Arc<dyn Any + Send + Sync>>,
    queries: QueryStore,
}
