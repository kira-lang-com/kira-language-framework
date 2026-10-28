//! Loading a `.klfd` definition package and running it as a language.
//!
//! Section 7.1 gives the package layout: every `.klfd` file of the directory
//! tree except under `std/` and `corpus/` belongs to the package and they all
//! share one namespace. A subdirectory holding its own `definitions/` is a
//! nested package (a second language shipped beside the first) and is not part of the outer one. Section 7.5 rule 1 requires exactly one declaration
//! extending `Language`, and rule 4 requires the KIR it produces to pass the
//! raw verifier; a failure there is a bug in the definition, not in the user
//! program, so it is reported as an internal error.

use crate::phase_runner::{PhaseHost, PhaseProgram, hosts_artifacts};
use crate::{Backend, BackendDefinitionExecutor};
use klf_defsvc::{DriveError, drive};
use klf_diag::Diagnostic;
use klf_diag::progress;
use klf_kir::Module;
use klf_kira_lower::{KiraDefinition, LanguageDefinitionMetadata};
use klf_services::{
    Definition, DefinitionImage, DefinitionPackage, DefinitionSource, DefinitionUnit,
    DriverRequest, LanguageDefaults, LanguageDescriptor, Session, effective_file_defaults,
    select_file_mode,
};
use klf_target::Target;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Directories of a package root that hold sources of the defined language
/// rather than of the definition itself.
const EXCLUDED_ROOT_DIRECTORIES: [&str; 3] = ["std", "corpus", "tests"];

#[derive(Debug)]
pub enum PackageError {
    Read { path: PathBuf, error: io::Error },
    Empty { root: PathBuf },
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, error } => {
                write!(formatter, "cannot read {}: {error}", path.display())
            }
            Self::Empty { root } => write!(
                formatter,
                "{} contains no .klfd definition sources",
                root.display()
            ),
        }
    }
}

impl std::error::Error for PackageError {}

/// Reads a definition package from disk.
///
/// # Errors
/// Returns [`PackageError`] when the tree cannot be read or holds no `.klfd`
/// source outside `std/` and `corpus/`.
pub fn load_package(root: &Path) -> Result<DefinitionPackage, PackageError> {
    let mut sources = Vec::new();
    collect_sources(root, root, &mut sources)?;
    for included in includes(root)? {
        let mut shared = Vec::new();
        collect_sources(&included.path, &included.path, &mut shared)?;
        for mut source in shared {
            source.name = format!("{}/{}", included.name, source.name);
            sources.push(source);
        }
    }
    if sources.is_empty() {
        return Err(PackageError::Empty {
            root: root.to_path_buf(),
        });
    }
    sources.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(DefinitionPackage::from_sources(sources))
}

/// A directory whose sources a package takes as its own.
struct Include {
    name: String,
    path: PathBuf,
}

/// The directories the package's `include` file names, one per line,
/// relative to the package root. Packages that share code, as the shader
/// outs share their text writer, list the directory holding it.
fn includes(root: &Path) -> Result<Vec<Include>, PackageError> {
    let file = root.join("include");
    if !file.is_file() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&file).map_err(|error| PackageError::Read {
        path: file.clone(),
        error,
    })?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| Include {
            name: line.to_owned(),
            path: root.join(line),
        })
        .collect())
}

fn collect_sources(
    root: &Path,
    directory: &Path,
    sources: &mut Vec<DefinitionSource>,
) -> Result<(), PackageError> {
    let entries = fs::read_dir(directory).map_err(|error| PackageError::Read {
        path: directory.to_path_buf(),
        error,
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| PackageError::Read {
            path: directory.to_path_buf(),
            error,
        })?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|error| PackageError::Read {
            path: path.clone(),
            error,
        })?;
        if kind.is_dir() {
            if excluded(root, &path) {
                continue;
            }
            collect_sources(root, &path, sources)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "klfd")
        {
            let text = fs::read_to_string(&path).map_err(|error| PackageError::Read {
                path: path.clone(),
                error,
            })?;
            sources.push(DefinitionSource::new(source_name(root, &path), text));
        }
    }
    Ok(())
}

fn excluded(root: &Path, directory: &Path) -> bool {
    if directory != root && directory.join("definitions").is_dir() {
        return true;
    }
    directory.parent() == Some(root)
        && directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| EXCLUDED_ROOT_DIRECTORIES.contains(&name))
}

fn source_name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// A definition package compiled to an executable image, with the typed
/// section-7 metadata of its single `Language` declaration.
pub struct LanguageDefinition {
    metadata: LanguageDefinitionMetadata,
    image: DefinitionImage,
    executor: BackendDefinitionExecutor,
    /// The image lowered to bytecode, once for every compilation.
    program: std::sync::OnceLock<Result<PhaseProgram, String>>,
}

impl LanguageDefinition {
    /// Loads a package directory and compiles it for `backend`.
    #[must_use]
    pub fn load(session: &mut Session, root: &Path, backend: Backend) -> Option<Self> {
        progress!("read definition package");
        match load_package(root) {
            Ok(package) => Self::from_package(session, &package, backend),
            Err(error) => {
                session.report(Diagnostic::error(
                    "KLF-DEF-0001",
                    format!("cannot load definition package: {error}"),
                ));
                None
            }
        }
    }

