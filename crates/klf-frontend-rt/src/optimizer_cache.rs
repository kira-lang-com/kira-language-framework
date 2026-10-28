//! The functions the KIR optimizer made for one artifact, kept between its
//! builds under the KLF cache root (see [`klf_kir_passes::Memo`]). Each entry
//! is a file named by its digest, written once; a build reads only what it
//! asks for, writes only what it made, and then removes what it did not use,
//! so the cache holds one build. Digests start with this `klf`'s own build,
//! so a changed optimizer finds nothing.

use klf_ridl::sha256::{hex, sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(crate) struct OptimizerCache {
    directory: PathBuf,
    identity: Vec<u8>,
    used: Mutex<HashSet<[u8; 32]>>,
}

impl OptimizerCache {
    /// The cache for the artifact of `module` written to a file named as
    /// `output` is, for `triple`; none without a cache root.
    pub(crate) fn for_artifact(module: &str, output: &Path, triple: &str) -> Option<Self> {
        let file = output.file_name()?;
        let mut identity = Vec::new();
        identity.extend_from_slice(env!("CARGO_PKG_VERSION").as_bytes());
        // A development `klf` keeps its release number across changes to the
        // optimizer, so its own build is part of every digest.
        if let Some(built) = std::env::current_exe()
            .ok()
            .and_then(|executable| executable.metadata().ok())
        {
            let modified = built
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_nanos());
            identity.extend_from_slice(format!(" {modified} {}", built.len()).as_bytes());
        }
        let mut artifact = Vec::new();
        for part in [module.as_bytes(), file.as_encoded_bytes(), triple.as_bytes()] {
            artifact.extend_from_slice(part);
            artifact.push(0);
        }
        let directory = klf_target::cache_root()?
            .join("optimizer")
            .join(&hex(&sha256(&artifact))[..16]);
        std::fs::create_dir_all(&directory).ok()?;
        Some(Self {
            directory,
            identity,
            used: Mutex::new(HashSet::new()),
        })
    }

    fn path(&self, key: &[u8; 32]) -> PathBuf {
        self.directory.join(hex(key))
    }

    fn note(&self, key: [u8; 32]) {
        self.used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key);
    }

    /// Removes every entry this build did not use. The cache only saves
    /// work, so what cannot be removed stays.
    pub(crate) fn settle(self) {
        let used = self
            .used
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let names: HashSet<String> = used.iter().map(|key| hex(key)).collect();
        let Ok(entries) = std::fs::read_dir(&self.directory) else {
            return;
        };
        for entry in entries.flatten() {
            if !names.contains(entry.file_name().to_string_lossy().as_ref()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

impl klf_kir_passes::Memo for OptimizerCache {
    fn digest(&self, bytes: &[u8]) -> [u8; 32] {
        let mut seeded = Vec::with_capacity(self.identity.len() + 1 + bytes.len());
        seeded.extend_from_slice(&self.identity);
        seeded.push(0);
        seeded.extend_from_slice(bytes);
        sha256(&seeded)
    }

    fn get(&self, key: &[u8; 32]) -> Option<Vec<u8>> {
        std::fs::read(self.path(key)).ok()
    }

    fn put(&self, key: [u8; 32], entry: Vec<u8>) {
        let path = self.path(&key);
        self.note(key);
        if path.exists() {
            return;
        }
        // Written aside and renamed, so a reader never sees half an entry.
        let partial = path.with_extension(format!("{}.partial", std::process::id()));
        if std::fs::write(&partial, entry).is_ok() && std::fs::rename(&partial, &path).is_err() {
            let _ = std::fs::remove_file(&partial);
        }
    }

    fn reuse(&self, key: &[u8; 32]) {
        self.note(*key);
    }
}
