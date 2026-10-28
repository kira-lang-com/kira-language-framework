//! The kernel-side `Definition` interface (the Rust form of the section-7
//! `Language` family).
//!
//! A definition is a compiler frontend: it reads source text of its language
//! and produces a KIR module through kernel [`Session`] services. In the full
//! design a definition is a `.klfd` package executed by the kernel; the current
//! bridge keeps the KLF Kira frontend and the toy calculator Rust-hosted while
//! the definition object and phase ABI are built.

use crate::diagnostics::DiagnosticSpec;
use crate::session::{Session, SessionHandle};
use klf_core::FileId;
use klf_diag::Diagnostic;
use klf_diag::progress;
use klf_kir::Module;
use klf_target::Target;
use std::any::Any;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryModel {
    Unmanaged,
    Counted,
    Affine,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Exclusivity {
    Static,
    Dynamic,
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefaultConvention {
    Ridl,
    Platform,
}

/// How a built executable ends (the optional `entry` member of `Language`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EntryConvention {
    /// Print the `RET`/`TRAP` outcome line and exit 0, which the kernel's
    /// test drivers read.
    #[default]
    Outcome,
    /// End as a C process: `main(argc, argv)`, the entry's value as the exit
    /// status, traps reported on stderr and aborting.
    Process,
    /// [`EntryConvention::Process`], but a trap ends the program with exit
    /// status 1, as a failed command does, rather than aborting.
    ProcessExit,
}

/// A per-file exception to the package-level metadata (section 7.3). A file
/// enters the mode through the pragma or the driver flag; the overrides that
/// are `None` keep the package-level choice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMode {
    pub name: String,
    pub pragma: String,
    pub flag: String,
    /// Source extensions that put a file in this mode without a pragma or a
    /// flag. Empty when the mode is entered only by pragma or flag.
    pub extensions: Vec<String>,
    pub memory: Option<MemoryModel>,
    pub convention: Option<DefaultConvention>,
}

/// Resolves the optional per-file overrides against the language defaults.
#[must_use]
pub fn effective_file_defaults(
    memory: MemoryModel,
    convention: DefaultConvention,
    mode: Option<&FileMode>,
) -> (MemoryModel, DefaultConvention) {
    mode.map_or((memory, convention), |mode| {
        (
            mode.memory.unwrap_or(memory),
            mode.convention.unwrap_or(convention),
        )
    })
}

/// A driver flag and its typed default value (sections 7.2–7.3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverFlag {
    pub name: String,
    pub default_value: DriverFlagValue,
}

/// Scalar types currently supported for declared driver flag defaults.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DriverFlagValue {
    Boolean(bool),
    Integer(i64),
    String(String),
}

/// The typed metadata exposed by a section-7 `Language` object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageDescriptor {
    pub name: String,
    pub driver: String,
    pub version: String,
    pub extensions: Vec<String>,
    pub memory: MemoryModel,
    pub exclusivity: Exclusivity,
    pub convention: DefaultConvention,
    pub std_root: String,
    pub entry: EntryConvention,
    pub file_modes: Vec<FileMode>,
    pub flags: Vec<DriverFlag>,
    pub diagnostics: Vec<DiagnosticSpec>,
    /// The declared `editor` member; absent means `EditorSpec.minimal`.
    pub editor: Option<klf_facts::spec::EditorSpec>,
    /// The `ShaderLanguage` members; absent for a `Language`.
    pub shader: Option<crate::ShaderFamily>,
    /// The definition's own command line (`KLF-CLI.md` section 3): its
    /// configuration, the options its `@Flag` and `@Option` members declare,
    /// and its `ParsableCommand` subcommands. The kernel adds the built-in
    /// commands.
    pub cli: klf_args::Command,
}