    /// Compiles an in-memory package and validates its `Language` declaration.
    #[must_use]
    pub fn from_package(
        session: &mut Session,
        package: &DefinitionPackage,
        backend: Backend,
    ) -> Option<Self> {
        let target = session.target().clone();
        let mut image_session = Session::new(target.clone());
        let built = build_image(&mut image_session, package, target);
        report_image_diagnostics(session, &image_session);
        let (metadata, module) = built?;
        let image = match DefinitionImage::new(metadata.descriptor.clone(), module) {
            Ok(image) => image,
            Err(error) => {
                session.report(Diagnostic::error(
                    "KLF-DEF-0003",
                    format!("definition `{}`: {error}", metadata.descriptor.name),
                ));
                return None;
            }
        };
        Some(Self {
            metadata,
            image,
            executor: BackendDefinitionExecutor::new(backend),
            program: std::sync::OnceLock::new(),
        })
    }

    #[must_use]
    pub fn descriptor(&self) -> &LanguageDescriptor {
        &self.metadata.descriptor
    }

    #[must_use]
    pub fn metadata(&self) -> &LanguageDefinitionMetadata {
        &self.metadata
    }

    #[must_use]
    pub fn image(&self) -> &DefinitionImage {
        &self.image
    }

    /// Runs the `compatDriver` override, if the definition declares one, over
    /// the original process arguments after `argv[0]`.
    ///
    /// # Errors
    /// Returns a message when the override traps or its result cannot be read.
    pub fn compat_driver(&self, arguments: &[String]) -> Result<Option<DriverRequest>, String> {
        if self.metadata.entrypoints.compat_driver.is_none() {
            return Ok(None);
        }
        crate::phase_runner::run_compat_driver(&self.image, arguments)
    }

    /// Runs the `project` hook (`KLF-EDITOR.md` section 5.3), if the
    /// definition declares one, for `path`.
    ///
    /// # Errors
    /// Returns a message when the hook traps, reports an error, or its result
    /// cannot be read.
    pub fn project(
        &self,
        session: &mut Session,
        path: &str,
    ) -> Result<Option<klf_defsvc::HookProject>, String> {
        if self.metadata.entrypoints.project.is_none() {
            return Ok(None);
        }
        crate::phase_runner::run_project(&self.image, session, path)
    }

    /// Runs the `ParsableCommand` numbered `index` with the values parsed
    /// for its argument slots, and answers the exit status its `run` returns.
    ///
    /// # Errors
    /// Returns a message when the command traps or fails, or the definition
    /// has no runnable command.
    pub fn run_command(
        &self,
        index: u32,
        values: Vec<Vec<klf_args::Value>>,
    ) -> Result<i32, String> {
        crate::phase_runner::run_command(&self.image, index, values)
    }

    /// Runs the `runProgram` hook, if the definition declares one, with the
    /// command that runs a built program and the driver arguments of the
    /// build; `None` without a hook.
    ///
    /// # Errors
    /// Returns a message when the hook traps or fails.
    pub fn run_program(
        &self,
        command: &[String],
        driver_arguments: &[String],
    ) -> Result<Option<i32>, String> {
        if self.metadata.entrypoints.run_program.is_none() {
            return Ok(None);
        }
        crate::phase_runner::run_program(&self.image, command, driver_arguments).map(Some)
    }

    /// Drives `units`, then `parse`, `elaborate` and `lower` for every unit the
    /// definition returned, and verifies the KIR it produced.
    #[must_use]
    pub fn compile_sources(
        &self,
        session: &mut Session,
        inputs: &[DefinitionSource],
    ) -> Option<Module> {
        let descriptor = &self.metadata.descriptor;
        session.set_language_defaults(LanguageDefaults {
            memory: descriptor.memory,
            convention: descriptor.convention,
            exclusivity: descriptor.exclusivity,
            file_modes: descriptor.file_modes.clone(),
            flag_globs: Vec::new(),
        });
        let mut files = Vec::with_capacity(inputs.len());
        for source in inputs {
            let selected_mode = match select_file_mode(
                &self.metadata.descriptor.file_modes,
                Path::new(&source.name),
                &source.text,
                &[],
            ) {
                Ok(mode) => mode,
                Err(error) => {
                    session.report(Diagnostic::error(
                        "KLF-DRV-0022",
                        format!("{}: {error}", source.name),
                    ));
                    return None;
                }
            };
            let file = session.add_file(source.name.as_str(), source.text.clone());
            let (memory, convention) = effective_file_defaults(
                self.metadata.descriptor.memory,
                self.metadata.descriptor.convention,
                selected_mode,
            );
            if let Err(error) = session.set_file_defaults(file, memory, convention) {
                session.report(Diagnostic::error("KLF-DRV-0022", error));
                return None;
            }
            if let Err(error) =
                session.set_file_exclusivity(file, self.metadata.descriptor.exclusivity)
            {
                session.report(Diagnostic::error("KLF-DRV-0022", error));
                return None;
            }
            if let Some(selected_mode) = selected_mode {
                let index = self
                    .metadata
                    .descriptor
                    .file_modes
                    .iter()
                    .position(|mode| std::ptr::eq(mode, selected_mode))?;
                if let Err(error) = session.set_file_mode(file, index) {
                    session.report(Diagnostic::error("KLF-DRV-0022", error));
                    return None;
                }
            }
            files.push(file);
        }
        for file in &files {
            session.register_unit(DefinitionUnit::new(*file));
        }
        session.register_diagnostics(self.metadata.descriptor.diagnostics.clone());

        let backend = self.executor.backend();
        let program = if hosts_artifacts(backend) {
            progress!("lower definition phases");
            match self.phase_program() {
                Ok(program) => Some(program),
                Err(message) => {
                    self.internal_error(session, "KLF-DEF-0006", &message);
                    return None;
                }
            }
        } else {
            None
        };

        let image = &self.image;
        let mut make_runner = || {
            Ok(program
                .as_ref()
                .map_or(PhaseHost::Backend { image, backend }, |program| {
                    program.host(image)
                }))
        };
        match drive(session, &files, &mut make_runner) {
            Ok(module) => Some(module),
            Err(error) => {
                self.report_drive_error(session, &error);
                None
            }
        }
    }

