//! Private, inherited Unix channels for RIDL section 24.7.
//!
//! Each request carries a new reply socket, so a service never waits on the
//! channel whose request it is handling. Transferred resources travel as OS
//! descriptors, not as the sender's numeric handle values.

#![allow(unsafe_code)]

use crate::{Call, MAX_DEPTH, MAX_MESSAGE, Message, Reply, Value};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// A connection is bound to exactly one peer, unlike a published socket name.
pub struct Connection {
    identity: u64,
    reader: Mutex<UnixStream>,
    writer: Mutex<UnixStream>,
    next_request: AtomicU64,
}

/// An incoming RIDL message, including a separately transferred OS resource.
pub struct Incoming {
    pub message: Message,
    pub resource: Option<OwnedFd>,
    reply_channel: Option<UnixStream>,
}

/// A handle to an object hosted by this connection's peer. Cloning keeps the
/// same proxy alive; dropping the last clone releases the remote reference.
#[derive(Clone)]
pub struct RemoteObject {
    inner: Arc<RemoteObjectInner>,
}

struct RemoteObjectInner {
    connection: Arc<Connection>,
    id: u32,
}

impl Drop for RemoteObjectInner {
    fn drop(&mut self) {
        let _ = self.connection.release(self.id);
    }
}

impl RemoteObject {
    #[must_use]
    pub fn id(&self) -> u32 {
        self.inner.id
    }
}

impl Connection {
    /// Makes a private connection, with one endpoint for each runtime.
    ///
    /// # Errors
    /// Returns the OS error when a private socket cannot be created.
    pub fn pair(identity: u64) -> io::Result<(Self, Self)> {
        let (host, plugin) = UnixStream::pair()?;
        Ok((
            Self::from_stream(host, identity)?,
            Self::from_stream(plugin, identity)?,
        ))
    }

    /// Adopts a channel the launcher handed to this process.
    ///
    /// # Errors
    /// Returns the OS error when the channel cannot be duplicated.
    pub fn from_stream(stream: UnixStream, identity: u64) -> io::Result<Self> {
        let writer = stream.try_clone()?;
        Ok(Self {
            identity,
            reader: Mutex::new(stream),
            writer: Mutex::new(writer),
            next_request: AtomicU64::new(1),
        })
    }

    #[must_use]
    pub fn identity(&self) -> u64 {
        self.identity
    }

    /// Calls a RIDL export on the peer and waits on this request's own socket.
    /// A different thread may continue serving the main connection meanwhile.
    ///
    /// # Errors
    /// Returns a transport, protocol, or size-limit error.
    pub fn call(&self, function: &str, arguments: Vec<Value>) -> io::Result<Reply> {
        let request = self.next_request.fetch_add(1, Ordering::Relaxed);
        if request == u64::MAX {
            return Err(invalid("RIDL request identifiers exhausted"));
        }
        let (mut answer, recipient) = UnixStream::pair()?;
        let sent = self.send(
            &Message::Request(Call::new(self.identity, request, function, arguments)),
            Some(recipient.as_raw_fd()),
        );
        // The receiver now owns the transferred endpoint. Keeping our copy
        // open would prevent EOF if the peer died before answering.
        drop(recipient);
        sent?;
        let (message, transferred) = read_frame(&mut answer)?;
        if transferred.is_some() {
            return Err(invalid("a RIDL reply carried an unexpected descriptor"));
        }
        match message {
            Message::Reply { request: id, reply } if id == request => Ok(reply),
            _ => Err(invalid("a RIDL reply did not name its request")),
        }
    }

    /// Transfers a resource to this peer under an exported object id.
    /// The peer owns the new descriptor; the sender retains its original.
    ///
    /// # Errors
    /// Returns a transport error when the descriptor cannot be transferred.
    pub fn export_resource(&self, object: u32, resource: &OwnedFd) -> io::Result<()> {
        self.send(
            &Message::Exported {
                connection: self.identity,
                object,
            },
            Some(resource.as_raw_fd()),
        )
    }

    /// Exports a RIDL object that remains in this process. Only the object
    /// identity crosses; the peer creates a proxy to call it over this channel.
    ///
    /// # Errors
    /// Returns a transport error if the object announcement cannot be sent.
    pub fn export_object(&self, object: u32) -> io::Result<()> {
        self.send(
            &Message::Exported {
                connection: self.identity,
                object,
            },
            None,
        )
    }

    /// Creates a proxy after receiving an object export from this connection.
    #[must_use]
    pub fn proxy(self: &Arc<Self>, object: u32) -> RemoteObject {
        RemoteObject {
            inner: Arc::new(RemoteObjectInner {
                connection: Arc::clone(self),
                id: object,
            }),
        }
    }

