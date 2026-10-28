//! Native ABI shims for the klf.process child services: commands built by
//! handle, started, waited for and read from.

use klf_host_process::children::{self, Failure, Signal, Stream};

use crate::net::{bytes, bytes_mut};

const INVALID_PATH: i64 = Failure::InvalidPath as i64;
const INVALID_HANDLE: i64 = Failure::InvalidHandle as i64;

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_spawn_create() -> i64 {
    children::spawn_create()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_spawn_argument(
    handle: i64,
    data: *const u8,
    length: i64,
) -> i64 {
    bytes(data, length).map_or(INVALID_PATH, |argument| {
        children::spawn_argument(handle, argument)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_spawn_environment(
    handle: i64,
    data: *const u8,
    length: i64,
) -> i64 {
    bytes(data, length).map_or(INVALID_PATH, |entry| {
        children::spawn_environment(handle, entry)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_spawn_clear_environment(handle: i64) -> i64 {
    children::spawn_clear_environment(handle)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_spawn_directory(
    handle: i64,
    data: *const u8,
    length: i64,
) -> i64 {
    bytes(data, length).map_or(INVALID_PATH, |path| children::spawn_directory(handle, path))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_spawn_stream(
    handle: i64,
    stream: i64,
    mode: i64,
    path: *const u8,
    length: i64,
) -> i64 {
    let Some(target) = bytes(path, length).and_then(|path| Stream::from_word(mode, path)) else {
        return INVALID_PATH;
    };
    children::spawn_stream(handle, stream, target)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_spawn_start(handle: i64) -> i64 {
    children::spawn_start(handle)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_child_wait(handle: i64, timeout: i64) -> i64 {
    children::child_wait(handle, timeout)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_child_wait_any(timeout: i64) -> i64 {
    children::child_wait_any(timeout)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_child_read(
    handle: i64,
    stream: i64,
    destination: *mut u8,
    capacity: i64,
    timeout: i64,
) -> i64 {
    bytes_mut(destination, capacity).map_or(INVALID_PATH, |destination| {
        children::child_read(handle, stream, destination, timeout)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_child_write(
    handle: i64,
    data: *const u8,
    length: i64,
) -> i64 {
    bytes(data, length).map_or(INVALID_PATH, |data| children::child_write(handle, data))
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_child_close_input(handle: i64) -> i64 {
    children::child_close_input(handle)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_child_signal(handle: i64, signal: i64) -> i64 {
    Signal::from_word(signal).map_or(INVALID_HANDLE, |signal| {
        children::child_signal(handle, signal)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_child_release(handle: i64) -> i64 {
    children::child_release(handle)
}

/// Kernel service `klf.process.processor_count`: how many threads the host
/// runs at once for this process.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_processor_count() -> i64 {
    children::processor_count()
}

/// Kernel service `klf.process.current_directory`: the byte length of the
/// working directory, copied to `destination` when it fits in `capacity`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_current_directory(
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    let Some(directory) = children::current_directory() else {
        return Failure::Io as i64;
    };
    let length = i64::try_from(directory.len()).unwrap_or(i64::MAX);
    if length <= capacity
        && let Some(room) = bytes_mut(destination, length)
    {
        room.copy_from_slice(&directory);
    }
    length
}
