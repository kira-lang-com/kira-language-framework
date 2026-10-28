use crate::definition::{
    Definition, DefinitionArtifact, DefinitionUnit, DefinitionUnitList, LanguageDescriptor,
};
use crate::session::{ArtifactId, Session};
use klf_core::FileId;
use klf_diag::Diagnostic;
use klf_kir::Module;
use klf_target::Target;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefinitionPhase {
    Units,
    Parse,
    Elaborate,
    Lower,
}

impl DefinitionPhase {
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Units => "units",
            Self::Parse => "parse",
            Self::Elaborate => "elaborate",
            Self::Lower => "lower",
        }
    }

    /// The phase's position in the `units`, `parse`, `elaborate`, `lower`
    /// order, which is the selector a native frontend's `klffe_phase`
    /// trampoline switches on.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Units => 0,
            Self::Parse => 1,
            Self::Elaborate => 2,
            Self::Lower => 3,
        }
    }

    const ALL: [Self; 4] = [Self::Units, Self::Parse, Self::Elaborate, Self::Lower];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefinitionPhaseArguments {
    Units {
        inputs: Vec<FileId>,
    },
    /// Parses `unit`. `program` lists the file of every unit of the program
    /// the `units` phase returned, in its order, including `unit`'s own. A
    /// parse reads the declarations of the program's other packages from it
    /// (`docs/conformers.md`). Empty when the caller names no program, which
    /// leaves the parse to `unit` alone.
    Parse {
        unit: DefinitionUnit,
        program: Vec<FileId>,
    },
    Elaborate {
        unit: DefinitionUnit,
        tree: DefinitionArtifact,
    },
    Lower {
        unit: DefinitionUnit,
        program: DefinitionArtifact,
    },
}

impl DefinitionPhaseArguments {
    #[must_use]
    pub const fn phase(&self) -> DefinitionPhase {
        match self {
            Self::Units { .. } => DefinitionPhase::Units,
            Self::Parse { .. } => DefinitionPhase::Parse,
            Self::Elaborate { .. } => DefinitionPhase::Elaborate,
            Self::Lower { .. } => DefinitionPhase::Lower,
        }
    }

    /// Marshals typed session-owned handles at the backend boundary.
    ///
    /// The first word identifies the session. Units and artifacts use handles
    /// allocated by that session; the units input also carries its file count
    /// followed by the registered file handles.
    ///
    /// # Errors
    /// Returns [`DefinitionHandleError`] when an input file, unit or artifact
    /// does not belong to the session.
    pub fn marshal(&self, session: &Session) -> Result<Vec<u64>, DefinitionHandleError> {
        let mut words = vec![session.handle().as_u64()];
        match self {
            Self::Units { inputs } => {
                for file in inputs {
                    if !session.has_file(*file) {
                        return Err(DefinitionHandleError::FileNotRegistered { file: *file });
                    }
                }
                words.push(u64::try_from(inputs.len()).unwrap_or(u64::MAX));
            }
            Self::Parse { unit, program } => {
                words.push(unit_word(session, unit)?);
                for file in program {
                    if !session.has_file(*file) {
                        return Err(DefinitionHandleError::FileNotRegistered { file: *file });
                    }
                }
                words.push(u64::try_from(program.len()).unwrap_or(u64::MAX));
            }
            Self::Elaborate { unit, tree } => {
                words.push(unit_word(session, unit)?);
                words.push(artifact_word(session, tree)?);
            }
            Self::Lower { unit, program } => {
                words.push(unit_word(session, unit)?);
                words.push(artifact_word(session, program)?);
            }
        }
        Ok(words)
    }
}

fn unit_word(session: &Session, unit: &DefinitionUnit) -> Result<u64, DefinitionHandleError> {
    session
        .unit_id(unit)
        .map(|id| u64::from(id.as_u32()))
        .ok_or_else(|| DefinitionHandleError::UnitNotRegistered { unit: unit.clone() })
}

fn artifact_word(
    session: &Session,
    artifact: &DefinitionArtifact,
) -> Result<u64, DefinitionHandleError> {
    if !artifact.belongs_to(session) || !session.has_artifact(artifact.id()) {
        return Err(DefinitionHandleError::ArtifactNotRegistered { id: artifact.id() });
    }
    Ok(u64::try_from(artifact.id().index()).unwrap_or(u64::MAX))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefinitionHandleError {
    PhaseMismatch {
        requested: DefinitionPhase,
        arguments: DefinitionPhase,
    },
    FileNotRegistered {
        file: FileId,
    },
    UnitNotRegistered {
        unit: DefinitionUnit,
    },
    ArtifactNotRegistered {
        id: ArtifactId,
    },
}

impl fmt::Display for DefinitionHandleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PhaseMismatch {
                requested,
                arguments,
            } => write!(
                formatter,
                "definition phase arguments are for `{}` but `{}` was requested",
                arguments.symbol(),
                requested.symbol()
            ),
            Self::FileNotRegistered { file } => {
                write!(
                    formatter,
                    "file handle {} is not registered in the session",
                    file.as_u32()
                )
            }
            Self::UnitNotRegistered { .. } => {
                formatter.write_str("definition unit is not registered in the session")
            }
            Self::ArtifactNotRegistered { id } => write!(
                formatter,
                "artifact handle {} is not registered in the session",
                id.index()
            ),
        }
    }
}

