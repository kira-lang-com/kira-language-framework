//! What a session's phases learned from the file system: every file they read,
//! every path they asked about and every directory they listed. A query's
//! result depends on these as much as on its unit's files (section 8.9), so a
//! result kept across builds is valid only while each still holds.
//!
//! What a speculation observed stays observed when it rolls back: the
//! speculation still read it.

use crate::Session;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Observation {
    /// The text at `path`, or `None` when it could not be read.
    Read {
        path: PathBuf,
        text: Option<Arc<str>>,
    },
    /// Whether a file or directory is at `path`.
    Exists { path: PathBuf, found: bool },
    /// The files under `directory` with `extension`, in path order, or `None`
    /// when the directory could not be listed.
    Listing {
        directory: PathBuf,
        extension: String,
        paths: Option<Vec<PathBuf>>,
    },
}

impl Session {
    /// Records what a phase observed, in the order it observed it.
    pub fn observe(&mut self, observations: impl IntoIterator<Item = Observation>) {
        self.observations.extend(observations);
    }

    /// Everything the session's phases have observed so far.
    #[must_use]
    pub fn observations(&self) -> &[Observation] {
        &self.observations
    }
}

/// Whether [`note_access`] keeps what it is given.
static RECORDING: AtomicBool = AtomicBool::new(false);

/// Every path the process's reads, probes and listings touched since
/// [`record_accesses`], absolute: what a driver's `--depfile` names
/// (section 5.5). The phases and the caches that stand in for them reach the
/// file system through the same functions, so a phase a cache answered still
/// touched every path its answer depends on.
static ACCESSED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// Starts keeping every path [`note_access`] is given, for the rest of the
/// process.
pub fn record_accesses() {
    RECORDING.store(true, Ordering::Relaxed);
}

/// Notes that the process read `path`, asked whether it exists, or listed it.
pub fn note_access(path: &Path) {
    if !RECORDING.load(Ordering::Relaxed) {
        return;
    }
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if let Ok(mut accessed) = ACCESSED.lock() {
        accessed.insert(path);
    }
}

/// The paths noted since [`record_accesses`], in path order.
#[must_use]
pub fn accessed() -> Vec<PathBuf> {
    ACCESSED
        .lock()
        .map(|accessed| accessed.iter().cloned().collect())
        .unwrap_or_default()
}
