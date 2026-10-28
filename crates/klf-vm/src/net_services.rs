//! The klf.net kernel services against VM memory.

use klf_host_net::{self as net, EVENT_WORDS, Failure};
use klf_rt::ServiceMemory;

use crate::Capabilities;

fn failure(failure: Failure) -> u64 {
    (failure as i64).cast_unsigned()
}

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

fn output(capacity: u64) -> Option<Vec<u8>> {
    let capacity = usize::try_from(capacity).ok()?;
    (capacity <= klf_rt::MAX_SERVICE_BUFFER_BYTES).then(|| vec![0; capacity])
}

/// Copies `bytes` back to the program when the answer says they were written.
fn copy_back(memory: &mut dyn ServiceMemory, address: u64, bytes: &[u8], answer: i64) -> u64 {
    if answer >= 0 && !bytes.is_empty() && !memory.write_buffer(address, bytes) {
        return failure(Failure::Io);
    }
    answer.cast_unsigned()
}

/// Copies a NUL-ended text back, NUL included, when the service wrote one;
/// a text that did not fit was not written and leaves the program's alone.
fn copy_ended(memory: &mut dyn ServiceMemory, address: u64, text: &[u8]) -> bool {
    let Some(end) = text.iter().position(|byte| *byte == 0) else {
        return true;
    };
    memory.write_buffer(address, &text[..=end])
}

pub(crate) fn register(capabilities: &mut Capabilities) {
    register_sockets(capabilities);
    register_streams(capabilities);
    register_waiting(capabilities);
}

fn register_sockets(capabilities: &mut Capabilities) {
    capabilities.register_with_memory("klfsvc_net_udp_bind", |arguments, memory| {
        let [address, length] = arguments else {
            return failure(Failure::InvalidAddress);
        };
        let Some(address) = input(memory, *address, *length) else {
            return failure(Failure::InvalidAddress);
        };
        net::udp_bind(&address).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_udp_send", |arguments, memory| {
        let [socket, peer, peer_length, data, count] = arguments else {
            return failure(Failure::Io);
        };
        let Some(peer) = input(memory, *peer, *peer_length) else {
            return failure(Failure::InvalidAddress);
        };
        let Some(data) = input(memory, *data, *count) else {
            return failure(Failure::Io);
        };
        net::udp_send(socket.cast_signed(), &peer, &data).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_udp_receive", |arguments, memory| {
        let [socket, destination, capacity, peer, peer_capacity] = arguments else {
            return failure(Failure::Io);
        };
        let (Some(mut data), Some(mut from)) = (output(*capacity), output(*peer_capacity)) else {
            return failure(Failure::Io);
        };
        let answer = net::udp_receive(socket.cast_signed(), &mut data, &mut from);
        let Ok(received) = usize::try_from(answer) else {
            return answer.cast_unsigned();
        };
        if !copy_ended(memory, *peer, &from) {
            return failure(Failure::Io);
        }
        copy_back(memory, *destination, &data[..received], answer)
    });
    capabilities.register_with_memory("klfsvc_net_local_address", |arguments, memory| {
        let [handle, destination, capacity] = arguments else {
            return failure(Failure::Io);
        };
        let Some(mut text) = output(*capacity) else {
            return failure(Failure::Io);
        };
        let answer = net::local_address(handle.cast_signed(), &mut text);
        if answer >= 0 && !copy_ended(memory, *destination, &text) {
            return failure(Failure::Io);
        }
        answer.cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_close", |arguments, _| {
        let [handle] = arguments else {
            return failure(Failure::InvalidHandle);
        };
        net::close(handle.cast_signed()).cast_unsigned()
    });
}