impl std::error::Error for DefinitionHandleError {}

#[derive(Clone, Debug, PartialEq)]
pub enum DefinitionPhaseResult {
    Units(Vec<DefinitionUnit>),
    Artifact(DefinitionArtifact),
    Module(Box<Module>),
    /// What a `ShaderLanguage`'s `lower` returns (`KLF-KSL.md` section 5.2).
    Shader(Box<klf_shader::Module>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefinitionResultError {
    PhaseMismatch {
        requested: DefinitionPhase,
        arguments: DefinitionPhase,
    },
    MissingReturn {
        phase: DefinitionPhase,
    },
    InvalidHandle {
        phase: DefinitionPhase,
    },
}

impl DefinitionPhaseResult {
    /// Like [`Self::from_return_word`], but a lowered module is moved out of
    /// the session rather than copied, for a caller that is done with it.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::from_return_word`].
    pub fn take_return_word(
        phase: DefinitionPhase,
        session: &mut Session,
        arguments: &DefinitionPhaseArguments,
        word: Option<u64>,
    ) -> Result<Self, DefinitionResultError> {
        if let (DefinitionPhase::Lower, DefinitionPhaseArguments::Lower { .. }, Some(word)) =
            (phase, arguments, word)
        {
            let handle =
                u32::try_from(word).map_err(|_| DefinitionResultError::InvalidHandle { phase })?;
            let id = ArtifactId::from_u32(handle);
            if let Some(module) = session.take_artifact::<Module>(id) {
                return Ok(Self::Module(Box::new(module)));
            }
            return session
                .take_artifact::<klf_shader::Module>(id)
                .map(|module| Self::Shader(Box::new(module)))
                .ok_or(DefinitionResultError::InvalidHandle { phase });
        }
        Self::from_return_word(phase, session, arguments, word)
    }

    /// Resolves a phase return word through the originating session.
    ///
    /// The `units` phase returns a [`crate::DefinitionUnitList`] artifact: the
    /// unit handles it registered, in the order it produced them.
    ///
    /// # Errors
    /// Returns [`DefinitionResultError`] when the phase arguments do not match,
    /// the phase returns no word, or the word is not a session-owned value of
    /// the expected kind.
    pub fn from_return_word(
        phase: DefinitionPhase,
        session: &Session,
        arguments: &DefinitionPhaseArguments,
        word: Option<u64>,
    ) -> Result<Self, DefinitionResultError> {
        if arguments.phase() != phase {
            return Err(DefinitionResultError::PhaseMismatch {
                requested: phase,
                arguments: arguments.phase(),
            });
        }
        let Some(word) = word else {
            return Err(DefinitionResultError::MissingReturn { phase });
        };
        let handle =
            u32::try_from(word).map_err(|_| DefinitionResultError::InvalidHandle { phase })?;
        match (phase, arguments) {
            (DefinitionPhase::Units, DefinitionPhaseArguments::Units { .. }) => {
                let list = session
                    .artifact::<DefinitionUnitList>(ArtifactId::from_u32(handle))
                    .ok_or(DefinitionResultError::InvalidHandle { phase })?;
                let mut units = Vec::with_capacity(list.units.len());
                for id in &list.units {
                    let unit = session
                        .unit(*id)
                        .cloned()
                        .ok_or(DefinitionResultError::InvalidHandle { phase })?;
                    units.push(unit);
                }
                Ok(Self::Units(units))
            }
            (DefinitionPhase::Parse, DefinitionPhaseArguments::Parse { unit, .. })
            | (DefinitionPhase::Elaborate, DefinitionPhaseArguments::Elaborate { unit, .. }) => {
                if !session.has_artifact(ArtifactId::from_u32(handle)) {
                    return Err(DefinitionResultError::InvalidHandle { phase });
                }
                Ok(Self::Artifact(DefinitionArtifact::from_id(
                    session,
                    unit.clone(),
                    ArtifactId::from_u32(handle),
                )))
            }
            (DefinitionPhase::Lower, DefinitionPhaseArguments::Lower { .. }) => {
                let id = ArtifactId::from_u32(handle);
                if let Some(module) = session.artifact::<Module>(id) {
                    return Ok(Self::Module(Box::new(module.clone())));
                }
                session
                    .artifact::<klf_shader::Module>(id)
                    .cloned()
                    .map(|module| Self::Shader(Box::new(module)))
                    .ok_or(DefinitionResultError::InvalidHandle { phase })
            }
            _ => Err(DefinitionResultError::PhaseMismatch {
                requested: phase,
                arguments: arguments.phase(),
            }),
        }
    }
}

impl fmt::Display for DefinitionResultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PhaseMismatch {
                requested,
                arguments,
            } => write!(
                formatter,
                "definition phase result arguments are for `{}` but `{}` was requested",
                arguments.symbol(),
                requested.symbol()
            ),
            Self::MissingReturn { phase } => {
                write!(
                    formatter,
                    "definition phase `{}` returned no handle",
                    phase.symbol()
                )
            }
            Self::InvalidHandle { phase } => write!(
                formatter,
                "definition phase `{}` returned an invalid session handle",
                phase.symbol()
            ),
        }
    }
}

