//! What a path is and how it is linked: its status, its permission bits,
//! links to it, copies of it, and files made to a length.
//!
//! Every operation answers 1, a length, or a negative [`Failure`] word, as
//! the handle operations do.

use std::fs;

use crate::handles::Failure;
use crate::path;

/// The words `status` writes, in order.
pub const STATUS_WORDS: usize = 6;

/// What kind of entry a path names, as the first status word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum Kind {
    File = 1,
    Directory = 2,
    Link = 3,
    Other = 4,
}

fn word(result: Result<i64, Failure>) -> i64 {
    result.unwrap_or_else(|failure| failure as i64)
}

fn named(bytes: &[u8]) -> Result<std::path::PathBuf, Failure> {
    path(bytes).ok_or(Failure::InvalidPath)
}

fn kind(file_type: fs::FileType) -> Kind {
    if file_type.is_symlink() {
        Kind::Link
    } else if file_type.is_dir() {
        Kind::Directory
    } else if file_type.is_file() {
        Kind::File
    } else {
        Kind::Other
    }
}

fn nanos(time: std::io::Result<std::time::SystemTime>) -> i64 {
    time.ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| i64::try_from(since.as_nanos()).ok())
        .unwrap_or(0)
}

#[cfg(unix)]
fn identity(metadata: &fs::Metadata) -> (i64, i64, i64) {
    use std::os::unix::fs::MetadataExt;
    (
        i64::from(metadata.mode() & 0o7777),
        metadata.dev().cast_signed(),
        metadata.ino().cast_signed(),
    )
}

#[cfg(not(unix))]
fn identity(metadata: &fs::Metadata) -> (i64, i64, i64) {
    let mode = if metadata.permissions().readonly() { 0o444 } else { 0o644 };
    (mode, 0, 0)
}

/// What `path` is: its kind, size in bytes, permission bits, modification
/// time in nanoseconds since the Unix epoch, device and inode, written to
/// `record`. A link is followed when `follow` is true.
#[must_use]
pub fn status(path_bytes: &[u8], follow: bool, record: &mut [i64; STATUS_WORDS]) -> i64 {
    word((|| {
        let path = named(path_bytes)?;
        let metadata = if follow {
            fs::metadata(path)?
        } else {
            fs::symlink_metadata(path)?
        };
        let (mode, device, inode) = identity(&metadata);
        *record = [
            kind(metadata.file_type()) as i64,
            i64::try_from(metadata.len()).unwrap_or(i64::MAX),
            mode,
            nanos(metadata.modified()),
            device,
            inode,
        ];
        Ok(1)
    })())
}

/// Sets the permission bits of `path`.
#[must_use]
pub fn set_mode(path_bytes: &[u8], bits: i64) -> i64 {
    word((|| {
        let path = named(path_bytes)?;
        let bits = u32::try_from(bits & 0o7777).map_err(|_| Failure::InvalidPath)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(bits))?;
        }
        #[cfg(not(unix))]
        {
            let mut permissions = fs::metadata(&path)?.permissions();
            permissions.set_readonly(bits & 0o222 == 0);
            fs::set_permissions(path, permissions)?;
        }
        Ok(1)
    })())
}

/// Makes `path` a symbolic link to `target`.
#[must_use]
pub fn symbolic_link(target: &[u8], path_bytes: &[u8]) -> i64 {
    word((|| {
        let target = named(target)?;
        let path = named(path_bytes)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, path)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, path)?;
        Ok(1)
    })())
}

/// The target of the symbolic link at `path`, as bytes.
///
/// # Errors
/// The [`Failure`] the host reports when `path` is not a readable link.
pub fn read_link(path_bytes: &[u8]) -> Result<Vec<u8>, Failure> {
    let target = fs::read_link(named(path_bytes)?)?;
    Ok(target.into_os_string().into_encoded_bytes())
}

/// Gives the file at `from` the second name `to`.
#[must_use]
pub fn hard_link(from: &[u8], to: &[u8]) -> i64 {
    word((|| {
        fs::hard_link(named(from)?, named(to)?)?;
        Ok(1)
    })())
}

/// Copies the file at `from` to `to` with its permission bits, through the
/// filesystem's own copy where it has one, and answers the bytes copied.
#[must_use]
pub fn copy(from: &[u8], to: &[u8]) -> i64 {
    word((|| {
        let copied = fs::copy(named(from)?, named(to)?)?;
        Ok(i64::try_from(copied).unwrap_or(i64::MAX))
    })())
}

/// Makes `path` a file of `length` bytes, creating it when it is missing.
/// Bytes it gains read as zero and take no space where the filesystem
/// allows holes.
#[must_use]
pub fn set_length(path_bytes: &[u8], length: i64) -> i64 {
    word((|| {
        let length = u64::try_from(length).map_err(|_| Failure::InvalidPath)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(named(path_bytes)?)?;
        file.set_len(length)?;
        Ok(1)
    })())
}
