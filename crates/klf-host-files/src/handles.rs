//! Open files the program holds across service calls, named by handle.
//!
//! A handle is a positive word; failures are negative words from
//! [`Failure`], so one result word carries either. Reads and writes are
//! positional and take the table lock only to find the file, so two threads
//! reading one file do not wait on each other.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::path;

/// Why a handle operation failed, as the negative word it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum Failure {
    Missing = -1,
    Denied = -2,
    Exists = -3,
    InvalidHandle = -4,
    StorageFull = -5,
    InvalidPath = -6,
    Io = -7,
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::Missing,
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => Self::Denied,
            io::ErrorKind::AlreadyExists => Self::Exists,
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Self::StorageFull,
            io::ErrorKind::InvalidInput | io::ErrorKind::InvalidFilename => Self::InvalidPath,
            _ => Self::Io,
        }
    }
}

/// How `open` reaches the file, by the word the service passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    ReadWrite,
    CreateNew,
}

impl Access {
    #[must_use]
    pub fn from_word(word: i64) -> Option<Self> {
        match word {
            0 => Some(Self::Read),
            1 => Some(Self::ReadWrite),
            2 => Some(Self::CreateNew),
            _ => None,
        }
    }

    fn options(self) -> fs::OpenOptions {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        match self {
            Self::Read => {}
            Self::ReadWrite => {
                options.write(true).create(true);
            }
            Self::CreateNew => {
                options.write(true).create_new(true);
            }
        }
        options
    }
}

static TABLE: RwLock<Vec<Option<Arc<fs::File>>>> = RwLock::new(Vec::new());

fn word(result: Result<i64, Failure>) -> i64 {
    result.unwrap_or_else(|failure| failure as i64)
}

fn file(handle: i64) -> Result<Arc<fs::File>, Failure> {
    let index = usize::try_from(handle - 1).map_err(|_| Failure::InvalidHandle)?;
    let table = TABLE.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    table
        .get(index)
        .and_then(Option::clone)
        .ok_or(Failure::InvalidHandle)
}

