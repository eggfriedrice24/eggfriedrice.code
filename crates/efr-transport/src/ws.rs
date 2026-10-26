//! The WebSocket side of the protocol edge: one connection of the phone, over a stream
//! that the daemon has accepted and admitted.
//!
//! The daemon owns the TCP listener on the tailnet address and the check of who the
//! peer is (the whois of tailscaled), because both need crates that this one must not
//! depend on. It hands each admitted stream to [`serve_websocket`], which:
//!
//! 1. reads the opening handshake, a bounded HTTP/1.1 request head (`handshake`), with
//!    a time limit on the injected clock, and answers anything but a valid upgrade with
//!    a plain HTTP error;
//! 2. answers `101 Switching Protocols` and starts the frames of fastwebsockets in
//!    server role (`frames`), with the server's pings (`keepalive`);
//! 3. runs the same connection loop as the Unix socket, with the same request table,
//!    cancellation, outbound queue and close grace. One text message carries one JSON
//!    frame.
//!
//! The listener offers no extension (so no permessage-deflate) and no subprotocol.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use efr_stdx::time::Clock;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

use crate::connection::{self, ConnectionParams};
use crate::{ConnId, Dispatcher, RemotePeer, TransportError};

mod frames;
mod handshake;
mod keepalive;

pub use self::frames::WebSocketCloseReason;
pub use self::handshake::WebSocketRefusal;
use self::handshake::{MAX_HEAD_BYTES, Parsed};

/// How long a peer may take to send its request head, and the server to send its
/// answer, before the connection closes without one.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The rules of the WebSocket handshake that depend on where the daemon listens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebSocketOptions {
    origins: Vec<String>,
}

impl WebSocketOptions {
    /// Options that allow only a request without an `Origin` header.
    pub fn new() -> Self {
        WebSocketOptions::default()
    }

    /// Also allows a request whose `Origin` is exactly `origin`, such as
    /// `http://100.101.102.103:7878` for a listener at that address. The compare is
    /// byte for byte, so `null` and every other origin stay refused.
    #[must_use]
    pub fn allow_origin(mut self, origin: impl Into<String>) -> Self {
        self.origins.push(origin.into());
        self
    }

    /// The origins allowed besides a missing one.
    pub fn origins(&self) -> &[String] {
        &self.origins
    }
}

/// Serves one WebSocket connection of `peer` on `stream` until the peer closes it, the
/// stream breaks, the peer breaks a rule or stops answering pings, or `shutdown` is
/// cancelled.
///
/// The daemon calls it for a stream that it has accepted and admitted; the stream's
/// first bytes must be the opening handshake. The connection gets an id of its own
/// ([`ConnId::next`]), and every request carries a context with the remote peer, whose
/// surface is always `phone`.
///
/// Returns an error when the handshake fails: [`TransportError::HandshakeRefused`]
/// after the HTTP error was sent, [`TransportError::HandshakeTimedOut`], or
/// [`TransportError::Io`] when the stream failed or ended first. Once the connection
/// runs, it returns `Ok` however it ends; a broken rule is logged at debug in the
/// connection's span. `clock` times the handshake, the pings and the close grace.
pub async fn serve_websocket<S, D>(
    stream: S,
    peer: RemotePeer,
    options: &WebSocketOptions,
    dispatcher: Arc<D>,
    clock: Arc<dyn Clock>,
    shutdown: CancellationToken,
) -> Result<(), TransportError>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    D: Dispatcher,
{
    let (mut read_half, mut write_half) = tokio::io::split(stream);
    let handshake = handshake(&mut read_half, &mut write_half, options.origins());
    let leftover = tokio::select! {
        biased;
        () = shutdown.cancelled() => return Ok(()),
        done = clock.timeout(HANDSHAKE_TIMEOUT, handshake) => match done {
            Ok(leftover) => leftover?,
            Err(_) => return Err(TransportError::HandshakeTimedOut),
        },
    };
    let params = ConnectionParams {
        conn_id: ConnId::next(),
        peer: peer.into(),
        dispatcher,
        clock: Arc::clone(&clock),
        shutdown: shutdown.clone(),
    };
    let span = connection::span(&params);
    let (source, sink) = {
        let _entered = span.enter();
        // The bytes after the head are the first frames.
        let read = Cursor::new(leftover).chain(read_half);
        frames::start(read, write_half, clock, shutdown)
    };
    connection::run_frames(source, sink, params, span).await;
    Ok(())
}

/// Reads the request head and answers it. Returns the bytes read after the head, or the
/// error after the HTTP error was sent.
async fn handshake<R, W>(
    read: &mut R,
    write: &mut W,
    origins: &[String],
) -> Result<Vec<u8>, TransportError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = Vec::with_capacity(1024);
    let upgrade = loop {
        match handshake::parse(&buf, origins) {
            Ok(Parsed::Complete(upgrade)) => break upgrade,
            Ok(Parsed::Partial) => {}
            Err(refusal) => {
                write.write_all(&handshake::refusal_response(refusal)).await?;
                write.shutdown().await?;
                return Err(TransportError::HandshakeRefused { refusal });
            }
        }
        // NOTE: the read never goes past 8 KiB, so a head that is not complete there is
        // too large.
        let room = MAX_HEAD_BYTES - buf.len();
        let limit = u64::try_from(room).unwrap_or(u64::MAX);
        let read_now = (&mut *read).take(limit).read_buf(&mut buf).await?;
        if read_now == 0 {
            return Err(TransportError::Io { source: std::io::ErrorKind::UnexpectedEof.into() });
        }
    };
    let leftover = buf.split_off(upgrade.head_len);
    write.write_all(&handshake::accept_response(&upgrade)).await?;
    write.flush().await?;
    Ok(leftover)
}

#[cfg(test)]
mod tests;
