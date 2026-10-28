//! The klf.process child services and the klf.fs entry services against VM
//! memory.

use klf_host_files::entries::{self, STATUS_WORDS};
use klf_host_files::handles::Failure as FileFailure;
use klf_host_process::children::{self, Failure, Signal, Stream};
use klf_rt::ServiceMemory;

use crate::Capabilities;

const INVALID_PATH: u64 = (Failure::InvalidPath as i64).cast_unsigned();
const INVALID_FILE_PATH: u64 = (FileFailure::InvalidPath as i64).cast_unsigned();

fn input(memory: &dyn ServiceMemory, address: u64, length: u64) -> Option<Vec<u8>> {
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

/// Copies `answer` to the buffer at `address` when it fits in `capacity`,
/// and answers its length either way.
fn deliver(memory: &mut dyn ServiceMemory, answer: &[u8], address: u64, capacity: u64) -> u64 {
    let fits = usize::try_from(capacity).is_ok_and(|capacity| answer.len() <= capacity);
    if fits && !answer.is_empty() && !memory.write_buffer(address, answer) {
        return INVALID_PATH;
    }
    answer.len() as u64
}

fn word(answer: i64) -> u64 {
    answer.cast_unsigned()
}

pub(crate) fn register(capabilities: &mut Capabilities) {
    register_commands(capabilities);
    register_children(capabilities);
    register_entries(capabilities);
    register_links(capabilities);
}

fn register_commands(capabilities: &mut Capabilities) {
    capabilities.register("klfsvc_process_spawn_create", |_| {
        word(children::spawn_create())
    });
    capabilities.register_with_memory("klfsvc_process_spawn_argument", |arguments, memory| {
        let [handle, data, length] = arguments else {
            return INVALID_PATH;
        };
        input(memory, *data, *length).map_or(INVALID_PATH, |argument| {
            word(children::spawn_argument(handle.cast_signed(), &argument))
        })
    });
    capabilities.register_with_memory("klfsvc_process_spawn_environment", |arguments, memory| {
        let [handle, data, length] = arguments else {
            return INVALID_PATH;
        };
        input(memory, *data, *length).map_or(INVALID_PATH, |entry| {
            word(children::spawn_environment(handle.cast_signed(), &entry))
        })
    });
    capabilities.register("klfsvc_process_spawn_clear_environment", |arguments| {
        let [handle] = arguments else {
            return INVALID_PATH;
        };
        word(children::spawn_clear_environment(handle.cast_signed()))
    });
    capabilities.register_with_memory("klfsvc_process_spawn_directory", |arguments, memory| {
        let [handle, data, length] = arguments else {
            return INVALID_PATH;
        };
        input(memory, *data, *length).map_or(INVALID_PATH, |path| {
            word(children::spawn_directory(handle.cast_signed(), &path))
        })
    });
    capabilities.register_with_memory("klfsvc_process_spawn_stream", |arguments, memory| {
        let [handle, stream, mode, path, length] = arguments else {
            return INVALID_PATH;
        };
        let target = input(memory, *path, *length)
            .and_then(|path| Stream::from_word(mode.cast_signed(), &path));
        target.map_or(INVALID_PATH, |target| {
            word(children::spawn_stream(
                handle.cast_signed(),
                stream.cast_signed(),
                target,
            ))
        })
    });
    capabilities.register("klfsvc_process_spawn_start", |arguments| {
        let [handle] = arguments else {
            return INVALID_PATH;
        };
        word(children::spawn_start(handle.cast_signed()))
    });
}

fn register_children(capabilities: &mut Capabilities) {
    capabilities.register("klfsvc_process_child_wait", |arguments| {
        let [handle, timeout] = arguments else {
            return INVALID_PATH;
        };
        word(children::child_wait(
            handle.cast_signed(),
            timeout.cast_signed(),
        ))
    });
    capabilities.register("klfsvc_process_child_wait_any", |arguments| {
        let [timeout] = arguments else {
            return INVALID_PATH;
        };
        word(children::child_wait_any(timeout.cast_signed()))
    });
    capabilities.register_with_memory("klfsvc_process_child_read", |arguments, memory| {
        let [handle, stream, destination, capacity, timeout] = arguments else {
            return INVALID_PATH;
        };
        let Some(capacity) = usize::try_from(*capacity)
            .ok()
            .filter(|capacity| *capacity <= klf_rt::MAX_SERVICE_BUFFER_BYTES)
        else {
            return INVALID_PATH;
        };
        let mut bytes = vec![0; capacity];
        let read = children::child_read(
            handle.cast_signed(),
            stream.cast_signed(),
            &mut bytes,
            timeout.cast_signed(),
        );
        if let Ok(filled) = usize::try_from(read)
            && filled != 0
            && !memory.write_buffer(*destination, &bytes[..filled])
        {
            return INVALID_PATH;
        }
        word(read)
    });
    capabilities.register_with_memory("klfsvc_process_child_write", |arguments, memory| {
        let [handle, data, length] = arguments else {
            return INVALID_PATH;
        };
        input(memory, *data, *length).map_or(INVALID_PATH, |data| {
            word(children::child_write(handle.cast_signed(), &data))
        })
    });
    capabilities.register("klfsvc_process_child_close_input", |arguments| {
        let [handle] = arguments else {
            return INVALID_PATH;
        };
        word(children::child_close_input(handle.cast_signed()))
    });
    capabilities.register("klfsvc_process_child_signal", |arguments| {
        let [handle, signal] = arguments else {
            return INVALID_PATH;
        };
        Signal::from_word(signal.cast_signed()).map_or(INVALID_PATH, |signal| {
            word(children::child_signal(handle.cast_signed(), signal))
        })
    });
    capabilities.register("klfsvc_process_child_release", |arguments| {
        let [handle] = arguments else {
            return INVALID_PATH;
        };
        word(children::child_release(handle.cast_signed()))
    });
    capabilities.register("klfsvc_process_processor_count", |_| {
        word(children::processor_count())
    });
    capabilities.register_with_memory("klfsvc_process_current_directory", |arguments, memory| {
        let [destination, capacity] = arguments else {
            return INVALID_PATH;
        };
        children::current_directory().map_or(INVALID_PATH, |directory| {
            deliver(memory, &directory, *destination, *capacity)
        })
    });
}

fn register_entries(capabilities: &mut Capabilities) {
    capabilities.register_with_memory("klfsvc_fs_status", |arguments, memory| {
        let [path, length, follow, record] = arguments else {
            return INVALID_FILE_PATH;
        };
        let Some(path) = input(memory, *path, *length) else {
            return INVALID_FILE_PATH;
        };
        let mut words = [0_i64; STATUS_WORDS];
        let answer = entries::status(&path, *follow == 1, &mut words);
        if answer == 1 {
            let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            if !memory.write_buffer(*record, &bytes) {
                return INVALID_FILE_PATH;
            }
        }
        word(answer)
    });
    capabilities.register_with_memory("klfsvc_fs_set_mode", |arguments, memory| {
        let [path, length, bits] = arguments else {
            return INVALID_FILE_PATH;
        };
        input(memory, *path, *length).map_or(INVALID_FILE_PATH, |path| {
            word(entries::set_mode(&path, bits.cast_signed()))
        })
    });
    capabilities.register_with_memory("klfsvc_fs_set_length", |arguments, memory| {
        let [path, length, size] = arguments else {
            return INVALID_FILE_PATH;
        };
        input(memory, *path, *length).map_or(INVALID_FILE_PATH, |path| {
            word(entries::set_length(&path, size.cast_signed()))
        })
    });
    capabilities.register_with_memory("klfsvc_fs_read_link", |arguments, memory| {
        let [path, length, destination, capacity] = arguments else {
            return INVALID_FILE_PATH;
        };
        let Some(path) = input(memory, *path, *length) else {
            return INVALID_FILE_PATH;
        };
        match entries::read_link(&path) {
            Ok(target) => deliver(memory, &target, *destination, *capacity),
            Err(failure) => word(failure as i64),
        }
    });
}

/// An entry service that takes two paths.
type TwoPaths = fn(&[u8], &[u8]) -> i64;

/// The entry services that take two paths.
fn register_links(capabilities: &mut Capabilities) {
    let pairs: [(&str, TwoPaths); 3] = [
        ("klfsvc_fs_symbolic_link", entries::symbolic_link),
        ("klfsvc_fs_hard_link", entries::hard_link),
        ("klfsvc_fs_copy", entries::copy),
    ];
    for (name, operation) in pairs {
        capabilities.register_with_memory(name, move |arguments, memory| {
            let [first, first_length, second, second_length] = arguments else {
                return INVALID_FILE_PATH;
            };
            match (
                input(memory, *first, *first_length),
                input(memory, *second, *second_length),
            ) {
                (Some(first), Some(second)) => word(operation(&first, &second)),
                _ => INVALID_FILE_PATH,
            }
        });
    }
}