    /// Announces that an exported object's last remote reference is gone.
    ///
    /// # Errors
    /// Returns a transport error if the peer cannot receive the notification.
    pub fn release(&self, object: u32) -> io::Result<()> {
        self.send(
            &Message::Released {
                connection: self.identity,
                object,
            },
            None,
        )
    }

    /// Receives exactly one call, resource, release or close from the peer.
    ///
    /// # Errors
    /// Returns transport or protocol errors for truncated or invalid messages.
    pub fn receive(&self) -> io::Result<Incoming> {
        let mut stream = self.reader.lock().unwrap_or_else(PoisonError::into_inner);
        let (message, descriptor) = read_frame(&mut stream)?;
        let mut resource = None;
        let mut reply_channel = None;
        let valid = match &message {
            Message::Request(call) => {
                if call.connection == self.identity {
                    reply_channel = descriptor.map(UnixStream::from);
                    reply_channel.is_some()
                } else {
                    false
                }
            }
            Message::Exported { connection, .. } if *connection == self.identity => {
                resource = descriptor;
                true
            }
            Message::Released { connection, .. } | Message::Closed { connection } => {
                *connection == self.identity && descriptor.is_none()
            }
            _ => false,
        };
        if !valid {
            return Err(invalid("invalid RIDL message or connection identity"));
        }
        Ok(Incoming {
            message,
            resource,
            reply_channel,
        })
    }

    /// Handles a single request; a host can also receive resources and releases
    /// with `receive` to manage their lifetimes in its own event loop.
    ///
    /// # Errors
    /// Returns a transport error if the call or its reply is malformed.
    pub fn serve_once(&self, handler: impl FnOnce(Call) -> Reply) -> io::Result<()> {
        let incoming = self.receive()?;
        let Message::Request(call) = incoming.message else {
            return Err(invalid("expected a RIDL request"));
        };
        let response = Message::Reply {
            request: call.request,
            reply: handler(call),
        };
        let mut reply = incoming
            .reply_channel
            .ok_or_else(|| invalid("request has no reply channel"))?;
        write_frame(&mut reply, &response, None)
    }

    /// Dispatches a received request. The caller can process exported resources
    /// and releases in the same loop without losing an already-received request.
    ///
    /// # Errors
    /// Returns a transport error if the response cannot be written.
    pub fn respond(
        &self,
        incoming: Incoming,
        handler: impl FnOnce(Call) -> Reply,
    ) -> io::Result<()> {
        let Message::Request(call) = incoming.message else {
            return Err(invalid("expected a RIDL request"));
        };
        let response = Message::Reply {
            request: call.request,
            reply: handler(call),
        };
        let mut reply = incoming
            .reply_channel
            .ok_or_else(|| invalid("request has no reply channel"))?;
        write_frame(&mut reply, &response, None)
    }

    /// Notifies the peer that every object on this connection is invalid.
    ///
    /// # Errors
    /// Returns a transport error if the close frame cannot be written.
    pub fn close(&self) -> io::Result<()> {
        self.send(
            &Message::Closed {
                connection: self.identity,
            },
            None,
        )
    }

