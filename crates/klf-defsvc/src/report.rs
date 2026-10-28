//! Rendering the diagnostics a phase reported.
//!
//! A phase that calls `klf.diag.error` is rejecting the program it was given,
//! not failing. The records it leaves behind carry only a code and a byte
//! range; the text comes from the `DiagnosticSpec` list the definition declared
//! on its `Language`. Both the VM-hosted driver and a native frontend resolve
//! them the same way, so the rendering lives here.

use klf_core::FileId;
use klf_diag::{RuntimeDiagnostic, Severity};
use klf_services::{ResolvedRuntimeDiagnostic, Session};

/// Resolves `records` against the session's registered specifications and
/// keeps them in the session. A phase that reported an error rejected the
/// program: the result is every diagnostic the session kept, this phase's and
/// the warnings of those before it, rendered with the source position each
/// points at, one per line. Warnings and notes alone let the
/// phase stand; they stay in the session for the driver to show.
///
/// # Errors
/// Returns the rendered diagnostics when the phase reported an error, or
/// every record unrendered when no file locates them.
pub fn settle_phase_diagnostics(
    session: &mut Session,
    file: Option<FileId>,
    records: &[RuntimeDiagnostic],
) -> Result<(), String> {
    if records.is_empty() {
        return Ok(());
    }
    let Some(file) = file else {
        return Err(unresolved(session, records));
    };
    let first = session.runtime_diagnostics().len();
    if let Err(record) = session.report_runtime_diagnostics(file, records) {
        return Err(session.describe_unresolved(record));
    }
    if session.runtime_diagnostics()[first..]
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Err(render_kept_diagnostics(session));
    }
    Ok(())
}

/// Every diagnostic the session's phases kept, rendered in the house style
/// (headline, source excerpt with carets, notes and helps; see
/// `klf_services::runtime_diagnostic`).
#[must_use]
pub fn render_kept_diagnostics(session: &Session) -> String {
    render_diagnostics(session, 0..session.runtime_diagnostics().len())
}

/// The diagnostics the session's phases kept in `kept`, rendered one per
/// line.
#[must_use]
pub fn render_diagnostics(session: &Session, kept: std::ops::Range<usize>) -> String {
    let rendered: Vec<String> = session.runtime_diagnostics()[kept]
        .iter()
        .map(|diagnostic| render(session, diagnostic))
        .collect();
    rendered.join("\n")
}

fn render(session: &Session, diagnostic: &ResolvedRuntimeDiagnostic) -> String {
    klf_services::runtime_diagnostic(session, diagnostic)
        .trim_end_matches('\n')
        .to_owned()
}

fn unresolved(session: &Session, records: &[RuntimeDiagnostic]) -> String {
    let Some(first) = records.first() else {
        return "phase reported no diagnostic".to_owned();
    };
    session.describe_unresolved(first.clone())
}