impl std::error::Error for DefinitionResultError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefinitionImageError {
    MissingPhase { phase: DefinitionPhase },
    DuplicatePhase { phase: DefinitionPhase },
    NonExecutablePhase { phase: DefinitionPhase },
    UnclaimedFileModeExtension { mode: String, extension: String },
    DuplicateFileModeExtension { extension: String },
}

impl fmt::Display for DefinitionImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPhase { phase } => {
                write!(
                    formatter,
                    "definition image is missing `{}`",
                    phase.symbol()
                )
            }
            Self::DuplicatePhase { phase } => {
                write!(
                    formatter,
                    "definition image contains duplicate `{}`",
                    phase.symbol()
                )
            }
            Self::NonExecutablePhase { phase } => {
                write!(
                    formatter,
                    "definition image phase `{}` has no body",
                    phase.symbol()
                )
            }
            Self::UnclaimedFileModeExtension { mode, extension } => write!(
                formatter,
                "file mode `{mode}` claims extension `{extension}` not declared by the language"
            ),
            Self::DuplicateFileModeExtension { extension } => write!(
                formatter,
                "file mode extension `{extension}` is assigned more than once"
            ),
        }
    }
}

impl std::error::Error for DefinitionImageError {}

/// A compiled definition module with the mandatory Language phase symbols.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionImage {
    descriptor: LanguageDescriptor,
    module: Module,
}

impl DefinitionImage {
    /// Validates and stores a compiled definition module.
    ///
    /// # Errors
    /// Returns [`DefinitionImageError`] when a mandatory phase symbol is
    /// missing, duplicated or not executable.
    pub fn new(
        descriptor: LanguageDescriptor,
        module: Module,
    ) -> Result<Self, DefinitionImageError> {
        for phase in DefinitionPhase::ALL {
            let count = module
                .functions
                .iter()
                .filter(|function| function.name.as_str() == phase.symbol())
                .count();
            if count == 0 {
                return Err(DefinitionImageError::MissingPhase { phase });
            }
            if count > 1 {
                return Err(DefinitionImageError::DuplicatePhase { phase });
            }
            let Some(function) = module
                .functions
                .iter()
                .find(|function| function.name.as_str() == phase.symbol())
            else {
                return Err(DefinitionImageError::MissingPhase { phase });
            };
            if function.foreign || function.blocks.is_empty() {
                return Err(DefinitionImageError::NonExecutablePhase { phase });
            }
        }
        let mut assigned_extensions = HashSet::new();
        for mode in &descriptor.file_modes {
            for extension in &mode.extensions {
                if !descriptor.extensions.contains(extension) {
                    return Err(DefinitionImageError::UnclaimedFileModeExtension {
                        mode: mode.name.clone(),
                        extension: extension.clone(),
                    });
                }
                if !assigned_extensions.insert(extension.as_str()) {
                    return Err(DefinitionImageError::DuplicateFileModeExtension {
                        extension: extension.clone(),
                    });
                }
            }
        }
        Ok(Self { descriptor, module })
    }

    #[must_use]
    pub fn descriptor(&self) -> &LanguageDescriptor {
        &self.descriptor
    }

    #[must_use]
    pub fn module(&self) -> &Module {
        &self.module
    }

    ///
    /// # Panics
    /// Panics only if the image was mutated outside its validated constructor.
    #[must_use]
    pub fn phase(&self, phase: DefinitionPhase) -> &klf_kir::Function {
        self.module
            .functions
            .iter()
            .find(|function| function.name.as_str() == phase.symbol())
            .expect("validated definition image phase")
    }
}
pub trait DefinitionPhaseExecutor {
    /// Executes one phase and resolves its typed result in the session.
    ///
    /// # Errors
    /// Returns the backend or phase execution error from the implementation.
    fn execute(
        &self,
        image: &DefinitionImage,
        phase: DefinitionPhase,
        session: &mut Session,
        arguments: &DefinitionPhaseArguments,
    ) -> Result<DefinitionPhaseResult, String>;
}

