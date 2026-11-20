//! Rustic's inter-process channel (ISO-02, ADR 0056).
//!
//! A [`Channel`] carries typed [`protocol`] messages between the broker
//! (browser) process and a renderer over any byte stream: the child's
//! standard input and output pipes in practice, which every supported
//! platform has without unsafe code or descriptor passing.
//!
//! Each side treats the other as hostile. A connection starts with a
//! handshake that names the protocol version and the sender's role; a
//! mismatch ends it. Every frame is length-prefixed and bounded
//! ([`Limits`]) before anything is allocated, decoding consumes exactly the
//! frame, unknown message kinds and trailing bytes are errors, strings are
//! validated UTF-8, and nothing panics on input. End of stream (the peer
//! exited or crashed) is [`IpcError::Closed`], so waiting work is
//! cancelled instead of hanging.

#![warn(missing_docs)]

pub mod protocol;
mod wire;

use std::fmt;
use std::io::{self, Read, Write};

pub use wire::{DecodeError, Reader, Writer};

/// Bytes every handshake starts with.
pub const MAGIC: &[u8; 8] = b"RUSTICIP";
/// The protocol version this build speaks. Peers must match exactly: the
/// broker and renderer are always the same executable.
pub const PROTOCOL_VERSION: u16 = 1;

/// Which end of the channel a process is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// The browser process: owns authority (files, network, storage).
    Broker,
    /// A renderer: parses and runs content, asks the broker for everything.
    Renderer,
}

impl Role {
    fn byte(self) -> u8 {
        match self {
            Self::Broker => 1,
            Self::Renderer => 2,
        }
    }

    fn peer(self) -> Self {
        match self {
            Self::Broker => Self::Renderer,
            Self::Renderer => Self::Broker,
        }
    }
}

/// Channel bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Largest frame payload accepted or sent.
    pub max_frame_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_frame_bytes: 128 * 1024 * 1024,
        }
    }
}

/// Why a channel operation failed. Any error ends the channel.
#[derive(Debug)]
pub enum IpcError {
    /// The peer closed the stream (it exited or crashed).
    Closed,
    /// Reading or writing failed.
    Io(io::Error),
    /// The handshake named another protocol, version or role.
    Handshake,
    /// A frame is larger than [`Limits::max_frame_bytes`].
    FrameTooLarge(u64),
    /// A frame did not decode as a message of this protocol.
    Malformed(DecodeError),
}

impl fmt::Display for IpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("the other process closed the channel"),
            Self::Io(error) => write!(formatter, "IPC I/O failed: {error}"),
            Self::Handshake => formatter.write_str("IPC handshake mismatch"),
            Self::FrameTooLarge(length) => write!(formatter, "IPC frame of {length} bytes refused"),
            Self::Malformed(error) => write!(formatter, "malformed IPC message: {error}"),
        }
    }
}

impl std::error::Error for IpcError {}

impl From<DecodeError> for IpcError {
    fn from(error: DecodeError) -> Self {
        Self::Malformed(error)
    }
}

fn io_error(error: io::Error) -> IpcError {
    if error.kind() == io::ErrorKind::UnexpectedEof || error.kind() == io::ErrorKind::BrokenPipe {
        IpcError::Closed
    } else {
        IpcError::Io(error)
    }
}

/// A message that can cross the channel.
pub trait Message: Sized {
    /// Encode the message (its kind tag first).
    fn encode(&self, writer: &mut Writer);
    /// Decode one message; the channel checks that nothing is left over.
    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError>;
}

/// One end of a connection: sends `Out` messages and receives `In`.
pub struct Channel<R, W> {
    reader: R,
    writer: W,
    limits: Limits,
}

impl<R, W> fmt::Debug for Channel<R, W> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Channel")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl<R: Read, W: Write> Channel<R, W> {
    /// Connect as `role`: send our handshake, then check the peer's.
    pub fn connect(reader: R, writer: W, role: Role, limits: Limits) -> Result<Self, IpcError> {
        let mut channel = Self {
            reader,
            writer,
            limits,
        };
        let mut hello = MAGIC.to_vec();
        hello.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        hello.push(role.byte());
        channel.writer.write_all(&hello).map_err(io_error)?;
        channel.writer.flush().map_err(io_error)?;
        let mut peer = [0u8; 11];
        channel.reader.read_exact(&mut peer).map_err(io_error)?;
        if peer[..8] != MAGIC[..]
            || peer[8..10] != PROTOCOL_VERSION.to_le_bytes()
            || peer[10] != role.peer().byte()
        {
            return Err(IpcError::Handshake);
        }
        Ok(channel)
    }

    /// Send one message.
    pub fn send<M: Message>(&mut self, message: &M) -> Result<(), IpcError> {
        let mut writer = Writer::new();
        message.encode(&mut writer);
        let payload = writer.into_bytes();
        if payload.len() > self.limits.max_frame_bytes {
            return Err(IpcError::FrameTooLarge(payload.len() as u64));
        }
        let length = u32::try_from(payload.len())
            .map_err(|_| IpcError::FrameTooLarge(payload.len() as u64))?;
        self.writer
            .write_all(&length.to_le_bytes())
            .map_err(io_error)?;
        self.writer.write_all(&payload).map_err(io_error)?;
        self.writer.flush().map_err(io_error)
    }

    /// Receive one message, refusing oversized or malformed frames.
    pub fn recv<M: Message>(&mut self) -> Result<M, IpcError> {
        let mut length = [0u8; 4];
        self.reader.read_exact(&mut length).map_err(io_error)?;
        let length = u32::from_le_bytes(length) as u64;
        if length > self.limits.max_frame_bytes as u64 {
            return Err(IpcError::FrameTooLarge(length));
        }
        // Read in bounded steps: a peer that announces a large frame and
        // then stops costs at most what it actually sent.
        let mut payload = Vec::new();
        let mut remaining = length as usize;
        let mut chunk = [0u8; 64 * 1024];
        while remaining > 0 {
            let want = remaining.min(chunk.len());
            self.reader
                .read_exact(&mut chunk[..want])
                .map_err(io_error)?;
            payload.extend_from_slice(&chunk[..want]);
            remaining -= want;
        }
        decode_frame(&payload)
    }
}

/// Decode one complete frame payload (also used by the fuzz target).
pub fn decode_frame<M: Message>(payload: &[u8]) -> Result<M, IpcError> {
    let mut reader = Reader::new(payload);
    let message = M::decode(&mut reader)?;
    reader.finish()?;
    Ok(message)
}

/// Encode a message as a frame payload (without the length prefix).
pub fn encode_frame<M: Message>(message: &M) -> Vec<u8> {
    let mut writer = Writer::new();
    message.encode(&mut writer);
    writer.into_bytes()
}