impl LanguageDescriptor {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        driver: impl Into<String>,
        version: impl Into<String>,
        extensions: Vec<String>,
        memory: MemoryModel,
        exclusivity: Exclusivity,
        convention: DefaultConvention,
        std_root: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            driver: driver.into(),
            version: version.into(),
            extensions,
            memory,
            exclusivity,
            convention,
            std_root: std_root.into(),
            entry: EntryConvention::Outcome,
            file_modes: Vec::new(),
            flags: Vec::new(),
            diagnostics: Vec::new(),
            editor: None,
            shader: None,
            cli: klf_args::Command::default(),
        }
    }

    #[must_use]
    pub fn with_cli(mut self, cli: klf_args::Command) -> Self {
        self.cli = cli;
        self
    }

    #[must_use]
    pub fn with_shader(mut self, shader: Option<crate::ShaderFamily>) -> Self {
        self.shader = shader;
        self
    }

    #[must_use]
    pub fn with_editor(mut self, editor: Option<klf_facts::spec::EditorSpec>) -> Self {
        self.editor = editor;
        self
    }

    /// The editor spec, `EditorSpec.minimal(name)` when none is declared.
    #[must_use]
    pub fn editor_spec(&self) -> klf_facts::spec::EditorSpec {
        self.editor
            .clone()
            .unwrap_or_else(|| klf_facts::spec::EditorSpec::minimal(&self.name))
    }

    #[must_use]
    pub fn with_file_modes(mut self, file_modes: Vec<FileMode>) -> Self {
        self.file_modes = file_modes;
        self
    }

    #[must_use]
    pub fn with_entry(mut self, entry: EntryConvention) -> Self {
        self.entry = entry;
        self
    }

    #[must_use]
    pub fn with_flags(mut self, flags: Vec<DriverFlag>) -> Self {
        self.flags = flags;
        self
    }

    #[must_use]
    pub fn file_mode(&self, name: &str) -> Option<&FileMode> {
        self.file_modes.iter().find(|mode| mode.name == name)
    }

    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: Vec<DiagnosticSpec>) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    #[must_use]
    pub fn minimal(name: &str, extensions: &[&str]) -> Self {
        Self::new(
            name,
            name,
            "0.1.0",
            extensions
                .iter()
                .map(|extension| (*extension).to_owned())
                .collect(),
            MemoryModel::Counted,
            Exclusivity::None,
            DefaultConvention::Platform,
            "",
        )
    }

    #[must_use]
    pub fn claims_extension(&self, extension: &str) -> bool {
        self.extensions
            .iter()
            .any(|candidate| candidate == extension)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionSource {
    pub name: String,
    pub text: Arc<str>,
}

impl DefinitionSource {
    #[must_use]
    pub fn new(name: impl Into<String>, text: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionPackage {
    sources: Vec<DefinitionSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefinitionRegistryError {
    DuplicateName {
        name: String,
    },
    DuplicateExtension {
        extension: String,
        first: String,
        second: String,
    },
}

impl fmt::Display for DefinitionRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName { name } => {
                write!(formatter, "definition name `{name}` is already registered")
            }
            Self::DuplicateExtension {
                extension,
                first,
                second,
            } => write!(
                formatter,
                "extension `.{extension}` is claimed by both `{first}` and `{second}`"
            ),
        }
    }
}

impl std::error::Error for DefinitionRegistryError {}

#[derive(Default)]
pub struct DefinitionRegistry {
    definitions: Vec<Box<dyn Definition>>,
}

impl DefinitionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a definition after validating its metadata.
    ///
    /// # Errors
    /// Returns [`DefinitionRegistryError`] when its name or an extension is
    /// already claimed by a registered definition, or repeated in its own
    /// metadata.
    pub fn register<D: Definition + 'static>(
        &mut self,
        definition: D,
    ) -> Result<(), DefinitionRegistryError> {
        self.register_box(Box::new(definition))
    }

    /// Registers a boxed definition after validating its metadata.
    ///
    /// # Errors
    /// Returns [`DefinitionRegistryError`] when its name or an extension is
    /// already claimed by a registered definition, or repeated in its own
    /// metadata.
    pub fn register_box(
        &mut self,
        definition: Box<dyn Definition>,
    ) -> Result<(), DefinitionRegistryError> {
        let language = definition.language();
        if self
            .definitions
            .iter()
            .any(|existing| existing.language().name == language.name)
        {
            return Err(DefinitionRegistryError::DuplicateName {
                name: language.name,
            });
        }

        let mut extensions = HashSet::new();
        for extension in &language.extensions {
            if !extensions.insert(extension) {
                return Err(DefinitionRegistryError::DuplicateExtension {
                    extension: extension.clone(),
                    first: language.name.clone(),
                    second: language.name,
                });
            }
            if let Some(existing) = self
                .definitions
                .iter()
                .find(|existing| existing.language().claims_extension(extension))
            {
                return Err(DefinitionRegistryError::DuplicateExtension {
                    extension: extension.clone(),
                    first: existing.language().name,
                    second: language.name,
                });
            }
        }

        self.definitions.push(definition);
        Ok(())
    }

    #[must_use]
    pub fn select(&self, file_name: &str) -> Option<&dyn Definition> {
        let extension = std::path::Path::new(file_name)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        self.definitions
            .iter()
            .find(|definition| definition.language().claims_extension(extension))
            .map(Box::as_ref)
    }

    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&dyn Definition> {
        self.definitions
            .iter()
            .find(|definition| definition.language().name == name)
            .map(Box::as_ref)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DefinitionUnit {
    files: Arc<[FileId]>,
}

impl DefinitionUnit {
    #[must_use]
    pub fn new(file: FileId) -> Self {
        Self {
            files: Arc::from(vec![file]),
        }
    }

    #[must_use]
    pub fn from_files(files: Vec<FileId>) -> Option<Self> {
        if files.is_empty() {
            return None;
        }
        Some(Self {
            files: Arc::from(files),
        })
    }

    #[must_use]
    pub fn file(&self) -> FileId {
        self.files[0]
    }

    #[must_use]
    pub fn files(&self) -> &[FileId] {
        &self.files
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionArtifact {
    session: SessionHandle,
    unit: DefinitionUnit,
    id: crate::session::ArtifactId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefinitionArtifactKind {
    Units,
    Tree,
    Program,
    Module,
}

/// A phase artifact the definition owns.
///
/// The kernel holds the identity (the [`crate::ArtifactId`] a phase returns)
/// and the definition holds the value. `payload` is the word the definition
/// gave the kernel for it, opaque here and meaningful only to the definition
/// instance that produced it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefinitionRuntimeArtifact {
    pub kind: DefinitionArtifactKind,
    pub payload: u64,
}

/// The result of the `units` phase: the units the definition split its inputs
/// into, in the order it produced them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionUnitList {
    pub units: Vec<crate::session::UnitId>,
}

impl DefinitionArtifact {
    #[must_use]
    pub fn new<T: Any + Send + Sync>(
        session: &mut Session,
        unit: DefinitionUnit,
        payload: T,
    ) -> Self {
        Self {
            session: session.handle(),
            unit,
            id: session.store_artifact(payload),
        }
    }

    #[must_use]
    pub fn unit(&self) -> DefinitionUnit {
        self.unit.clone()
    }

    pub(crate) fn from_id(
        session: &Session,
        unit: DefinitionUnit,
        id: crate::session::ArtifactId,
    ) -> Self {
        Self {
            session: session.handle(),
            unit,
            id,
        }
    }

    #[must_use]
    pub const fn id(&self) -> crate::session::ArtifactId {
        self.id
    }

    pub(crate) fn belongs_to(&self, session: &Session) -> bool {
        self.session == session.handle()
    }

    #[must_use]
    pub fn downcast_ref<'a, T: Any>(&self, session: &'a Session) -> Option<&'a T> {
        if !self.belongs_to(session) {
            return None;
        }
        session.artifact(self.id)
    }
}

