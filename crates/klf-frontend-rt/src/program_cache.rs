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
use std::collections::HashMap;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

/// When this build started: `reusable` runs before the compile reads
/// anything.
static STARTED: OnceLock<SystemTime> = OnceLock::new();

/// What a path held when the program was built.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
enum Held {
    File {
        signature: Option<FileSignature>,
        digest: [u8; 32],
    },
    Directory {
        signature: Option<FileSignature>,
        names: Vec<PathBuf>,
    },
    Missing,
}

/// The cheap identity checked before reading file contents again. Unix change
/// time and inode catch same-size edits even when a tool restores mtime.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FileSignature {
    length: u64,
    modified: u128,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: i128,
}

fn file_signature(metadata: &Metadata) -> Option<FileSignature> {
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
    Some(FileSignature {
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

#[derive(Default)]
struct ValidationWork {
    paths: usize,
    files: usize,
    directories: usize,
    app_directories: usize,
    tests_directories: usize,
    package_directories: usize,
    other_directories: usize,
    content_reads: usize,
    content_hashes: usize,
    directory_lists: usize,
}

impl ValidationWork {
    fn include(&mut self, other: Self) {
        self.paths += other.paths;
        self.files += other.files;
        self.directories += other.directories;
        self.app_directories += other.app_directories;
        self.tests_directories += other.tests_directories;
        self.package_directories += other.package_directories;
        self.other_directories += other.other_directories;
        self.content_reads += other.content_reads;
        self.content_hashes += other.content_hashes;
        self.directory_lists += other.directory_lists;
    }
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

fn held(
    path: &Path,
    started: Option<SystemTime>,
    directory_names: &mut HashMap<(u64, u64), Vec<PathBuf>>,
) -> (Held, bool) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return (Held::Missing, false);
    };
    let changed = started.is_some_and(|started| changed_metadata_since(&metadata, started));
    if changed {
        return (Held::Missing, true);
    }
    let signature = file_signature(&metadata);
    if metadata.is_dir() {
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            Some((metadata.dev(), metadata.ino()))
        };
        #[cfg(not(unix))]
        let identity: Option<(u64, u64)> = None;
        let read_names = || {
            let mut names: Vec<PathBuf> = std::fs::read_dir(path)
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|entry| PathBuf::from(entry.file_name()))
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        };
        let names = if let Some(identity) = identity {
            directory_names
                .entry(identity)
                .or_insert_with(read_names)
                .clone()
        } else {
            read_names()
        };
        return (Held::Directory { signature, names }, false);
    }
    // A file that cannot be read holds nothing a build could reuse.
    (
        std::fs::read(path).map_or(Held::Missing, |bytes| Held::File {
            signature,
            digest: sha256(&bytes),
        }),
        false,
    )
}

fn still_held(path: &Path, was: &Held, work: &mut ValidationWork) -> bool {
    work.paths += 1;
    let metadata = std::fs::metadata(path).ok();
    match was {
        Held::Missing => metadata.is_none(),
        Held::Directory { signature, names } => {
            work.directories += 1;
            match (path.file_name(), path.parent().and_then(Path::file_name)) {
                (Some(name), _) if name == "app" => work.app_directories += 1,
                (Some(name), _) if name == "tests" => work.tests_directories += 1,
                (_, Some(parent)) if parent == "packages" => work.package_directories += 1,
                _ => work.other_directories += 1,
            }
            let Some(metadata) = metadata.filter(Metadata::is_dir) else {
                return false;
            };
            let current = file_signature(&metadata);
            if signature.is_some() && current == *signature {
                return true;
            }
            work.directory_lists += 1;
            let mut current: Vec<PathBuf> = match std::fs::read_dir(path) {
                Ok(entries) => entries
                    .flatten()
                    .map(|entry| PathBuf::from(entry.file_name()))
                    .collect(),
                Err(_) => return false,
            };
            current.sort();
            current == *names
        }
        Held::File { signature, digest } => {
            work.files += 1;
            let Some(metadata) = metadata.filter(|metadata| metadata.is_file()) else {
                return false;
            };
            let current = file_signature(&metadata);
            if signature.is_some() && current == *signature {
                return true;
            }
            work.content_reads += 1;
            let Ok(bytes) = std::fs::read(path) else {
                return false;
            };
            work.content_hashes += 1;
            sha256(&bytes) == *digest
        }
    }
}

/// Whether `path` was written, or a directory's names changed, at or after
/// `started`.
#[cfg(test)]
fn changed_since(path: &Path, started: SystemTime) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    changed_metadata_since(&metadata, started)
}

fn changed_metadata_since(metadata: &Metadata, started: SystemTime) -> bool {
    if metadata
        .modified()
        .is_ok_and(|modified| modified >= started)
    {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Some(started) = started
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|value| i128::try_from(value.as_nanos()).ok())
        else {
            return false;
        };
        let changed =
            i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec());
        return changed >= started;
    }
    #[cfg(not(unix))]
    false
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
    let (unchanged, work) = std::thread::scope(|scope| {
        let workers = std::thread::available_parallelism().map_or(1, usize::from);
        let chunk = stamp.inputs.len().div_ceil(workers).max(1);
        let parts: Vec<_> = stamp
            .inputs
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    let mut work = ValidationWork::default();
                    let mut unchanged = true;
                    for (path, was) in part {
                        if !still_held(path, was, &mut work) {
                            unchanged = false;
                        }
                    }
                    (unchanged, work)
                })
            })
            .collect();
        let mut unchanged = true;
        let mut work = ValidationWork::default();
        for part in parts {
            match part.join() {
                Ok((holds, part_work)) => {
                    unchanged &= holds;
                    work.include(part_work);
                }
                Err(_) => unchanged = false,
            }
        }
        (unchanged, work)
    });
    klf_diag::progress!(
        "program cache validation paths={} files={} directories={} app_directories={} tests_directories={} package_directories={} other_directories={} content_reads={} content_hashes={} directory_lists={}",
        work.paths,
        work.files,
        work.directories,
        work.app_directories,
        work.tests_directories,
        work.package_directories,
        work.other_directories,
        work.content_reads,
        work.content_hashes,
        work.directory_lists
    );
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
    let started = STARTED.get().copied();
    let mut held_inputs = Vec::with_capacity(inputs.len());
    let mut directory_names = HashMap::new();
    for input in inputs {
        let (was, changed) = held(&input, started, &mut directory_names);
        if changed {
            return;
        }
        held_inputs.push((input, was));
    }
    let stamp = Stamp {
        key: key.to_owned(),
        executable: linked,
        inputs: held_inputs,
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