fn length(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// Opens `path` and answers its handle, or a negative [`Failure`].
#[must_use]
pub fn open(path_bytes: &[u8], access: Access) -> i64 {
    word((|| {
        let path = path(path_bytes).ok_or(Failure::InvalidPath)?;
        let opened = Arc::new(access.options().open(path)?);
        let mut table = TABLE.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = if let Some(free) = table.iter().position(Option::is_none) {
            table[free] = Some(opened);
            free
        } else {
            table.push(Some(opened));
            table.len() - 1
        };
        Ok(length(index + 1))
    })())
}

/// Closes a handle; the file closes once no read or write still holds it.
#[must_use]
pub fn close(handle: i64) -> i64 {
    word((|| {
        let index = usize::try_from(handle - 1).map_err(|_| Failure::InvalidHandle)?;
        let mut table = TABLE.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        let slot = table.get_mut(index).ok_or(Failure::InvalidHandle)?;
        slot.take().ok_or(Failure::InvalidHandle)?;
        Ok(1)
    })())
}

/// Closes an operating-system descriptor that a library the program calls
/// handed it, such as a dma-buf a graphics driver exported. No `open`
/// numbered it, so it is not in the table; answers 1 once closed (D-392).
#[must_use]
pub fn close_descriptor(descriptor: i64) -> i64 {
    word(close_raw(descriptor))
}

// Closing a descriptor the program was handed asserts that it owns it,
// which no safe API can express; the one call is the whole unsafe region.
#[cfg(unix)]
#[allow(unsafe_code)]
fn close_raw(descriptor: i64) -> Result<i64, Failure> {
    unsafe extern "C" {
        fn close(descriptor: i32) -> i32;
    }
    let raw = i32::try_from(descriptor).map_err(|_| Failure::InvalidHandle)?;
    if raw < 0 {
        return Err(Failure::InvalidHandle);
    }
    // SAFETY: `close` takes any integer and reports a descriptor that is not
    // open as EBADF; the caller gives up the descriptor it names.
    if unsafe { close(raw) } == 0 {
        return Ok(1);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(9) {
        return Err(Failure::InvalidHandle);
    }
    Err(Failure::from(error))
}

#[cfg(not(unix))]
fn close_raw(_descriptor: i64) -> Result<i64, Failure> {
    Err(Failure::InvalidHandle)
}

/// Reads up to `destination.len()` bytes at `offset`; fewer only at the end.
#[must_use]
pub fn read_at(handle: i64, offset: i64, destination: &mut [u8]) -> i64 {
    word((|| {
        let file = file(handle)?;
        let mut position = u64::try_from(offset).map_err(|_| Failure::Io)?;
        let mut filled = 0;
        while filled < destination.len() {
            match positional::read(&file, &mut destination[filled..], position) {
                Ok(0) => break,
                Ok(read) => {
                    filled += read;
                    position += read as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(length(filled))
    })())
}

/// Writes all of `data` at `offset`, growing the file when it ends first.
#[must_use]
pub fn write_at(handle: i64, offset: i64, data: &[u8]) -> i64 {
    word((|| {
        let file = file(handle)?;
        let mut position = u64::try_from(offset).map_err(|_| Failure::Io)?;
        let mut written = 0;
        while written < data.len() {
            match positional::write(&file, &data[written..], position) {
                Ok(0) => return Err(Failure::StorageFull),
                Ok(count) => {
                    written += count;
                    position += count as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(length(written))
    })())
}

/// Makes the file's data durable, and its size when it changed.
#[must_use]
pub fn sync(handle: i64) -> i64 {
    word((|| {
        file(handle)?.sync_data()?;
        Ok(1)
    })())
}

/// Takes the exclusive lock of the open file `handle`: 1 once held, 0 when
/// another holder has it and `wait` is 0. Closing the handle, or the process
/// ending, releases it.
#[must_use]
pub fn lock(handle: i64, wait: i64) -> i64 {
    word((|| {
        let file = file(handle)?;
        if wait != 0 {
            file.lock()?;
            return Ok(1);
        }
        match file.try_lock() {
            Ok(()) => Ok(1),
            Err(fs::TryLockError::WouldBlock) => Ok(0),
            Err(fs::TryLockError::Error(error)) => Err(error.into()),
        }
    })())
}

#[must_use]
pub fn truncate(handle: i64, size: i64) -> i64 {
    word((|| {
        let size = u64::try_from(size).map_err(|_| Failure::Io)?;
        file(handle)?.set_len(size)?;
        Ok(1)
    })())
}

#[must_use]
pub fn size(handle: i64) -> i64 {
    word((|| {
        let size = file(handle)?.metadata()?.len();
        Ok(i64::try_from(size).unwrap_or(i64::MAX))
    })())
}

/// Makes a directory's entries durable, so a file created or renamed in it
/// survives a crash. Windows journals entries itself and has nothing to do.
#[must_use]
pub fn sync_directory(path_bytes: &[u8]) -> i64 {
    word((|| {
        let path = path(path_bytes).ok_or(Failure::InvalidPath)?;
        directory::sync(&path)?;
        Ok(1)
    })())
}

#[cfg(unix)]
mod positional {
    use std::os::unix::fs::FileExt;

    pub(super) fn read(file: &std::fs::File, into: &mut [u8], at: u64) -> std::io::Result<usize> {
        file.read_at(into, at)
    }

    pub(super) fn write(file: &std::fs::File, from: &[u8], at: u64) -> std::io::Result<usize> {
        file.write_at(from, at)
    }
}

#[cfg(windows)]
mod positional {
    use std::os::windows::fs::FileExt;

    pub(super) fn read(file: &std::fs::File, into: &mut [u8], at: u64) -> std::io::Result<usize> {
        file.seek_read(into, at)
    }

    pub(super) fn write(file: &std::fs::File, from: &[u8], at: u64) -> std::io::Result<usize> {
        file.seek_write(from, at)
    }
}

#[cfg(not(any(unix, windows)))]
mod positional {
    pub(super) fn read(_: &std::fs::File, _: &mut [u8], _: u64) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    pub(super) fn write(_: &std::fs::File, _: &[u8], _: u64) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

#[cfg(unix)]
mod directory {
    pub(super) fn sync(path: &super::Path) -> std::io::Result<()> {
        std::fs::File::open(path)?.sync_all()
    }
}

#[cfg(not(unix))]
mod directory {
    pub(super) fn sync(path: &super::Path) -> std::io::Result<()> {
        if path.is_dir() {
            Ok(())
        } else {
            Err(std::io::ErrorKind::NotFound.into())
        }
    }
}
