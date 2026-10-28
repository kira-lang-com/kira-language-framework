//! Native ABI shims for the klf.fs entry services: status, permission bits,
//! links, copies and lengths.

use klf_host_files::entries::{self, STATUS_WORDS};
use klf_host_files::handles::Failure;

use crate::net::{bytes, bytes_mut};

const INVALID_PATH: i64 = Failure::InvalidPath as i64;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_status(
    path: *const u8,
    length: i64,
    follow: i64,
    record: *mut i64,
) -> i64 {
    let Some(path) = bytes(path, length) else {
        return INVALID_PATH;
    };
    if record.is_null() {
        return INVALID_PATH;
    }
    let mut words = [0_i64; STATUS_WORDS];
    let answer = entries::status(path, follow == 1, &mut words);
    if answer == 1 {
        // SAFETY: the caller hands a record of STATUS_WORDS words.
        unsafe { std::ptr::copy_nonoverlapping(words.as_ptr(), record, STATUS_WORDS) };
    }
    answer
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_set_mode(path: *const u8, length: i64, bits: i64) -> i64 {
    bytes(path, length).map_or(INVALID_PATH, |path| entries::set_mode(path, bits))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_symbolic_link(
    target: *const u8,
    target_length: i64,
    path: *const u8,
    path_length: i64,
) -> i64 {
    match (bytes(target, target_length), bytes(path, path_length)) {
        (Some(target), Some(path)) => entries::symbolic_link(target, path),
        _ => INVALID_PATH,
    }
}

/// Kernel service `klf.fs.read_link`: the byte length of the link's target,
/// copied to `destination` when it fits in `capacity`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_read_link(
    path: *const u8,
    length: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    let Some(path) = bytes(path, length) else {
        return INVALID_PATH;
    };
    match entries::read_link(path) {
        Ok(target) => {
            let length = i64::try_from(target.len()).unwrap_or(i64::MAX);
            if length <= capacity
                && let Some(room) = bytes_mut(destination, length)
            {
                room.copy_from_slice(&target);
            }
            length
        }
        Err(failure) => failure as i64,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_hard_link(
    from: *const u8,
    from_length: i64,
    to: *const u8,
    to_length: i64,
) -> i64 {
    match (bytes(from, from_length), bytes(to, to_length)) {
        (Some(from), Some(to)) => entries::hard_link(from, to),
        _ => INVALID_PATH,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_copy(
    from: *const u8,
    from_length: i64,
    to: *const u8,
    to_length: i64,
) -> i64 {
    match (bytes(from, from_length), bytes(to, to_length)) {
        (Some(from), Some(to)) => entries::copy(from, to),
        _ => INVALID_PATH,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_set_length(path: *const u8, length: i64, size: i64) -> i64 {
    bytes(path, length).map_or(INVALID_PATH, |path| entries::set_length(path, size))
}
