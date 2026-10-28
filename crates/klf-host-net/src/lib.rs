//! The `klf.net` kernel services: sockets, readiness and name resolution.
//!
//! One handle table per process holds every socket, poller and lookup, so the
//! VM and the native runtime answer the same words. A handle is a positive
//! word; a failure is a negative word from [`Failure`], so one result word
//! carries either. Every socket is non-blocking: an operation that cannot
//! proceed answers [`Failure::WouldBlock`], and the poller says when to try
//! again. Readiness is reported when it changes, so a caller reads or writes
//! until it is told would-block before it waits.
//!
//! Addresses cross as text (`192.0.2.1:443`, `[2001:db8::1]:443`), because
//! parsing them is the hosted language's concern.

mod poller;
mod resolver;
mod sockets;
mod table;

use std::io;

pub use poller::{
    create as poller_create, wait as poller_wait, wake as poller_wake, watch as poller_watch,
};
pub use resolver::{resolve, resolved};
pub use sockets::{
    local_address, tcp_accept, tcp_connect, tcp_listen, tcp_receive, tcp_send, tcp_shutdown,
    udp_bind, udp_receive, udp_send, unix_connect,
};
pub use table::close;

/// Why a network operation failed, as the negative word it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum Failure {
    WouldBlock = -1,
    Refused = -2,
    Unreachable = -3,
    AddressInUse = -4,
    InvalidHandle = -5,
    InvalidAddress = -6,
    TimedOut = -7,
    Reset = -8,
    NotFound = -9,
    Io = -10,
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            // A stream still connecting cannot send yet; that is a wait, not
            // a failure.
            io::ErrorKind::WouldBlock | io::ErrorKind::NotConnected => Self::WouldBlock,
            io::ErrorKind::ConnectionRefused => Self::Refused,
            io::ErrorKind::HostUnreachable | io::ErrorKind::NetworkUnreachable => Self::Unreachable,
            io::ErrorKind::AddrInUse => Self::AddressInUse,
            io::ErrorKind::AddrNotAvailable | io::ErrorKind::InvalidInput => Self::InvalidAddress,
            io::ErrorKind::TimedOut => Self::TimedOut,
            io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe => Self::Reset,
            _ => Self::Io,
        }
    }
}

/// The bits of an interest or event word.
pub mod bits {
    pub const READABLE: i64 = 1;
    pub const WRITABLE: i64 = 2;
    pub const PEER_CLOSED: i64 = 4;
    pub const ERROR: i64 = 8;
    pub const RESOLVED: i64 = 16;
    pub const WOKEN: i64 = 32;
}

/// The words of one event: its token, then its bits.
pub const EVENT_WORDS: usize = 2;

fn word(result: Result<i64, Failure>) -> i64 {
    result.unwrap_or_else(|failure| failure as i64)
}

fn length(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

fn address(text: &[u8]) -> Result<std::net::SocketAddr, Failure> {
    std::str::from_utf8(text)
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or(Failure::InvalidAddress)
}

/// Writes `text` and a NUL into `destination` when both fit, as `klf.fs.list`
/// does, and answers the text's length either way.
fn ended_text(text: &str, destination: &mut [u8]) -> usize {
    let bytes = text.as_bytes();
    if bytes.len() < destination.len() {
        destination[..bytes.len()].copy_from_slice(bytes);
        destination[bytes.len()] = 0;
    }
    bytes.len()
}