impl DefinitionPackage {
    #[must_use]
    pub fn single(name: impl Into<String>, text: impl Into<Arc<str>>) -> Self {
        Self {
            sources: vec![DefinitionSource::new(name, text)],
        }
    }

    #[must_use]
    pub fn from_sources(sources: Vec<DefinitionSource>) -> Self {
        Self { sources }
    }

    #[must_use]
    pub fn sources(&self) -> &[DefinitionSource] {
        &self.sources
    }
}

pub trait Definition {
    /// Lowercase language identifier (section 7.3).
    fn name(&self) -> String;

    /// File extensions the language claims; the first is primary.
    fn extensions(&self) -> Vec<String>;

    /// Typed section-7 metadata. Definitions may override the defaults as
    /// their language surface grows beyond the M4 compile entry point.
    fn language(&self) -> LanguageDescriptor {
        let name = self.name();
        let extensions = self.extensions();
        let extension_refs: Vec<&str> = extensions.iter().map(String::as_str).collect();
        LanguageDescriptor::minimal(&name, &extension_refs)
    }

    /// Whether this definition supplies the typed parse/elaborate/lower
    /// queries below. Legacy definitions continue through `compile_unit`.
    fn supports_phase_queries(&self) -> bool {
        false
    }

    /// Parses one registered unit into a language-owned type-erased artifact.
    fn parse(&self, _session: &mut Session, _unit: DefinitionUnit) -> Option<DefinitionArtifact> {
        None
    }