    fn report_drive_error(&self, session: &mut Session, error: &DriveError) {
        match error {
            DriveError::NoInputs => session.report(Diagnostic::error(
                "KLF-DEF-0002",
                "a definition needs at least one input file",
            )),
            DriveError::Phase { .. } | DriveError::Mismatch(_) | DriveError::Runner(_) => {
                session.report(Diagnostic::error(
                    "KLF-DEF-0006",
                    format!("definition `{}` {error}", self.metadata.descriptor.name),
                ));
            }
            DriveError::Leaked => {
                self.internal_error(session, "KLF-RT-0100", &error.to_string());
            }
            DriveError::Conflict(message) => {
                self.internal_error(session, "KLF-KIR-9002", message);
            }
            DriveError::Verify(failures) => {
                for failure in failures {
                    self.internal_error(session, "KLF-KIR-9001", failure);
                }
            }
        }
    }

    fn internal_error(&self, session: &mut Session, code: &'static str, message: &str) {
        session.report(Diagnostic::error(
            code,
            format!("definition `{}` {message}", self.metadata.descriptor.name),
        ));
    }
}

impl Definition for LanguageDefinition {
    fn name(&self) -> String {
        self.metadata.descriptor.name.clone()
    }

    fn extensions(&self) -> Vec<String> {
        self.metadata.descriptor.extensions.clone()
    }

    fn language(&self) -> LanguageDescriptor {
        self.metadata.descriptor.clone()
    }

    fn compile_package(
        &self,
        session: &mut Session,
        package: &DefinitionPackage,
        _module_name: &str,
        _target: Target,
    ) -> Option<Module> {
        self.compile_sources(session, package.sources())
    }

    fn compile(
        &self,
        session: &mut Session,
        source_name: &str,
        source: &str,
        _module_name: &str,
        _target: Target,
    ) -> Option<Module> {
        self.compile_sources(session, &[DefinitionSource::new(source_name, source)])
    }
}

fn build_image(
    session: &mut Session,
    package: &DefinitionPackage,
    target: Target,
) -> Option<(LanguageDefinitionMetadata, Module)> {
    progress!("compile definition");
    let units = KiraDefinition.units(session, package)?;
    let unit = units.into_iter().next()?;
    progress!("parse definition");
    let parsed = klf_kira_lower::parse_unit(session, &unit)?;
    let metadata = klf_kira_lower::extract_language_metadata(session, &parsed.file)?;
    progress!("lower definition");
    let module = KiraDefinition.compile_unit(session, unit, "Definition", target)?;
    Some((metadata, module))
}

/// Moves the diagnostics of the private session a package is compiled in into
/// `session`, along with the sources they point at, so they render with their
/// file and line.
fn report_image_diagnostics(session: &mut Session, image_session: &Session) {
    if image_session.diagnostics().is_empty() {
        return;
    }
    let files: Vec<_> = image_session
        .files()
        .iter()
        .map(|file| session.add_file(file.name().clone(), file.shared_text()))
        .collect();
    let remap = |file: &mut klf_core::FileId| {
        if let Some(moved) = files.get(file.index()) {
            *file = *moved;
        }
    };
    for diagnostic in image_session.diagnostics() {
        let mut diagnostic = diagnostic.clone();
        if let Some(span) = diagnostic.primary_span.as_mut() {
            remap(&mut span.file);
        }
        for label in &mut diagnostic.labels {
            remap(&mut label.span.file);
        }
        for note in &mut diagnostic.notes {
            if let Some(span) = note.span.as_mut() {
                remap(&mut span.file);
            }
        }
        session.report(diagnostic);
    }
}

impl LanguageDefinition {
    /// The image's phase program, built on first use.
    pub(crate) fn phase_program(&self) -> Result<&PhaseProgram, String> {
        self.program
            .get_or_init(|| PhaseProgram::new(&self.image))
            .as_ref()
            .map_err(Clone::clone)
    }
}
