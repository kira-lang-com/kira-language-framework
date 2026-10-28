use klf_host_files::handles::{self, Access, Failure};

use crate::Capabilities;

fn input(memory: &dyn klf_rt::ServiceMemory, address: u64, length: u64) -> Option<Vec<u8>> {
    let length = usize::try_from(length).ok()?;
    if length > klf_rt::MAX_SERVICE_BUFFER_BYTES {
        return None;
    }
    let mut bytes = vec![0; length];
    if length != 0 && !memory.read_buffer(address, &mut bytes) {
        return None;
    }
    Some(bytes)
}

pub(crate) fn register(capabilities: &mut Capabilities) {
    capabilities.register_with_memory("klfsvc_fs_read", |arguments, memory| {
        let [path, path_len, offset, destination, count] = arguments else {
            return 0;
        };
        let Some(path) = input(memory, *path, *path_len) else {
            return 0;
        };
        let Ok(count) = usize::try_from(*count) else {
            return 0;
        };
        if count > klf_rt::MAX_SERVICE_BUFFER_BYTES {
            return 0;
        }
        let mut bytes = vec![0; count];
        let read = klf_host_files::read(&path, offset.cast_signed(), &mut bytes);
        if read != 0 && !memory.write_buffer(*destination, &bytes[..read]) {
            return 0;
        }
        u64::try_from(read).unwrap_or(u64::MAX)
    });

    capabilities.register_with_memory("klfsvc_fs_write", |arguments, memory| {
        let [path, path_len, data, count, append] = arguments else {
            return 0;
        };
        let Some(path) = input(memory, *path, *path_len) else {
            return 0;
        };
        let Some(data) = input(memory, *data, *count) else {
            return 0;
        };
        u64::from(klf_host_files::write(&path, &data, *append != 0))
    });

    capabilities.register_with_memory("klfsvc_fs_list", |arguments, memory| {
        let [path, path_len, destination, capacity] = arguments else {
            return u64::MAX;
        };
        let Some(path) = input(memory, *path, *path_len) else {
            return u64::MAX;
        };
        let listing = klf_host_files::list(&path);
        let length = u64::try_from(listing.len()).unwrap_or(u64::MAX);
        if usize::try_from(*capacity).is_ok_and(|capacity| listing.len() <= capacity)
            && !listing.is_empty()
            && !memory.write_buffer(*destination, &listing)
        {
            return u64::MAX;
        }
        length
    });

    macro_rules! predicate {
        ($symbol:literal, $function:path) => {
            capabilities.register_with_memory($symbol, |arguments, memory| {
                let [path, length] = arguments else { return 0 };
                let Some(path) = input(memory, *path, *length) else {
                    return 0;
                };
                u64::from($function(&path))
            });
        };
    }
    predicate!("klfsvc_fs_file_exists", klf_host_files::file_exists);
    predicate!("klfsvc_fs_path_exists", klf_host_files::path_exists);
    predicate!("klfsvc_fs_is_directory", klf_host_files::is_directory);
    predicate!("klfsvc_fs_make_directory", klf_host_files::make_directory);
    predicate!("klfsvc_fs_remove_path", klf_host_files::remove_path);

    capabilities.register_with_memory("klfsvc_fs_file_size", |arguments, memory| {
        let [path, length] = arguments else { return 0 };
        let Some(path) = input(memory, *path, *length) else {
            return 0;
        };
        klf_host_files::file_size(&path)
    });
    capabilities.register_with_memory("klfsvc_fs_rename", |arguments, memory| {
        let [from, from_len, to, to_len] = arguments else {
            return 0;
        };
        let Some(from) = input(memory, *from, *from_len) else {
            return 0;
        };
        let Some(to) = input(memory, *to, *to_len) else {
            return 0;
        };
        u64::from(klf_host_files::rename(&from, &to))
    });
    register_handles(capabilities);
}

fn failure(failure: Failure) -> u64 {
    (failure as i64).cast_unsigned()
}

fn register_handles(capabilities: &mut Capabilities) {
    capabilities.register_with_memory("klfsvc_fs_open", |arguments, memory| {
        let [path, length, access] = arguments else {
            return failure(Failure::InvalidPath);
        };
        let Some(path) = input(memory, *path, *length) else {
            return failure(Failure::InvalidPath);
        };
        let Some(access) = Access::from_word(access.cast_signed()) else {
            return failure(Failure::InvalidPath);
        };
        handles::open(&path, access).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_fs_read_at", |arguments, memory| {
        let [handle, offset, destination, count] = arguments else {
            return failure(Failure::Io);
        };
        let Ok(count) = usize::try_from(*count) else {
            return failure(Failure::Io);
        };
        if count > klf_rt::MAX_SERVICE_BUFFER_BYTES {
            return failure(Failure::Io);
        }
        let mut bytes = vec![0; count];
        let read = handles::read_at(handle.cast_signed(), offset.cast_signed(), &mut bytes);
        let Ok(filled) = usize::try_from(read) else {
            return read.cast_unsigned();
        };
        if filled != 0 && !memory.write_buffer(*destination, &bytes[..filled]) {
            return failure(Failure::Io);
        }
        read.cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_fs_write_at", |arguments, memory| {
        let [handle, offset, data, count] = arguments else {
            return failure(Failure::Io);
        };
        let Some(data) = input(memory, *data, *count) else {
            return failure(Failure::Io);
        };
        handles::write_at(handle.cast_signed(), offset.cast_signed(), &data).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_fs_sync_directory", |arguments, memory| {
        let [path, length] = arguments else {
            return failure(Failure::InvalidPath);
        };
        let Some(path) = input(memory, *path, *length) else {
            return failure(Failure::InvalidPath);
        };
        handles::sync_directory(&path).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_fs_close", |arguments, _| {
        let [handle] = arguments else {
            return failure(Failure::InvalidHandle);
        };
        handles::close(handle.cast_signed()).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_fs_close_descriptor", |arguments, _| {
        let [descriptor] = arguments else {
            return failure(Failure::InvalidHandle);
        };
        handles::close_descriptor(descriptor.cast_signed()).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_fs_sync", |arguments, _| {
        let [handle] = arguments else {
            return failure(Failure::InvalidHandle);
        };
        handles::sync(handle.cast_signed()).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_fs_lock", |arguments, _| {
        let [handle, wait] = arguments else {
            return failure(Failure::InvalidHandle);
        };
        handles::lock(handle.cast_signed(), wait.cast_signed()).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_fs_handle_size", |arguments, _| {
        let [handle] = arguments else {
            return failure(Failure::InvalidHandle);
        };
        handles::size(handle.cast_signed()).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_fs_truncate", |arguments, _| {
        let [handle, size] = arguments else {
            return failure(Failure::InvalidHandle);
        };
        handles::truncate(handle.cast_signed(), size.cast_signed()).cast_unsigned()
    });
}
