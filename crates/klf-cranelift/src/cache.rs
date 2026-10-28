//! Machine code kept between builds of one artifact, by the hash Cranelift
//! takes of each function it compiles. A build starts from what the previous
//! build of the artifact left and keeps only the code it used, so the cache
//! never holds more than one build.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The hash Cranelift names a function's compiled code by.
pub(crate) type Key = [u8; 32];

const MAGIC: &[u8; 8] = b"KLFCC\0\0\x01";

pub struct CodeCache {
    path: PathBuf,
    entries: HashMap<Key, Vec<u8>>,
}

impl CodeCache {
    /// The cache for the artifact of `module` written to a file named as
    /// `output` is, for `triple`, under the KLF cache root; nothing when there
    /// is no cache root. The directory `output` is in is left out: a build
    /// system writes each build of an artifact into a directory named by its
    /// inputs, and the code of the functions that did not change is the
    /// same whichever directory the build was given.
    #[must_use]
    pub fn for_artifact(module: &str, output: &Path, triple: &str) -> Option<Self> {
        let file = output.file_name()?;
        let digest = fnv1a(
            module
                .as_bytes()
                .iter()
                .chain(b"\0")
                .chain(file.as_encoded_bytes())
                .chain(b"\0")
                .chain(triple.as_bytes()),
        );
        let path = klf_target::cache_root()?
            .join("cranelift")
            .join(format!("{digest:016x}.bin"));
        Some(Self::open(path))
    }

    /// The cache stored at `path`, empty when there is none or it cannot be
    /// read whole.
    #[must_use]
    pub fn open(path: PathBuf) -> Self {
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|bytes| decode(&bytes))
            .unwrap_or_default();
        Self { path, entries }
    }

    pub(crate) fn entries(&self) -> &HashMap<Key, Vec<u8>> {
        &self.entries
    }

    /// Keeps the code a build used and adds what it compiled anew.
    pub(crate) fn settle(&mut self, used: &HashSet<Key>, compiled: Vec<(Key, Vec<u8>)>) {
        self.entries.retain(|key, _| used.contains(key));
        self.entries.extend(compiled);
    }

    /// Writes the cache, replacing what an earlier build stored.
    ///
    /// # Errors
    /// Returns the I/O error when the cache cannot be written.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut keys: Vec<&Key> = self.entries.keys().collect();
        keys.sort_unstable();
        let mut bytes = MAGIC.to_vec();
        for key in keys {
            let code = &self.entries[key];
            let length = u32::try_from(code.len()).map_err(std::io::Error::other)?;
            bytes.extend_from_slice(key);
            bytes.extend_from_slice(&length.to_le_bytes());
            bytes.extend_from_slice(code);
        }
        let staged = self.path.with_extension("tmp");
        std::fs::write(&staged, bytes)?;
        std::fs::rename(staged, &self.path)
    }
}

/// A hash that stays the same across builds of the compiler, so an
/// artifact finds its cache again.
fn fnv1a<'a>(bytes: impl Iterator<Item = &'a u8>) -> u64 {
    bytes.fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn decode(bytes: &[u8]) -> Option<HashMap<Key, Vec<u8>>> {
    let mut rest = bytes.strip_prefix(MAGIC.as_slice())?;
    let mut entries = HashMap::new();
    while !rest.is_empty() {
        let (key, after) = rest.split_first_chunk::<32>()?;
        let (length, after) = after.split_first_chunk::<4>()?;
        let length = usize::try_from(u32::from_le_bytes(*length)).ok()?;
        let code = after.get(..length)?;
        entries.insert(*key, code.to_vec());
        rest = &after[length..];
    }
    Some(entries)
}
