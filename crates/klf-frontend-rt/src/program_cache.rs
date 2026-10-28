//! A run's executable, kept under the cache root (section 5.6), is reused
//! whole while nothing it was built from changed.
//!
//! Beside the executable a stamp keeps the key of the build (release,
//! definition, frontend, target and profile, as the query cache keys its
//! entries) and every input with what it held: each path the compile read,
//! asked the existence of or listed (the paths `--depfile` names, section
//! 5.5), and the archives the program links. A file holds its contents'
//! SHA-256, a directory the names under it, a missing path its absence.
//! While every input still holds, the executable is the one the build would
//! link again, and the run starts without compiling. An input changed while
//! the build ran may differ from what the compile read, so such a build keeps
//! no stamp and the next run compiles again.

use klf_ridl::sha256::sha256;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

/// When this build started: `reusable` runs before the compile reads
/// anything.
static STARTED: OnceLock<SystemTime> = OnceLock::new();

/// What a path held when the program was built.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
enum Held {
    File([u8; 32]),
    Directory(Vec<PathBuf>),
    Missing,
}

#[derive(Deserialize, Serialize)]
struct Stamp {
    key: String,
    /// The executable's size and modification time once linked, so an
    /// executable another build wrote over is not taken for this one.
    executable: (u64, u128),
    inputs: Vec<(PathBuf, Held)>,
    /// The diagnostics the build kept, shown again when it is reused.
    warnings: String,
}

/// Where the stamp of `executable` is kept.
fn stamp_path(executable: &Path) -> PathBuf {
    let mut name = executable.as_os_str().to_owned();
    name.push(".stamp");
    PathBuf::from(name)
}

fn signature(path: &Path) -> Option<(u64, u128)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((metadata.len(), modified))
}

fn held(path: &Path) -> Held {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Held::Missing;
    };
    if metadata.is_dir() {
        let mut names: Vec<PathBuf> = std::fs::read_dir(path)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| PathBuf::from(entry.file_name()))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        return Held::Directory(names);
    }
    // A file that cannot be read holds nothing a build could reuse.
    std::fs::read(path).map_or(Held::Missing, |bytes| Held::File(sha256(&bytes)))
}

/// Whether `path` was written, or a directory's names changed, at or after
/// `started`.
fn changed_since(path: &Path, started: SystemTime) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| modified >= started)
}

/// The diagnostics the build of `executable` kept, when its stamp under
/// `key` still holds: the executable can run as it is.
pub(crate) fn reusable(executable: &Path, key: &str) -> Option<String> {
    STARTED.get_or_init(SystemTime::now);
    let bytes = std::fs::read(stamp_path(executable)).ok()?;
    let stamp: Stamp = postcard::from_bytes(&bytes).ok()?;
    if stamp.key != key || signature(executable)? != stamp.executable {
        return None;
    }
    let unchanged = std::thread::scope(|scope| {
        let workers = std::thread::available_parallelism().map_or(1, usize::from);
        let chunk = stamp.inputs.len().div_ceil(workers).max(1);
        let parts: Vec<_> = stamp
            .inputs
            .chunks(chunk)
            .map(|part| scope.spawn(move || part.iter().all(|(path, was)| held(path) == *was)))
            .collect();
        parts.into_iter().all(|part| part.join().unwrap_or(false))
    });
    unchanged.then_some(stamp.warnings)
}

/// Keeps the stamp of `executable`, just built under `key` from the paths
/// the compile touched and the archives in `links`. A stamp that cannot be
/// written only costs the next run its compile.
pub(crate) fn keep(executable: &Path, key: &str, links: &[PathBuf], warnings: &str) {
    let path = stamp_path(executable);
    let _ = std::fs::remove_file(&path);
    let Some(linked) = signature(executable) else {
        return;
    };
    let inputs: Vec<PathBuf> = klf_services::accessed()
        .into_iter()
        .chain(links.iter().cloned())
        .collect();
    if let Some(started) = STARTED.get()
        && inputs.iter().any(|input| changed_since(input, *started))
    {
        return;
    }
    let inputs = inputs
        .into_iter()
        .map(|input| {
            let was = held(&input);
            (input, was)
        })
        .collect();
    let stamp = Stamp {
        key: key.to_owned(),
        executable: linked,
        inputs,
        warnings: warnings.to_owned(),
    };
    let Ok(bytes) = postcard::to_stdvec(&stamp) else {
        return;
    };
    let staged = path.with_extension(format!("stamp.{}.partial", std::process::id()));
    if std::fs::write(&staged, bytes).is_err() || std::fs::rename(&staged, &path).is_err() {
        let _ = std::fs::remove_file(&staged);
    }
}

#[cfg(test)]
mod tests {
    use super::changed_since;
    use std::time::{Duration, SystemTime};

    #[test]
    fn a_file_written_during_the_build_counts_as_changed() {
        let directory =
            std::env::temp_dir().join(format!("klf-program-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let started = SystemTime::now() - Duration::from_secs(1);
        let file = directory.join("added.kira");
        std::fs::write(&file, "x").unwrap();
        assert!(changed_since(&file, started));
        assert!(changed_since(&directory, started));
        let later = SystemTime::now() + Duration::from_secs(60);
        assert!(!changed_since(&file, later));
        assert!(!changed_since(&directory.join("missing"), started));
        let _ = std::fs::remove_dir_all(&directory);
    }
}
