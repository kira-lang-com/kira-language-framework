//! UDP sockets and TCP listeners and streams.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr};
use std::sync::Mutex;

use mio::net::{TcpListener, TcpStream, UdpSocket};
use socket2::{Domain, Socket, Type};

use crate::table::{self, Connection, Resource};
use crate::{Failure, address, ended_text, length, word};

/// Binds a UDP socket at `address`; port 0 picks a free one.
#[must_use]
pub fn udp_bind(address_text: &[u8]) -> i64 {
    word((|| {
        let socket = UdpSocket::bind(address(address_text)?)?;
        Ok(table::insert(Resource::Udp(Mutex::new(socket))))
    })())
}

/// Sends one datagram to `peer`, answering the bytes sent.
#[must_use]
pub fn udp_send(handle: i64, peer: &[u8], data: &[u8]) -> i64 {
    word((|| {
        let peer = address(peer)?;
        let resource = table::get(handle)?;
        let Resource::Udp(socket) = &*resource else {
            return Err(Failure::InvalidHandle);
        };
        Ok(length(table::lock(socket).send_to(data, peer)?))
    })())
}

/// Receives one datagram into `destination`, writing its sender into `peer`
/// as NUL-ended text, and answers the bytes received. A datagram longer than
/// `destination` is cut to it, as the platform does.
#[must_use]
pub fn udp_receive(handle: i64, destination: &mut [u8], peer: &mut [u8]) -> i64 {
    word((|| {
        let resource = table::get(handle)?;
        let Resource::Udp(socket) = &*resource else {
            return Err(Failure::InvalidHandle);
        };
        let (count, from) = table::lock(socket).recv_from(destination)?;
        ended_text(&from.to_string(), peer);
        Ok(length(count))
    })())
}

/// Starts connecting a stream to `address`. The handle comes back at once;
/// the poller reports the stream writable once it is connected, or an error
/// when the connection failed.
#[must_use]
pub fn tcp_connect(address_text: &[u8]) -> i64 {
    word((|| {
        let stream = TcpStream::connect(address(address_text)?)?;
        // Requests and frames are written whole by the protocol above, so
        // holding them back to coalesce only adds latency. Only a hint: a
        // socket still connecting may refuse it, as Windows does, and that
        // is no reason to fail the connection.
        let _ = stream.set_nodelay(true);
        Ok(table::insert(Resource::Stream(Mutex::new(Connection::Tcp(stream)))))
    })())
}

/// Connects a stream to the Unix domain socket at `path`. It sends,
/// receives, shuts down and waits in a poller as a TCP stream does. A host
/// without Unix sockets answers [`Failure::InvalidAddress`].
#[must_use]
pub fn unix_connect(path: &[u8]) -> i64 {
    word((|| {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let path = std::path::Path::new(std::ffi::OsStr::from_bytes(path));
            let stream = mio::net::UnixStream::connect(path)?;
            Ok(table::insert(Resource::Stream(Mutex::new(Connection::Local(stream)))))
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(Failure::InvalidAddress)
        }
    })())
}

/// Listens at `address` with room for `backlog` connections not yet accepted.
#[must_use]
pub fn tcp_listen(address_text: &[u8], backlog: i64) -> i64 {
    word((|| {
        let at: SocketAddr = address(address_text)?;
        let backlog = i32::try_from(backlog.max(1)).unwrap_or(i32::MAX);
        let socket = Socket::new(Domain::for_address(at), Type::STREAM, None)?;
        // A restarted server binds its port again while the old connections
        // are still in TIME_WAIT.
        socket.set_reuse_address(true)?;
        socket.bind(&at.into())?;
        socket.listen(backlog)?;
        socket.set_nonblocking(true)?;
        let listener = TcpListener::from_std(socket.into());
        Ok(table::insert(Resource::Listener(Mutex::new(listener))))
    })())
}

/// Accepts one waiting connection, writing its peer into `peer` as NUL-ended
/// text, and answers the stream's handle.
#[must_use]
pub fn tcp_accept(handle: i64, peer: &mut [u8]) -> i64 {
    word((|| {
        let resource = table::get(handle)?;
        let Resource::Listener(listener) = &*resource else {
            return Err(Failure::InvalidHandle);
        };
        let (stream, from) = table::lock(listener).accept()?;
        stream.set_nodelay(true)?;
        ended_text(&from.to_string(), peer);
        Ok(table::insert(Resource::Stream(Mutex::new(Connection::Tcp(stream)))))
    })())
}

/// Writes what the stream accepts of `data` and answers how much that was.
#[must_use]
pub fn tcp_send(handle: i64, data: &[u8]) -> i64 {
    word((|| {
        let resource = table::get(handle)?;
        let Resource::Stream(stream) = &*resource else {
            return Err(Failure::InvalidHandle);
        };
        Ok(length(table::lock(stream).write(data)?))
    })())
}

/// Reads what has arrived into `destination`; 0 is the peer's end.
#[must_use]
pub fn tcp_receive(handle: i64, destination: &mut [u8]) -> i64 {
    word((|| {
        let resource = table::get(handle)?;
        let Resource::Stream(stream) = &*resource else {
            return Err(Failure::InvalidHandle);
        };
        Ok(length(table::lock(stream).read(destination)?))
    })())
}

/// Ends the write side; the peer reads its end once what was sent arrives.
#[must_use]
pub fn tcp_shutdown(handle: i64) -> i64 {
    word((|| {
        let resource = table::get(handle)?;
        let Resource::Stream(stream) = &*resource else {
            return Err(Failure::InvalidHandle);
        };
        table::lock(stream).shutdown(Shutdown::Write)?;
        Ok(1)
    })())
}

/// The address a socket is bound at, as text copied with a NUL when it fits;
/// answers the text's length.
#[must_use]
pub fn local_address(handle: i64, destination: &mut [u8]) -> i64 {
    word((|| {
        let resource = table::get(handle)?;
        let bound = match &*resource {
            Resource::Udp(socket) => table::lock(socket).local_addr()?.to_string(),
            Resource::Listener(listener) => table::lock(listener).local_addr()?.to_string(),
            Resource::Stream(stream) => table::lock(stream).local_text()?,
            Resource::Poller(_) | Resource::Lookup(_) => return Err(Failure::InvalidHandle),
        };
        Ok(length(ended_text(&bound, destination)))
    })())
}
