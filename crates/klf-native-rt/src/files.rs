//! Native ABI shims for the klf.fs kernel services.

use klf_host_files::handles::{self, Access, Failure};

mod entries;

fn bytes<'a>(pointer: *const u8, length: i64) -> Option<&'a [u8]> {
    let length = usize::try_from(length).ok()?;
    if length == 0 {
        return Some(&[]);
    }
    if pointer.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(pointer, length) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_read(
    path: *const u8,
    path_length: i64,
    offset: i64,
    destination: *mut u8,
    count: i64,
) -> i64 {
    let Some(path) = bytes(path, path_length) else {
        return 0;
    };
    let Ok(count) = usize::try_from(count) else {
        return 0;
    };
    if count != 0 && destination.is_null() {
        return 0;
    }
    let output = if count == 0 {
        &mut [][..]
    } else {
        unsafe { std::slice::from_raw_parts_mut(destination, count) }
    };
    i64::try_from(klf_host_files::read(path, offset, output)).unwrap_or(i64::MAX)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_write(
    path: *const u8,
    path_length: i64,
    data: *const u8,
    count: i64,
    append: i64,
) -> i64 {
    let Some(path) = bytes(path, path_length) else {
        return 0;
    };
    let Some(data) = bytes(data, count) else {
        return 0;
    };
    i64::from(klf_host_files::write(path, data, append != 0))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_list(
    path: *const u8,
    path_length: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    let Some(path) = bytes(path, path_length) else {
        return 0;
    };
    let listing = klf_host_files::list(path);
    let length = i64::try_from(listing.len()).unwrap_or(i64::MAX);
    let Ok(capacity) = usize::try_from(capacity) else {
        return length;
    };
    if listing.len() <= capacity && !listing.is_empty() && !destination.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(listing.as_ptr(), destination, listing.len()) };
    }
    length
}

macro_rules! path_predicate {
    ($name:ident, $host:path) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(path: *const u8, length: i64) -> i64 {
            let Some(path) = bytes(path, length) else {
                return 0;
            };
            i64::from($host(path))
        }
    };
}

path_predicate!(klfsvc_fs_file_exists, klf_host_files::file_exists);
path_predicate!(klfsvc_fs_path_exists, klf_host_files::path_exists);
path_predicate!(klfsvc_fs_is_directory, klf_host_files::is_directory);
path_predicate!(klfsvc_fs_make_directory, klf_host_files::make_directory);
path_predicate!(klfsvc_fs_remove_path, klf_host_files::remove_path);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_file_size(path: *const u8, length: i64) -> i64 {
    let Some(path) = bytes(path, length) else {
        return 0;
    };
    i64::try_from(klf_host_files::file_size(path)).unwrap_or(i64::MAX)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_rename(
    from: *const u8,
    from_length: i64,
    to: *const u8,
    to_length: i64,
) -> i64 {
    let Some(from) = bytes(from, from_length) else {
        return 0;
    };
    let Some(to) = bytes(to, to_length) else {
        return 0;
    };
    i64::from(klf_host_files::rename(from, to))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_open(path: *const u8, length: i64, access: i64) -> i64 {
    let Some(path) = bytes(path, length) else {
        return Failure::InvalidPath as i64;
    };
    let Some(access) = Access::from_word(access) else {
        return Failure::InvalidPath as i64;
    };
    handles::open(path, access)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_fs_close(handle: i64) -> i64 {
    handles::close(handle)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_fs_close_descriptor(descriptor: i64) -> i64 {
    handles::close_descriptor(descriptor)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_read_at(
    handle: i64,
    offset: i64,
    destination: *mut u8,
    count: i64,
) -> i64 {
    let Ok(count) = usize::try_from(count) else {
        return Failure::Io as i64;
    };
    if count == 0 {
        return 0;
    }
    if destination.is_null() {
        return Failure::Io as i64;
    }
    let output = unsafe { std::slice::from_raw_parts_mut(destination, count) };
    handles::read_at(handle, offset, output)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_write_at(
    handle: i64,
    offset: i64,
    data: *const u8,
    count: i64,
) -> i64 {
    let Some(data) = bytes(data, count) else {
        return Failure::Io as i64;
    };
    handles::write_at(handle, offset, data)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_fs_sync(handle: i64) -> i64 {
    handles::sync(handle)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_fs_lock(handle: i64, wait: i64) -> i64 {
    handles::lock(handle, wait)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_fs_truncate(handle: i64, size: i64) -> i64 {
    handles::truncate(handle, size)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_fs_handle_size(handle: i64) -> i64 {
    handles::size(handle)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_fs_sync_directory(path: *const u8, length: i64) -> i64 {
    let Some(path) = bytes(path, length) else {
        return Failure::InvalidPath as i64;
    };
    handles::sync_directory(path)
}
