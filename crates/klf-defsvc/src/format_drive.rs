//! Formatting one file (`KLF-FORMAT.md` section 5): `units` and `parse` as
//! for a build, then the definition's `format` hook over the tree, with a
//! layout open for the `klf.format.*` services. The VM-hosted driver and a
//! native frontend run the same schedule; each supplies the hook call.

use crate::drive::run_units;
use crate::{DefinitionServiceBridge, PhaseRunner, prepare_phase, settle_phase_diagnostics};
use klf_core::FileId;
use klf_diag::RuntimeDiagnostic;
use klf_format::{Builder, FormatError, Options, Syntax};
use klf_services::{DefinitionPhase, DefinitionPhaseArguments, DefinitionPhaseResult, Session};
use std::fmt;

/// The KIR entry the KLF Kira lowerer generates around a `format` hook.
pub const FORMAT_ENTRY: &str = "__klf_format";

/// What calls a definition's `format` hook.
pub trait FormatRunner: PhaseRunner {
    /// Calls the hook with `Elaborate`-shaped arguments: the session, the
    /// unit and its tree. Answers the layout and the hook's `Bool`.
    ///
    /// # Errors
    /// Returns a message when the hook traps or fails.
    fn format(
        &mut self,
        session: &mut Session,
        arguments: &DefinitionPhaseArguments,
        syntax: Syntax,
    ) -> Result<(Builder, bool), String>;
}

#[derive(Debug)]
pub enum FormatFailure {
    /// The file does not parse, or the definition declined it.
    Declined(String),
    /// The layout changes more than whitespace and comment placement
    /// (`KLF-LSP-9002`).
    Layout(FormatError),
    /// The definition's code failed.
    Run(String),
}

impl fmt::Display for FormatFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Declined(message) | Self::Run(message) => formatter.write_str(message),
            Self::Layout(error) => write!(formatter, "KLF-LSP-9002: {error}"),
        }
    }
}

/// Formats `file` and answers its formatted text.
///
/// # Errors
/// Returns [`FormatFailure`] when the file does not parse, the hook declines
/// or fails, or its layout is rejected.
pub fn format_file<R: FormatRunner>(
    session: &mut Session,
    file: FileId,
    syntax: &Syntax,
    options: &Options,
    make_runner: &mut dyn FnMut() -> Result<R, String>,
) -> Result<String, FormatFailure> {
    session.set_emit("format");
    let units = run_units(session, &[file], make_runner)
        .map_err(|error| FormatFailure::Declined(error.to_string()))?;
    let unit = units
        .into_iter()
        .find(|unit| unit.file() == file)
        .ok_or_else(|| FormatFailure::Declined("no unit holds the file".to_owned()))?;
    let mut runner = make_runner().map_err(FormatFailure::Run)?;
    // A format run parses the file alone: no other package of the program is
    // read (`docs/conformers.md`).
    let arguments = DefinitionPhaseArguments::Parse {
        unit: unit.clone(),
        program: Vec::new(),
    };
    let tree = match runner.run(DefinitionPhase::Parse, session, &arguments) {
        Ok(DefinitionPhaseResult::Artifact(tree)) => tree,
        Ok(_) => {
            return Err(FormatFailure::Run(
                "parse returned the wrong value".to_owned(),
            ));
        }
        Err(message) => return Err(FormatFailure::Declined(message)),
    };
    let arguments = DefinitionPhaseArguments::Elaborate { unit, tree };
    let (builder, formatted) = runner
        .format(session, &arguments, syntax.clone())
        .map_err(FormatFailure::Run)?;
    if !formatted {
        return Err(FormatFailure::Declined(
            "the definition did not format the file".to_owned(),
        ));
    }
    let source = builder.source().to_owned();
    let doc = builder.finish().map_err(FormatFailure::Layout)?;
    klf_format::print_source(&source, &doc, options).map_err(FormatFailure::Layout)
}

/// Opens one `format` hook call: the bridge with a layout over the unit's
/// file, and the argument words.
///
/// # Errors
/// Returns a message when the arguments do not belong to the session.
pub fn open_format(
    session: &mut Session,
    arguments: &DefinitionPhaseArguments,
    syntax: Syntax,
) -> Result<(DefinitionServiceBridge, Vec<u64>, String), String> {
    let (bridge, words, source) =
        prepare_phase(session, DefinitionPhase::Elaborate, arguments, false)?;
    bridge.begin_format(syntax)?;
    Ok((bridge, words, source))
}

/// Closes one `format` hook call that returned `word`.
///
/// # Errors
/// Returns a message when the hook reported an error.
pub fn close_format(
    bridge: &DefinitionServiceBridge,
    session: &mut Session,
    reported: &[RuntimeDiagnostic],
    word: u64,
) -> Result<(Builder, bool), String> {
    bridge.commit_records(session)?;
    settle_phase_diagnostics(session, bridge.file, reported)?;
    Ok((bridge.take_format()?, word & 1 == 1))
}
