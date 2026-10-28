//! The TCP connection of section 17.5: a client opens, says hello, pushes
//! modules and reads the device's answers.
//!
//! Framing is `u32 length, u8 type, payload` per frame, so a reader pulls
//! whole frames off the socket without the peer having to agree on message
//! boundaries. The exchange is one request and one response per frame, which
//! keeps a client from pipelining past a nack it has not read yet.

use crate::wire::{Frame, WireError};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// How long a client waits on the device before giving up. A push is answered
/// as soon as the device has installed or refused it, so this bounds a dead
/// peer rather than a slow one.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A client's connection to a device.
pub struct Connection<S: Read + Write> {
    stream: S,
}

impl Connection<TcpStream> {
    /// Connects to a device at `address`, which is `host:port` and may name a
    /// tailnet address as readily as a loopback one.
    ///
    /// # Errors
    /// Returns a message when the address will not parse or nothing answers.
    pub fn dial(address: &str) -> Result<Self, String> {
        let stream = TcpStream::connect(address)
            .map_err(|error| format!("cannot reach {address}: {error}"))?;
        stream.set_read_timeout(Some(TIMEOUT)).ok();
        stream.set_write_timeout(Some(TIMEOUT)).ok();
        Ok(Self { stream })
    }

    /// The address the device is at, which a device behind a tailnet reports
    /// back so a client can name it in a log.
    ///
    /// # Errors
    /// Returns the socket's error when it has no peer address.
    pub fn peer(&self) -> std::io::Result<std::net::SocketAddr> {
        self.stream.peer_addr()
    }
}

impl<S: Read + Write> Connection<S> {
    #[must_use]
    pub fn new(stream: S) -> Self {
        Self { stream }
    }

    /// Sends `frame` and reads the device's answer.
    ///
    /// # Errors
    /// Returns a message when the write or the read fails, or when the bytes
    /// that come back are not a whole frame.
    pub fn exchange(&mut self, frame: &Frame) -> Result<Frame, String> {
        self.write(frame)?;
        self.read()
    }

    /// Sends `frame` without waiting for an answer.
    ///
    /// # Errors
    /// Returns a message when the write fails.
    pub fn write(&mut self, frame: &Frame) -> Result<(), String> {
        let bytes = frame.encode();
        self.stream
            .write_all(&bytes)
            .and_then(|()| self.stream.flush())
            .map_err(|error| format!("cannot send a frame: {error}"))
    }

    /// Reads the device's next frame.
    ///
    /// # Errors
    /// Returns a message when the read fails or the frame is malformed. A peer
    /// that closes cleanly at a frame boundary is an end of exchange, not an
    /// error, so that is reported as such.
    pub fn read(&mut self) -> Result<Frame, String> {
        let mut header = [0u8; 4];
        self.stream
            .read_exact(&mut header)
            .map_err(|error| format!("cannot read a frame header: {error}"))?;
        let length = u32::from_le_bytes(header) as usize;
        if length > crate::wire::MAX_FRAME {
            return Err(WireError(format!("frame of {length} bytes is too large")).to_string());
        }
        let mut body = vec![0u8; length];
        self.stream
            .read_exact(&mut body)
            .map_err(|error| format!("cannot read a frame body: {error}"))?;
        let mut frame = Vec::with_capacity(length + 4);
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&body);
        Frame::decode(&frame).map_err(|error| error.to_string())
    }

    /// Reads until the device's socket closes, handing each frame to `on_frame`.
    ///
    /// This is the device side of a connection that stays open: a live app is
    /// pushed to repeatedly over one connection, and the client is not there to
    /// be told each time.
    ///
    /// # Errors
    /// Returns a message when a frame cannot be read or `on_frame` fails.
    pub fn serve(
        &mut self,
        mut on_frame: impl FnMut(Frame) -> Result<(), String>,
    ) -> Result<(), String> {
        loop {
            match self.read() {
                Ok(frame) => on_frame(frame)?,
                Err(message) if message.contains("cannot read a frame header") => {
                    return Ok(());
                }
                Err(message) => return Err(message),
            }
        }
    }
}
