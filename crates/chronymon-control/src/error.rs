//! Typed errors for the client.

use std::path::PathBuf;
use std::time::Duration;

use crate::client::Server;

/// Neither a path nor an address: not where chronyd could be.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}: neither a path, starting with /, nor an address")]
pub struct ParseServerError(pub String);

/// A reply's status other than success (`STT_*` in `candm.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// `STT_FAILED`.
    Failed,
    /// `STT_UNAUTH`: the command is not allowed from here.
    Unauthorized,
    /// `STT_INVALID`: chronyd does not know the command.
    Invalid,
    /// `STT_NOSUCHSOURCE`: no source at that index or address.
    NoSuchSource,
    /// `STT_BADPKTVERSION`: chronyd speaks another protocol version.
    BadPacketVersion,
    /// `STT_BADPKTLENGTH`: the request was too short.
    BadPacketLength,
    /// Any other code.
    Other(u16),
}

/// What is wrong with a reply's bytes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The reply ends before a field does.
    #[error("reply is {length} bytes, needed {needed}")]
    Truncated {
        /// The reply's length.
        length: usize,
        /// The length the field being read needed.
        needed: usize,
    },

    /// A reply type this client does not decode for the command: chrony
    /// has changed the report's layout.
    #[error("unsupported reply type {reply} to command {command}")]
    UnsupportedReply {
        /// The command code sent.
        command: u16,
        /// The reply type received.
        reply: u16,
    },

    /// An address family code chrony does not define.
    #[error("unknown address family {0}")]
    AddressFamily(u16),

    /// A timestamp out of the representable range.
    #[error("timestamp {0} s out of range")]
    Timestamp(i64),

    /// A field holds a code chrony does not define for it.
    #[error("{field} has unknown value {value}")]
    UnknownValue {
        /// The field.
        field: &'static str,
        /// What it held.
        value: u32,
    },
}

/// Anything that can go wrong asking chronyd for a report.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Setting up the client's own socket failed.
    #[error("client socket {path}: {source}")]
    Socket {
        /// The path involved.
        path: PathBuf,
        /// What failed.
        source: std::io::Error,
    },

    /// chronyd could not be reached: it is not running, or this user
    /// may not use its socket (it needs chronyd's user or root).
    #[error("chronyd {server}: {source}")]
    Connect {
        /// Where chronyd was asked.
        server: Server,
        /// What failed.
        source: std::io::Error,
    },

    /// Sending or receiving failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// No reply after every attempt.
    #[error("no reply after {attempts} attempts of {timeout:?}")]
    Timeout {
        /// Attempts made.
        attempts: u16,
        /// How long each waited.
        timeout: Duration,
    },

    /// chronyd answered with a failure.
    #[error("chronyd refused: {0:?}")]
    Status(Status),

    /// chronyd's reply could not be decoded.
    #[error("bad reply: {0}")]
    Decode(#[from] DecodeError),
}