fn register_streams(capabilities: &mut Capabilities) {
    capabilities.register_with_memory("klfsvc_net_tcp_connect", |arguments, memory| {
        let [address, length] = arguments else {
            return failure(Failure::InvalidAddress);
        };
        let Some(address) = input(memory, *address, *length) else {
            return failure(Failure::InvalidAddress);
        };
        net::tcp_connect(&address).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_unix_connect", |arguments, memory| {
        let [path, length] = arguments else {
            return failure(Failure::InvalidAddress);
        };
        let Some(path) = input(memory, *path, *length) else {
            return failure(Failure::InvalidAddress);
        };
        net::unix_connect(&path).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_tcp_listen", |arguments, memory| {
        let [address, length, backlog] = arguments else {
            return failure(Failure::InvalidAddress);
        };
        let Some(address) = input(memory, *address, *length) else {
            return failure(Failure::InvalidAddress);
        };
        net::tcp_listen(&address, backlog.cast_signed()).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_tcp_accept", |arguments, memory| {
        let [listener, peer, capacity] = arguments else {
            return failure(Failure::Io);
        };
        let Some(mut from) = output(*capacity) else {
            return failure(Failure::Io);
        };
        let answer = net::tcp_accept(listener.cast_signed(), &mut from);
        if answer >= 0 && !copy_ended(memory, *peer, &from) {
            return failure(Failure::Io);
        }
        answer.cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_tcp_send", |arguments, memory| {
        let [stream, data, count] = arguments else {
            return failure(Failure::Io);
        };
        let Some(data) = input(memory, *data, *count) else {
            return failure(Failure::Io);
        };
        net::tcp_send(stream.cast_signed(), &data).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_tcp_receive", |arguments, memory| {
        let [stream, destination, capacity] = arguments else {
            return failure(Failure::Io);
        };
        let Some(mut data) = output(*capacity) else {
            return failure(Failure::Io);
        };
        let answer = net::tcp_receive(stream.cast_signed(), &mut data);
        let Ok(received) = usize::try_from(answer) else {
            return answer.cast_unsigned();
        };
        copy_back(memory, *destination, &data[..received], answer)
    });
    capabilities.register_with_memory("klfsvc_net_tcp_shutdown", |arguments, _| {
        let [stream] = arguments else {
            return failure(Failure::InvalidHandle);
        };
        net::tcp_shutdown(stream.cast_signed()).cast_unsigned()
    });
}

fn register_waiting(capabilities: &mut Capabilities) {
    capabilities.register_with_memory("klfsvc_net_poller_create", |_, _| {
        net::poller_create().cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_poller_watch", |arguments, _| {
        let [poller, handle, interest, token] = arguments else {
            return failure(Failure::InvalidHandle);
        };
        net::poller_watch(
            poller.cast_signed(),
            handle.cast_signed(),
            interest.cast_signed(),
            token.cast_signed(),
        )
        .cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_poller_wait", |arguments, memory| {
        let [poller, timeout, destination, capacity] = arguments else {
            return failure(Failure::Io);
        };
        let Some(words) = usize::try_from(*capacity)
            .ok()
            .and_then(|capacity| capacity.checked_mul(EVENT_WORDS))
            .filter(|words| words * size_of::<i64>() <= klf_rt::MAX_SERVICE_BUFFER_BYTES)
        else {
            return failure(Failure::Io);
        };
        let mut events = vec![0_i64; words];
        let answer = net::poller_wait(poller.cast_signed(), timeout.cast_signed(), &mut events);
        let Ok(count) = usize::try_from(answer) else {
            return answer.cast_unsigned();
        };
        let bytes: Vec<u8> = events[..count * EVENT_WORDS]
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        copy_back(memory, *destination, &bytes, answer)
    });
    capabilities.register_with_memory("klfsvc_net_poller_wake", |arguments, _| {
        let [poller] = arguments else {
            return failure(Failure::InvalidHandle);
        };
        net::poller_wake(poller.cast_signed()).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_resolve", |arguments, memory| {
        let [poller, host, length, token] = arguments else {
            return failure(Failure::InvalidAddress);
        };
        let Some(host) = input(memory, *host, *length) else {
            return failure(Failure::InvalidAddress);
        };
        net::resolve(poller.cast_signed(), &host, token.cast_signed()).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_net_resolved", |arguments, memory| {
        let [lookup, destination, capacity] = arguments else {
            return failure(Failure::Io);
        };
        let Some(mut text) = output(*capacity) else {
            return failure(Failure::Io);
        };
        let answer = net::resolved(lookup.cast_signed(), &mut text);
        // The list was written only when all of it fits.
        match usize::try_from(answer) {
            Ok(length) if length <= text.len() => {
                copy_back(memory, *destination, &text[..length], answer)
            }
            _ => answer.cast_unsigned(),
        }
    });
}
