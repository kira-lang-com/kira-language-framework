//! Sub-queries whose answers are kept across builds (KLF-RIDL section 9.2).
//!
//! A language performing a pre-parse step inside `parse`, such as Kira's
//! macro expansion, keys each answer of the step by everything the answer
//! depends on: the SHA-256 of each file it observed (`klf.source.digest`)
//! and what it was asked. A later build asking the same finds the answer
//! again (`klf.subquery.recall`) instead of computing it. An answer is the
//! text of a source file, kept with `klf.subquery.keep`.
//!
//! Answers are kept only where the driver says, for the toolchain it names
//! ([`keep_subqueries`]): an answer computed by another toolchain may differ.

use crate::{DefinitionServiceState, ServiceSink};
use klf_rt::{MAX_SERVICE_BUFFER_BYTES, ServiceMemory};
use klf_ridl::sha256::{hex, sha256};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

const MISSING: u64 = u64::MAX;

/// Where answers are kept, and the toolchain every key starts from.
struct Store {
    directory: PathBuf,
    toolchain: String,
}

static STORE: OnceLock<Store> = OnceLock::new();

/// Keeps the answers of sub-queries under `directory` for the rest of the
/// process, each keyed by `toolchain` before its own key: the release,
/// definition and frontend that compute them.
pub fn keep_subqueries(directory: PathBuf, toolchain: String) {
    let _ = STORE.set(Store {
        directory,
        toolchain,
    });
}

pub(super) fn register(state: &Arc<Mutex<DefinitionServiceState>>, services: &mut ServiceSink) {
    services.register_state("klfsvc_source_digest", state, digest);
    services.register_state_with_memory("klfsvc_subquery_recall", state, recall);
    services.register_state_with_memory("klfsvc_subquery_keep", state, keep);
}

/// `(session, file, word) -> bits`: the 64 bits at `word`, 0 to 3, of the
/// SHA-256 of the file's text.
fn digest(state: &mut DefinitionServiceState, arguments: &[u64]) -> u64 {
    let [session, file, word] = arguments else {
        return MISSING;
    };
    let Ok(word) = usize::try_from(*word) else {
        return MISSING;
    };
    if word >= 4 {
        return MISSING;
    }
    let digest = if let Some(digest) = state.source_digests.get(file) {
        *digest
    } else {
        let Some(text) = state.source_of(&[*session, *file]) else {
            return MISSING;
        };
        let digest = sha256(text.as_bytes());
        state.source_digests.insert(*file, digest);
        digest
    };
    let mut bits = [0; 8];
    bits.copy_from_slice(&digest[word * 8..word * 8 + 8]);
    u64::from_le_bytes(bits)
}

/// `(session, origin, key, key length) -> file`: a new source beside
/// `origin` holding the answer kept under `key`, or -1 when there is none.
fn recall(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [session, origin, address, length] = arguments else {
        return MISSING;
    };
    if *session != state.session.as_u64() {
        return MISSING;
    }
    let Some(path) = read_text(memory, *address, *length).and_then(|key| answer_path(&key)) else {
        return MISSING;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return MISSING;
    };
    state
        .add_virtual_source(*origin, text)
        .map_or(MISSING, |file| u64::from(file.as_u32()))
}

/// `(session, key, key length, file)`: keeps the file's text as the answer
/// under `key`, when it can. A build that cannot keep it only costs a later
/// build the time to compute it again.
fn keep(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [session, address, length, file] = arguments else {
        return 0;
    };
    let path = read_text(memory, *address, *length).and_then(|key| answer_path(&key));
    if let (Some(path), Some(text)) = (path, state.source_of(&[*session, *file])) {
        let _ = write_answer(&path, text);
    }
    0
}

fn answer_path(key: &str) -> Option<PathBuf> {
    let store = STORE.get()?;
    let digest = sha256(format!("{}\0{key}", store.toolchain).as_bytes());
    let name = hex(&digest);
    Some(store.directory.join(&name[..2]).join(&name[2..]))
}

/// Writes `text` at `path` whole: a reader never sees an answer half
/// written.
fn write_answer(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let staged = path.with_extension(format!("{}.partial", std::process::id()));
    std::fs::write(&staged, text)?;
    std::fs::rename(&staged, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&staged);
    })
}

fn read_text(memory: &mut dyn ServiceMemory, address: u64, length: u64) -> Option<String> {
    let length = usize::try_from(length).ok()?;
    if length > MAX_SERVICE_BUFFER_BYTES {
        return None;
    }
    let mut bytes = vec![0; length];
    if !memory.read_buffer(address, &mut bytes) {
        return None;
    }
    String::from_utf8(bytes).ok()
}
