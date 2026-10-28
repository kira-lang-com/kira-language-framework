//! The host side of the `klf.fs` kernel services: the process's file system,
//! reached through paths given as bytes.
//!
//! The VM's capability table and the native runtime's `klfsvc_fs_*` symbols
//! both answer through these functions, so a program sees one file system
//! whichever backend runs it. Every operation answers a question about the
//! outside world, and the outside world may say no: a missing file reads as
//! no bytes, a failed write or removal as `false`. None of them traps.
//!
//! A path is UTF-8 on every host but Unix, where any byte string names a
//! file; a path that is not a valid name on this host names nothing.

pub mod entries;
pub mod handles;

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

#[cfg_attr(
    unix,
    expect(clippy::unnecessary_wraps, reason = "bytes name no path off Unix")
)]
fn path(bytes: &[u8]) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(std::ffi::OsString::from_vec(bytes.to_vec()).into())
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes).ok().map(PathBuf::from)
    }
}

/// Reads up to `destination.len()` bytes of the file at `path`, starting at
/// `offset`, and answers how many it read.
#[must_use]
pub fn read(path_bytes: &[u8], offset: i64, destination: &mut [u8]) -> usize {
    let Some(path) = path(path_bytes) else {
        return 0;
    };
    let Ok(offset) = u64::try_from(offset) else {
        return 0;
    };
    let Ok(mut file) = fs::File::open(path) else {
        return 0;
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return 0;
    }
    file.read(destination).unwrap_or(0)
}

/// Writes `data` to `path`; when `append` is true it is appended.
#[must_use]
pub fn write(path_bytes: &[u8], data: &[u8], append: bool) -> bool {
    let Some(path) = path(path_bytes) else {
        return false;
    };
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true);
    if append {
        options.append(true);
    } else {
        options.truncate(true);
    }
    options
        .open(path)
        .and_then(|mut file| file.write_all(data))
        .is_ok()
}

/// Directory entries encoded as newline-separated host names.
#[must_use]
pub fn list(path_bytes: &[u8]) -> Vec<u8> {
    let Some(path) = path(path_bytes) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            names.extend_from_slice(entry.file_name().as_bytes());
        }
        #[cfg(not(unix))]
        {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            names.extend_from_slice(name.as_bytes());
        }
        names.push(0);
    }
    names
}

#[must_use]
pub fn file_exists(path_bytes: &[u8]) -> bool {
    path(path_bytes).is_some_and(|path| path.is_file())
}

#[must_use]
pub fn path_exists(path_bytes: &[u8]) -> bool {
    path(path_bytes).is_some_and(|path| path.exists())
}

#[must_use]
pub fn is_directory(path_bytes: &[u8]) -> bool {
    path(path_bytes).is_some_and(|path| path.is_dir())
}

#[must_use]
pub fn file_size(path_bytes: &[u8]) -> u64 {
    path(path_bytes)
        .and_then(|path| fs::metadata(path).ok())
        .map_or(0, |metadata| metadata.len())
}

#[must_use]
pub fn make_directory(path_bytes: &[u8]) -> bool {
    // One directory, not its parents; one already there is made.
    path(path_bytes).is_some_and(|path| fs::create_dir(&path).is_ok() || path.is_dir())
}

#[must_use]
pub fn remove_path(path_bytes: &[u8]) -> bool {
    let Some(path) = path(path_bytes) else {
        return false;
    };
    if path.is_dir() {
        fs::remove_dir_all(path).is_ok()
    } else {
        fs::remove_file(path).is_ok()
    }
}

#[must_use]
pub fn rename(from: &[u8], to: &[u8]) -> bool {
    match (path(from), path(to)) {
        (Some(from), Some(to)) => fs::rename(from, to).is_ok(),
        _ => false,
    }
}