    fn send(&self, message: &Message, descriptor: Option<RawFd>) -> io::Result<()> {
        let mut stream = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        write_frame(&mut stream, message, descriptor)
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn write_frame(stream: &mut UnixStream, message: &Message, fd: Option<RawFd>) -> io::Result<()> {
    validate_values(message)?;
    let bytes = message.encode().map_err(|error| invalid(&error))?;
    let count = u32::try_from(bytes.len()).map_err(|_| invalid("RIDL message too long"))?;
    let length = count.to_le_bytes();
    send_header(stream, &length, fd)?;
    stream.write_all(&bytes)
}

fn send_header(stream: &mut UnixStream, header: &[u8; 4], fd: Option<RawFd>) -> io::Result<()> {
    let mut iov = libc::iovec {
        iov_base: header.as_ptr().cast_mut().cast(),
        iov_len: header.len(),
    };
    // The ancillary buffer has cmsghdr alignment, not just byte alignment.
    let mut ancillary = [0usize; 8];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    if let Some(descriptor) = fd {
        message.msg_control = ancillary.as_mut_ptr().cast();
        message.msg_controllen = unsafe { libc::CMSG_SPACE(4) as _ };
        let control = unsafe { libc::CMSG_FIRSTHDR(&raw const message) };
        unsafe {
            (*control).cmsg_level = libc::SOL_SOCKET;
            (*control).cmsg_type = libc::SCM_RIGHTS;
            (*control).cmsg_len = libc::CMSG_LEN(4) as _;
            std::ptr::copy_nonoverlapping(
                (&raw const descriptor).cast::<u8>(),
                libc::CMSG_DATA(control),
                size_of::<RawFd>(),
            );
        }
    }
    let count = loop {
        let result = unsafe { libc::sendmsg(stream.as_raw_fd(), &raw const message, 0) };
        if result >= 0 {
            break result.cast_unsigned();
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    if count == 0 {
        return Err(io::ErrorKind::WriteZero.into());
    }
    if count < header.len() {
        stream.write_all(&header[count..])?;
    }
    Ok(())
}

fn read_frame(stream: &mut UnixStream) -> io::Result<(Message, Option<OwnedFd>)> {
    let mut header = [0u8; 4];
    let mut iov = libc::iovec {
        iov_base: header.as_mut_ptr().cast(),
        iov_len: header.len(),
    };
    let mut ancillary = [0usize; 8];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    message.msg_control = ancillary.as_mut_ptr().cast();
    message.msg_controllen =
        u32::try_from(size_of_val(&ancillary)).expect("small ancillary buffer") as _;
    let count = loop {
        let result = unsafe { libc::recvmsg(stream.as_raw_fd(), &raw mut message, 0) };
        if result >= 0 {
            break result.cast_unsigned();
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    if count == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    let mut received = Vec::new();
    let mut control = unsafe { libc::CMSG_FIRSTHDR(&raw const message) };
    while !control.is_null() {
        if unsafe {
            (*control).cmsg_level == libc::SOL_SOCKET && (*control).cmsg_type == libc::SCM_RIGHTS
        } {
            let bytes = (unsafe { (*control).cmsg_len } as usize)
                .saturating_sub(unsafe { libc::CMSG_LEN(0) } as usize);
            if !bytes.is_multiple_of(size_of::<RawFd>()) {
                return Err(invalid("a RIDL message carried malformed descriptors"));
            }
            for index in 0..bytes / size_of::<RawFd>() {
                let descriptor = unsafe {
                    std::ptr::read_unaligned(
                        libc::CMSG_DATA(control)
                            .add(index * size_of::<RawFd>())
                            .cast::<RawFd>(),
                    )
                };
                // SAFETY: SCM_RIGHTS installed a new descriptor owned by the receiver.
                let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
                let flags = unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_GETFD) };
                if flags < 0
                    || unsafe {
                        libc::fcntl(owned.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC)
                    } < 0
                {
                    return Err(io::Error::last_os_error());
                }
                received.push(owned);
            }
        }
        control = unsafe { libc::CMSG_NXTHDR(&raw const message, control) };
    }
    if message.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(invalid("RIDL ancillary data was truncated"));
    }
    if received.len() > 1 {
        return Err(invalid("a RIDL message carried multiple descriptors"));
    }
    stream.read_exact(&mut header[count..])?;
    let count = u32::from_le_bytes(header) as usize;
    if count > MAX_MESSAGE {
        return Err(invalid("RIDL message exceeds 64 KiB"));
    }
    let mut body = vec![0; count];
    stream.read_exact(&mut body)?;
    let mut decoded = Message::decode(&body).map_err(|error| invalid(&error))?;
    validate_values(&decoded)?;
    orient_incoming(&mut decoded);
    Ok((decoded, received.pop()))
}

fn orient_incoming(message: &mut Message) {
    match message {
        Message::Request(call) => {
            for argument in &mut call.arguments {
                flip_objects(argument);
            }
        }
        Message::Reply {
            reply: Reply::Returned(value) | Reply::Failed(value),
            ..
        } => flip_objects(value),
        _ => {}
    }
}

fn flip_objects(value: &mut Value) {
    match value {
        Value::Object(reference) => reference.side = reference.side.flipped(),
        Value::Seq(values) | Value::Struct(_, values) | Value::Enum(_, _, values) => {
            for value in values {
                flip_objects(value);
            }
        }
        Value::Optional(Some(value)) | Value::Ok(value) | Value::Err(value) => {
            flip_objects(value);
        }
        _ => {}
    }
}

fn validate_values(message: &Message) -> io::Result<()> {
    let mut todo = Vec::new();
    match message {
        Message::Request(call) => todo.extend(call.arguments.iter().map(|value| (value, 0))),
        Message::Reply {
            reply: Reply::Returned(value) | Reply::Failed(value),
            ..
        } => {
            todo.push((value, 0));
        }
        _ => {}
    }
    while let Some((value, depth)) = todo.pop() {
        if depth >= MAX_DEPTH {
            return Err(invalid("KLF-RT-0204: RIDL value exceeds nesting limit"));
        }
        let next = depth + 1;
        match value {
            Value::RawPtr(_) => return Err(invalid("raw pointers cannot cross RIDL processes")),
            Value::Seq(children) | Value::Struct(_, children) | Value::Enum(_, _, children) => {
                todo.extend(children.iter().map(|value| (value, next)));
            }
            Value::Optional(Some(value)) | Value::Ok(value) | Value::Err(value) => {
                todo.push((value, next));
            }
            _ => {}
        }
    }
    Ok(())
}
