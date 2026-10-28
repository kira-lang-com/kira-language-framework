//! The session's analysis mode, the open documents that stand in for files on
//! disk, and the editor facts its phases recorded (`KLF-EDITOR.md` sections 6,
//! 8.1 and 8.4).

use super::Session;
use klf_facts::{AnalysisMode, Facts};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct EditorState {
    mode: AnalysisMode,
    facts: Facts,
    overlay: Arc<HashMap<PathBuf, Arc<str>>>,
}

impl Session {
    #[must_use]
    pub const fn mode(&self) -> AnalysisMode {
        self.editor.mode
    }

    pub fn set_mode(&mut self, mode: AnalysisMode) {
        self.editor.mode = mode;
    }

    #[must_use]
    pub const fn facts(&self) -> &Facts {
        &self.editor.facts
    }

    /// Adds what one phase recorded, after the facts already held.
    pub fn record_facts(&mut self, facts: Facts) {
        self.editor.facts.absorb(facts);
    }

    /// Takes the recorded facts, leaving the session with none.
    pub fn take_facts(&mut self) -> Facts {
        std::mem::take(&mut self.editor.facts)
    }

    /// Sets the texts that replace files on disk when a phase reads them: the
    /// documents an editor has open.
    pub fn set_overlay(&mut self, documents: impl IntoIterator<Item = (PathBuf, Arc<str>)>) {
        self.editor.overlay = Arc::new(
            documents
                .into_iter()
                .map(|(path, text)| (overlay_key(&path), text))
                .collect(),
        );
    }

    #[must_use]
    pub fn overlay(&self) -> Arc<HashMap<PathBuf, Arc<str>>> {
        Arc::clone(&self.editor.overlay)
    }

    /// The text a read of `path` sees: the open document, else the file.
    ///
    /// # Errors
    /// Returns the error reading the file.
    pub fn read_text(&self, path: &Path) -> std::io::Result<Arc<str>> {
        read_through(&self.editor.overlay, path)
    }
}

/// The key a path has in an overlay: canonical when the file exists, else
/// absolute, so a document not yet saved is found by the path it will have.
#[must_use]
pub fn overlay_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Reads `path` through `overlay`.
///
/// # Errors
/// Returns the error reading the file when no document stands in for it.
pub fn read_through<S: std::hash::BuildHasher>(
    overlay: &HashMap<PathBuf, Arc<str>, S>,
    path: &Path,
) -> std::io::Result<Arc<str>> {
    crate::note_access(path);
    if let Some(text) = overlay.get(&overlay_key(path)) {
        return Ok(Arc::clone(text));
    }
    std::fs::read_to_string(path).map(Arc::from)
}
