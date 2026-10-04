//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;

use efr_protocol::{ProtocolError, Seq};

/// Every way an `efr-transport` operation can fail.
///
/// Variants carry the data a caller acts on (the path, the uid, the sequence number).
/// The message names what failed in one sentence and leaves the source's text to the
/// `source()` chain.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// The directory that holds the socket could not be created.
    #[error("could not create the socket directory {}", .path.display())]
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A daemon already answers on the socket path, so a second one must not take it.
    #[error("a daemon already listens on {}", .path.display())]
    AddressInUse {
        /// The socket path.
        path: PathBuf,
    },

    /// Something other than a socket is at the socket path. It is left alone, because
    /// removing a file that efr did not create could destroy the user's data.
    #[error("{} exists and is not a socket", .path.display())]
    NotASocket {
        /// The socket path.
        path: PathBuf,
    },

    /// Looking at what is at the socket path failed.
    #[error("could not inspect {}", .path.display())]
    Inspect {
        /// The path.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// The listening socket could not be created.
    #[error("could not bind a Unix socket at {}", .path.display())]
    Bind {
        /// The path the socket was bound at.
        path: PathBuf,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// The socket's mode could not be set to 0600.
    #[error("could not restrict the permissions of {}", .path.display())]
    SetPermissions {
        /// The socket.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// The socket could not be moved to its final path.
    #[error("could not rename {} to {}", .from.display(), .to.display())]
    Rename {
        /// The temporary path.
        from: PathBuf,
        /// The final path.
        to: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// Accepting a connection failed.
    #[error("could not accept a connection")]
    Accept {
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// The kernel did not report the credentials of a connected peer.
    #[error("could not read the credentials of a connected peer")]
    PeerCredentials {
        /// The error from `getsockopt(SO_PEERCRED)`.
        #[source]
        source: nix::errno::Errno,
    },

    /// A process of another user connected. The connection was closed before it could
    /// send a frame.
    #[error("a process of uid {uid} connected, but only uid {allowed} may")]
    PeerRejected {
        /// The peer's uid.
        uid: u32,
        /// The peer's pid, when the kernel reported one.
        pid: Option<u32>,
        /// The only uid that may connect.
        allowed: u32,
    },

    /// Reading from or writing to a connection failed.
    #[error("the connection failed")]
    Io {
        /// The error from the operating system.
        #[from]
        source: io::Error,
    },

    /// A frame could not be encoded, or the byte stream is out of step.
    #[error("a frame broke the protocol")]
    Protocol {
        /// The error from the framing or the JSON encoder.
        #[source]
        source: ProtocolError,
    },

    /// The connection is closed, so no more frames can be sent on it.
    #[error("the connection is closed")]
    Closed,

    /// A subscriber fell behind its bounded queue and its subscription was closed.
    #[error("the subscriber fell behind after seq {last_seq}")]
    Overflow {
        /// The last sequence number that the subscriber received.
        last_seq: Seq,
    },

    /// A unary method tried to send a second result.
    #[error("{method} is unary and has already sent its result")]
    ResultAlreadySent {
        /// The wire name of the method.
        method: &'static str,
    },

    /// A stream-only operation was used on a unary method.
    #[error("{method} is not a streaming method")]
    NotAStream {
        /// The wire name of the method.
        method: &'static str,
    },
}

impl From<ProtocolError> for TransportError {
    fn from(source: ProtocolError) -> Self {
        TransportError::Protocol { source }
    }
}

#[cfg(test)]
mod tests;