/// A `Definition` view over a validated compiled image.
pub struct ExecutableDefinition {
    image: DefinitionImage,
    executor: Option<Arc<dyn DefinitionPhaseExecutor>>,
}

impl ExecutableDefinition {
    #[must_use]
    pub fn new(image: DefinitionImage) -> Self {
        Self {
            image,
            executor: None,
        }
    }

    #[must_use]
    pub fn with_executor<E: DefinitionPhaseExecutor + 'static>(
        image: DefinitionImage,
        executor: E,
    ) -> Self {
        Self {
            image,
            executor: Some(Arc::new(executor)),
        }
    }

    #[must_use]
    pub fn image(&self) -> &DefinitionImage {
        &self.image
    }
}

fn execute_phase(
    executor: &dyn DefinitionPhaseExecutor,
    image: &DefinitionImage,
    phase: DefinitionPhase,
    session: &mut Session,
    arguments: &DefinitionPhaseArguments,
) -> Option<DefinitionPhaseResult> {
    match executor.execute(image, phase, session, arguments) {
        Ok(result) => Some(result),
        Err(message) => {
            session.report(Diagnostic::error(
                "KLF-DEF-0006",
                format!("definition phase `{}` failed: {message}", phase.symbol()),
            ));
            None
        }
    }
}

fn report_result_mismatch(session: &mut Session, phase: DefinitionPhase) {
    session.report(Diagnostic::error(
        "KLF-DEF-0006",
        format!(
            "definition phase `{}` returned the wrong value",
            phase.symbol()
        ),
    ));
}
impl Definition for ExecutableDefinition {
    fn name(&self) -> String {
        self.image.descriptor().name.clone()
    }

    fn extensions(&self) -> Vec<String> {
        self.image.descriptor().extensions.clone()
    }

    fn language(&self) -> LanguageDescriptor {
        self.image.descriptor().clone()
    }

    fn compile(
        &self,
        session: &mut Session,
        source_name: &str,
        source: &str,
        _module_name: &str,
        _target: Target,
    ) -> Option<Module> {
        let Some(executor) = &self.executor else {
            session.report(Diagnostic::error(
                "KLF-DEF-0005",
                "executable definition Session-handle bridge is not configured",
            ));
            return None;
        };
        let file = session.add_file(source_name, Arc::from(source));
        let input_unit = DefinitionUnit::new(file);
        session.register_unit(input_unit);
        let units = execute_phase(
            executor.as_ref(),
            &self.image,
            DefinitionPhase::Units,
            session,
            &DefinitionPhaseArguments::Units { inputs: vec![file] },
        )?;
        let DefinitionPhaseResult::Units(units) = units else {
            report_result_mismatch(session, DefinitionPhase::Units);
            return None;
        };
        if units.len() != 1 {
            session.report(Diagnostic::error(
                "KLF-DEF-0002",
                "the current executable definition path accepts one source unit",
            ));
            return None;
        }
        let unit = units.into_iter().next()?;
        if session.unit_id(&unit).is_none() {
            report_result_mismatch(session, DefinitionPhase::Units);
            return None;
        }

        let parsed = execute_phase(
            executor.as_ref(),
            &self.image,
            DefinitionPhase::Parse,
            session,
            &DefinitionPhaseArguments::Parse {
                unit: unit.clone(),
                program: vec![unit.file()],
            },
        )?;
        let DefinitionPhaseResult::Artifact(parsed) = parsed else {
            report_result_mismatch(session, DefinitionPhase::Parse);
            return None;
        };
        if parsed.unit() != unit {
            report_result_mismatch(session, DefinitionPhase::Parse);
            return None;
        }

        let elaborated = execute_phase(
            executor.as_ref(),
            &self.image,
            DefinitionPhase::Elaborate,
            session,
            &DefinitionPhaseArguments::Elaborate {
                unit: unit.clone(),
                tree: parsed,
            },
        )?;
        let DefinitionPhaseResult::Artifact(program) = elaborated else {
            report_result_mismatch(session, DefinitionPhase::Elaborate);
            return None;
        };
        if program.unit() != unit {
            report_result_mismatch(session, DefinitionPhase::Elaborate);
            return None;
        }

        let lowered = execute_phase(
            executor.as_ref(),
            &self.image,
            DefinitionPhase::Lower,
            session,
            &DefinitionPhaseArguments::Lower { unit, program },
        )?;
        let DefinitionPhaseResult::Module(module) = lowered else {
            report_result_mismatch(session, DefinitionPhase::Lower);
            return None;
        };
        Some(*module)
    }
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