    /// Elaborates a parsed artifact into a language-owned program artifact.
    fn elaborate(
        &self,
        _session: &mut Session,
        _unit: DefinitionUnit,
        _parsed: &DefinitionArtifact,
    ) -> Option<DefinitionArtifact> {
        None
    }

    /// Lowers an elaborated artifact to KIR.
    fn lower(
        &self,
        _session: &mut Session,
        _unit: DefinitionUnit,
        _program: &DefinitionArtifact,
        _module_name: &str,
        _target: Target,
    ) -> Option<Module> {
        None
    }

    /// Groups package files into definition translation units.
    fn units(
        &self,
        session: &mut Session,
        package: &DefinitionPackage,
    ) -> Option<Vec<DefinitionUnit>> {
        if package.sources().is_empty() {
            session.report(Diagnostic::error(
                "KLF-DEF-0001",
                "definition package has no source files",
            ));
            return None;
        }
        Some(
            package
                .sources()
                .iter()
                .map(|source| {
                    let unit = DefinitionUnit::new(
                        session.add_file(source.name.as_str(), Arc::clone(&source.text)),
                    );
                    session.register_unit(unit.clone());
                    unit
                })
                .collect(),
        )
    }

    /// Compiles a definition package through the current one-file phase
    /// boundary. Multi-file package assembly is a later query layer.
    fn compile_package(
        &self,
        session: &mut Session,
        package: &DefinitionPackage,
        module_name: &str,
        target: Target,
    ) -> Option<Module> {
        let units = self.units(session, package)?;
        let unit = units.first().cloned()?;
        if units.len() != 1 {
            session.report(Diagnostic::error(
                "KLF-DEF-0002",
                "the current definition phase accepts one source file",
            ));
            return None;
        }
        let name = session.file(unit.file()).name().clone();
        if self.supports_phase_queries() {
            progress!("parse {name}");
            let parsed = self.parse(session, unit.clone())?;
            progress!("elaborate {name}");
            let program = self.elaborate(session, unit.clone(), &parsed)?;
            progress!("lower {name}");
            return self.lower(session, unit, &program, module_name, target);
        }
        progress!("lower {name}");
        self.compile_unit(session, unit, module_name, target)
    }

    /// Runs the current one-file parse/check/lower implementation for a unit.
    /// Definitions override this to consume the already registered file rather
    /// than registering the same source a second time.
    fn compile_unit(
        &self,
        session: &mut Session,
        unit: DefinitionUnit,
        module_name: &str,
        target: Target,
    ) -> Option<Module> {
        let file = session.file(unit.file());
        let source_name = file.name().as_str().to_owned();
        let source = file.text().to_owned();
        self.compile(session, &source_name, &source, module_name, target)
    }

    /// Reads one source file and lowers it to a KIR module, reporting any
    /// diagnostics into `session`. Returns `None` when compilation failed.
    fn compile(
        &self,
        session: &mut Session,
        source_name: &str,
        source: &str,
        module_name: &str,
        target: Target,
    ) -> Option<Module>;
}

#[cfg(test)]
#[path = "definition_tests.rs"]
mod tests;
