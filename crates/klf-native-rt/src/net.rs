//! Native ABI shims for the klf.net kernel services.

use klf_host_net::{self as net, EVENT_WORDS, Failure};

const INVALID: i64 = Failure::InvalidAddress as i64;
const IO: i64 = Failure::Io as i64;

pub(crate) fn bytes<'a>(pointer: *const u8, length: i64) -> Option<&'a [u8]> {
    let length = usize::try_from(length).ok()?;
    if length == 0 {
        return Some(&[]);
    }
    if pointer.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(pointer, length) })
}

pub(crate) fn bytes_mut<'a>(pointer: *mut u8, length: i64) -> Option<&'a mut [u8]> {
    let length = usize::try_from(length).ok()?;
    if length == 0 {
        return Some(&mut []);
    }
    if pointer.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts_mut(pointer, length) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_udp_bind(address: *const u8, length: i64) -> i64 {
    bytes(address, length).map_or(INVALID, net::udp_bind)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_udp_send(
    socket: i64,
    peer: *const u8,
    peer_length: i64,
    data: *const u8,
    count: i64,
) -> i64 {
    let Some(peer) = bytes(peer, peer_length) else {
        return INVALID;
    };
    let Some(data) = bytes(data, count) else {
        return IO;
    };
    net::udp_send(socket, peer, data)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_udp_receive(
    socket: i64,
    destination: *mut u8,
    capacity: i64,
    peer: *mut u8,
    peer_capacity: i64,
) -> i64 {
    let Some(destination) = bytes_mut(destination, capacity) else {
        return IO;
    };
    let Some(peer) = bytes_mut(peer, peer_capacity) else {
        return IO;
    };
    net::udp_receive(socket, destination, peer)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_tcp_connect(address: *const u8, length: i64) -> i64 {
    bytes(address, length).map_or(INVALID, net::tcp_connect)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_unix_connect(path: *const u8, length: i64) -> i64 {
    bytes(path, length).map_or(INVALID, net::unix_connect)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_tcp_listen(
    address: *const u8,
    length: i64,
    backlog: i64,
) -> i64 {
    bytes(address, length).map_or(INVALID, |address| net::tcp_listen(address, backlog))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_tcp_accept(listener: i64, peer: *mut u8, capacity: i64) -> i64 {
    let Some(peer) = bytes_mut(peer, capacity) else {
        return IO;
    };
    net::tcp_accept(listener, peer)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_tcp_send(stream: i64, data: *const u8, count: i64) -> i64 {
    bytes(data, count).map_or(IO, |data| net::tcp_send(stream, data))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_tcp_receive(
    stream: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    bytes_mut(destination, capacity).map_or(IO, |destination| net::tcp_receive(stream, destination))
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_net_tcp_shutdown(stream: i64) -> i64 {
    net::tcp_shutdown(stream)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_local_address(
    handle: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    bytes_mut(destination, capacity)
        .map_or(IO, |destination| net::local_address(handle, destination))
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_net_close(handle: i64) -> i64 {
    net::close(handle)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_net_poller_create() -> i64 {
    net::poller_create()
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_net_poller_watch(
    poller: i64,
    handle: i64,
    interest: i64,
    token: i64,
) -> i64 {
    net::poller_watch(poller, handle, interest, token)
}

/// The events land in a word buffer the program owns, which need not be
/// aligned for `i64`, so they are gathered here and copied out as bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_poller_wait(
    poller: i64,
    timeout: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    let Ok(capacity) = usize::try_from(capacity) else {
        return IO;
    };
    if capacity != 0 && destination.is_null() {
        return IO;
    }
    let mut events = vec![0_i64; capacity.saturating_mul(EVENT_WORDS)];
    let count = net::poller_wait(poller, timeout, &mut events);
    if let Ok(written) = usize::try_from(count)
        && written != 0
    {
        let length = written * EVENT_WORDS * size_of::<i64>();
        unsafe { std::ptr::copy_nonoverlapping(events.as_ptr().cast::<u8>(), destination, length) };
    }
    count
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_net_poller_wake(poller: i64) -> i64 {
    net::poller_wake(poller)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_resolve(
    poller: i64,
    host: *const u8,
    length: i64,
    token: i64,
) -> i64 {
    bytes(host, length).map_or(INVALID, |host| net::resolve(poller, host, token))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_net_resolved(
    lookup: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    bytes_mut(destination, capacity).map_or(IO, |destination| net::resolved(lookup, destination))
}
