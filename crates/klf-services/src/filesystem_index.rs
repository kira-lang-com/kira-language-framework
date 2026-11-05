//! A process-wide index of directory entries used by input expansion and
//! language definition services. Directory snapshots are keyed by filesystem
//! identity where available and checked against the full identity before
//! reuse, so aliases share one listing without hiding edits or retargeted
//! symlinks.

use crate::{FileIdentity, file_identity, file_identity_from_metadata};
use std::collections::{HashMap, VecDeque, hash_map::DefaultHasher};
use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

const SHARD_COUNT: usize = 64;
const MAX_SNAPSHOTS_PER_SHARD: usize = 1_024;
const MAX_SNAPSHOT_BYTES_PER_SHARD: usize = 512 * 1024;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum DirectoryKey {
    #[cfg(unix)]
    FileSystem { device: u64, inode: u64 },
    #[cfg(not(unix))]
    Path(PathBuf),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectorySnapshot {
    names: Arc<[OsString]>,
    identity: Option<FileIdentity>,
    memory_cost: usize,
}

impl DirectorySnapshot {
    fn memory_cost(&self) -> usize {
        self.memory_cost
    }
}

impl DirectorySnapshot {
    /// Immediate entry names in the order returned by the filesystem.
    #[must_use]
    pub fn names(&self) -> &[OsString] {
        &self.names
    }

    /// The filesystem identity, when the directory stayed unchanged while it
    /// was listed.
    #[must_use]
    pub fn identity(&self) -> Option<&FileIdentity> {
        self.identity.as_ref()
    }

    /// Entry paths rooted at the spelling used for this request.
    #[must_use]
    pub fn paths(&self, directory: &Path) -> Vec<PathBuf> {
        self.names.iter().map(|name| directory.join(name)).collect()
    }
}

#[derive(Default)]
struct DirectoryShard {
    entries: HashMap<DirectoryKey, DirectorySnapshot>,
    insertion_order: VecDeque<DirectoryKey>,
    cached_bytes: usize,
}

/// A bounded, concurrent directory snapshot index shared by the driver's
/// input walker and every language definition service in the process.
pub struct DirectoryIndex {
    shards: [RwLock<DirectoryShard>; SHARD_COUNT],
}

impl Default for DirectoryIndex {
    fn default() -> Self {
        Self {
            shards: std::array::from_fn(|_| RwLock::new(DirectoryShard::default())),
        }
    }
}

impl DirectoryIndex {
    /// Lists a directory, reusing an unchanged snapshot across aliases and
    /// build sessions.
    ///
    /// Filesystem I/O happens outside the shard lock, so unrelated directory
    /// scans remain concurrent. Concurrent first requests for the same
    /// directory may race to list it; later requests share one snapshot.
    ///
    /// # Errors
    /// Returns the filesystem error when the directory cannot be listed.
    pub fn snapshot(&self, directory: &Path) -> io::Result<DirectorySnapshot> {
        self.snapshot_with_identity(directory, file_identity(directory))
    }

    /// Lists a directory using metadata already obtained while walking its
    /// parent, avoiding a second stat for that directory.
    ///
    /// # Errors
    /// Returns the filesystem error when the directory cannot be listed.
    pub fn snapshot_with_metadata(
        &self,
        directory: &Path,
        metadata: &std::fs::Metadata,
    ) -> io::Result<DirectorySnapshot> {
        self.snapshot_with_identity(directory, file_identity_from_metadata(metadata))
    }

    fn snapshot_with_identity(
        &self,
        directory: &Path,
        before: Option<FileIdentity>,
    ) -> io::Result<DirectorySnapshot> {
        let key = directory_key(directory, before.as_ref());
        let shard_index = key.as_ref().map(key_shard);
        if let (Some(key), Some(shard_index)) = (&key, shard_index)
            && let Ok(shard) = self.shards[shard_index].read()
            && let Some(snapshot) = shard.entries.get(key)
            && snapshot.identity == before
        {
            return Ok(snapshot.clone());
        }

        let names = std::fs::read_dir(directory)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<Vec<_>>>()?;
        let after = file_identity(directory);
        let identity = before.filter(|before| Some(before) == after.as_ref());
        let names: Arc<[OsString]> = Arc::from(names);
        let memory_cost = std::mem::size_of::<DirectorySnapshot>()
            + names
                .iter()
                .map(|name| std::mem::size_of::<OsString>() + name.capacity())
                .sum::<usize>();
        let snapshot = DirectorySnapshot {
            names,
            identity,
            memory_cost,
        };

        if let Some(key) = directory_key(directory, snapshot.identity.as_ref()) {
            let shard_index = key_shard(&key);
            let mut shard = self.shards[shard_index]
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(current) = shard.entries.get(&key)
                && current.identity == snapshot.identity
            {
                return Ok(current.clone());
            }
            if let Some(previous) = shard.entries.get(&key) {
                shard.cached_bytes -= previous.memory_cost();
            }
            if !shard.entries.contains_key(&key) {
                shard.insertion_order.push_back(key.clone());
            }
            shard.cached_bytes += snapshot.memory_cost();
            shard.entries.insert(key, snapshot.clone());
            while shard.entries.len() > MAX_SNAPSHOTS_PER_SHARD
                || shard.cached_bytes > MAX_SNAPSHOT_BYTES_PER_SHARD
            {
                let Some(oldest) = shard.insertion_order.pop_front() else {
                    break;
                };
                if let Some(removed) = shard.entries.remove(&oldest) {
                    shard.cached_bytes -= removed.memory_cost();
                }
            }
        }
        Ok(snapshot)
    }
}

fn directory_key(directory: &Path, identity: Option<&FileIdentity>) -> Option<DirectoryKey> {
    let identity = identity?;
    #[cfg(unix)]
    {
        let _ = directory;
        Some(DirectoryKey::FileSystem {
            device: identity.device,
            inode: identity.inode,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = identity;
        Some(DirectoryKey::Path(directory.to_path_buf()))
    }
}

fn key_shard(key: &DirectoryKey) -> usize {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish() as usize % SHARD_COUNT
}

static SHARED_DIRECTORY_INDEX: OnceLock<DirectoryIndex> = OnceLock::new();

/// The shared index used by one-shot commands and persistent build services.
#[must_use]
pub fn shared_directory_index() -> &'static DirectoryIndex {
    SHARED_DIRECTORY_INDEX.get_or_init(DirectoryIndex::default)
}
