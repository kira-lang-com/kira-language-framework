//! The handle table every `klf.net` service names its resources through.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use mio::event::Source;
use mio::net::{TcpListener, TcpStream, UdpSocket};
use mio::{Interest, Registry, Token};

use crate::poller::Poller;
use crate::resolver::Lookup;
use crate::{Failure, length, word};

// A socket sits behind a lock because registering it with a poller needs it
// mutably; every operation on it is non-blocking, so the lock is held briefly.
pub(crate) enum Resource {
    Udp(Mutex<UdpSocket>),
    Listener(Mutex<TcpListener>),
    Stream(Mutex<Connection>),
    Poller(Poller),
    Lookup(Lookup),
}

/// A connected stream: TCP, or a Unix domain socket where the host has them.
/// Both read, write, end their write side and wait in a poller alike.
pub(crate) enum Connection {
    Tcp(TcpStream),
    #[cfg(unix)]
    Local(mio::net::UnixStream),
}

impl Connection {
    pub(crate) fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.shutdown(how),
            #[cfg(unix)]
            Self::Local(stream) => stream.shutdown(how),
        }
    }

    /// The address the stream is bound at, as text; a Unix socket answers
    /// its path, empty when it has none.
    pub(crate) fn local_text(&self) -> io::Result<String> {
        match self {
            Self::Tcp(stream) => Ok(stream.local_addr()?.to_string()),
            #[cfg(unix)]
            Self::Local(stream) => Ok(stream
                .local_addr()?
                .as_pathname()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default()),
        }
    }
}

impl Read for Connection {
    fn read(&mut self, destination: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.read(destination),
            #[cfg(unix)]
            Self::Local(stream) => stream.read(destination),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.write(data),
            #[cfg(unix)]
            Self::Local(stream) => stream.write(data),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.flush(),
            #[cfg(unix)]
            Self::Local(stream) => stream.flush(),
        }
    }
}

impl Source for Connection {
    fn register(&mut self, registry: &Registry, token: Token, interests: Interest) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.register(registry, token, interests),
            #[cfg(unix)]
            Self::Local(stream) => stream.register(registry, token, interests),
        }
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.reregister(registry, token, interests),
            #[cfg(unix)]
            Self::Local(stream) => stream.reregister(registry, token, interests),
        }
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.deregister(registry),
            #[cfg(unix)]
            Self::Local(stream) => stream.deregister(registry),
        }
    }
}

// An entry is shared so a call in flight keeps its socket alive while another
// thread closes the handle; the socket closes when the last call ends.
static TABLE: RwLock<Vec<Option<Arc<Resource>>>> = RwLock::new(Vec::new());

pub(crate) fn insert(resource: Resource) -> i64 {
    let mut table = TABLE.write().unwrap_or_else(PoisonError::into_inner);
    let entry = Some(Arc::new(resource));
    let index = if let Some(free) = table.iter().position(Option::is_none) {
        table[free] = entry;
        free
    } else {
        table.push(entry);
        table.len() - 1
    };
    length(index + 1)
}

pub(crate) fn lock<Value>(held: &Mutex<Value>) -> MutexGuard<'_, Value> {
    held.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn get(handle: i64) -> Result<Arc<Resource>, Failure> {
    let index = usize::try_from(handle - 1).map_err(|_| Failure::InvalidHandle)?;
    let table = TABLE.read().unwrap_or_else(PoisonError::into_inner);
    table
        .get(index)
        .and_then(Option::clone)
        .ok_or(Failure::InvalidHandle)
}

/// Releases a socket, poller or lookup.
#[must_use]
pub fn close(handle: i64) -> i64 {
    word((|| {
        let index = usize::try_from(handle - 1).map_err(|_| Failure::InvalidHandle)?;
        let mut table = TABLE.write().unwrap_or_else(PoisonError::into_inner);
        let slot = table.get_mut(index).ok_or(Failure::InvalidHandle)?;
        slot.take().ok_or(Failure::InvalidHandle)?;
        Ok(1)
    })())
}
