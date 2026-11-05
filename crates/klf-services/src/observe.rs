//! What a session's phases learned from the file system: every file they read,
//! every path they asked about and every directory they listed. A query's
//! result depends on these as much as on its unit's files (section 8.9), so a
//! result kept across builds is valid only while each still holds.
//!
//! What a speculation observed stays observed when it rolls back: the
//! speculation still read it.

use crate::Session;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Filesystem identity captured alongside a source read. Stable identities let
/// incremental query validation avoid rereading and hashing unchanged files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileIdentity {
    pub length: u64,
    pub modified: u128,
    #[cfg(unix)]
    pub device: u64,
    #[cfg(unix)]
    pub inode: u64,
    #[cfg(unix)]
    pub changed: i128,
}

/// The metadata identity of `path`, when it exists and has a usable timestamp.
#[must_use]
pub fn file_identity(path: &Path) -> Option<FileIdentity> {
    let metadata = std::fs::metadata(path).ok()?;
    file_identity_from_metadata(&metadata)
}

/// The metadata identity, when its timestamp is usable.
#[must_use]
pub fn file_identity_from_metadata(metadata: &std::fs::Metadata) -> Option<FileIdentity> {
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    #[cfg(unix)]
    let (device, inode, changed) = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec()),
        )
    };
    Some(FileIdentity {
        length: metadata.len(),
        modified,
        #[cfg(unix)]
        device,
        #[cfg(unix)]
        inode,
        #[cfg(unix)]
        changed,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Observation {
    /// The text at `path`, or `None` when it could not be read.
    Read {
        path: PathBuf,
        text: Option<Arc<str>>,
        identity: Option<FileIdentity>,
    },
    /// Whether a file or directory is at `path`.
    Exists { path: PathBuf, found: bool },
    /// The files under `directory` with `extension`, in path order, or `None`
    /// when the directory could not be listed.
    Listing {
        directory: PathBuf,
        extension: String,
        paths: Option<Vec<PathBuf>>,
        identity: Option<FileIdentity>,
        overlay_entries: Vec<PathBuf>,
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
static ACCESSED: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();

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
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let path = if absolute
        .components()
        .any(|component| component == Component::CurDir)
    {
        let mut normalized = PathBuf::new();
        for component in absolute.components() {
            if component != Component::CurDir {
                normalized.push(component.as_os_str());
            }
        }
        normalized
    } else {
        absolute
    };
    if let Ok(mut accessed) = ACCESSED.get_or_init(Default::default).lock() {
        accessed.insert(path);
    }
}

/// The paths noted since [`record_accesses`], in path order.
#[must_use]
pub fn accessed() -> Vec<PathBuf> {
    ACCESSED
        .get_or_init(Default::default)
        .lock()
        .map(|accessed| accessed.iter().cloned().collect())
        .unwrap_or_default()
}

/// The immediate names contributed by editor overlays under `directory`.
#[must_use]
pub fn directory_overlay_entries(
    overlay: &std::collections::HashMap<PathBuf, Arc<str>>,
    directory: &Path,
) -> Vec<PathBuf> {
    let directory = crate::overlay_key(directory);
    let mut entries = std::collections::BTreeSet::new();
    for path in overlay.keys() {
        if let Ok(relative) = path.strip_prefix(&directory)
            && let Some(first) = relative.components().next()
        {
            entries.insert(directory.join(first.as_os_str()));
        }
    }
    entries.into_iter().collect()
}
