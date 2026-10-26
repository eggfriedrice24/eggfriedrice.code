//! The frames of a WebSocket after its handshake, in server role.
//!
//! One message carries one JSON frame, the same frames as on the Unix socket. The
//! reader takes whole text messages only: a binary frame, a frame without FIN, a
//! continuation frame, a message larger than [`MAX_INBOUND_MESSAGE`] and a client frame
//! without a mask close the connection with their close code.
//!
//! The frame parser of fastwebsockets is not cancel safe, so it never runs inside the
//! connection loop's `select!`. A reader task parses frames and fills a bounded queue,
//! and [`WsSource`] reads that queue, which is cancel safe. The reader answers pings
//! through the writer and hands pongs to the keepalive. [`WsSink`] is the writer: it
//! sends each server frame's bare JSON body as one text message, the reader's pongs
//! and close echoes, and the keepalive's pings, and it sends the close frame last.
//!
//! Outbound messages are at most 16 MiB, because [`EncodedFrame`] refuses a larger
//! frame in the handler that made it.

use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use efr_protocol::ClientFrame;
use efr_stdx::time::Clock;
use fastwebsockets::{
    Frame, OpCode, Payload, Role, WebSocketError, WebSocketRead, WebSocketWrite,
    after_handshake_split,
};
use tokio::io::{AsyncRead, AsyncWrite, BufWriter};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;
use zeroize::Zeroize as _;

use crate::TransportError;
use crate::codec::EncodedFrame;
use crate::connection::{Decoded, FrameSink, FrameSource};
use crate::ws::keepalive::{self, Pings};

/// The largest message a client may send: 1 MiB.
pub(crate) const MAX_INBOUND_MESSAGE: usize = 1024 * 1024;

/// How many decoded frames wait for the connection loop before the reader stops
/// reading, so a slow loop slows the socket instead of filling memory.
const INBOUND_FRAMES: usize = 16;

/// How many control frames (pongs, pings, close echoes) wait for the writer.
const CONTROL_FRAMES: usize = 16;

/// The longest frame head that fastwebsockets writes: 2 bytes, an 8-byte length and a
/// 4-byte mask key, with room to spare.
const HEAD_BYTES: usize = 16;

/// Why the listener closed a WebSocket. Each reason has its close code (RFC 6455,
/// section 7.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WebSocketCloseReason {
    /// The client sent a binary frame; frames are JSON text.
    Binary,
    /// The client sent a frame without FIN or a continuation frame; every message must
    /// be one frame.
    Fragmented,
    /// The client sent a frame without a mask, which RFC 6455 forbids.
    Unmasked,
    /// The client sent a message larger than 1 MiB.
    TooLarge,
    /// The client sent a text message that is not UTF-8.
    InvalidUtf8,
    /// The client broke another rule of the WebSocket framing.
    Protocol,
    /// The client did not answer the server's pings.
    PingTimeout,
}

impl WebSocketCloseReason {
    /// The close code that the close frame carries.
    pub const fn code(self) -> u16 {
        match self {
            WebSocketCloseReason::Binary | WebSocketCloseReason::Fragmented => 1003,
            WebSocketCloseReason::Unmasked | WebSocketCloseReason::Protocol => 1002,
            WebSocketCloseReason::TooLarge => 1009,
            WebSocketCloseReason::InvalidUtf8 => 1007,
            WebSocketCloseReason::PingTimeout => 1001,
        }
    }

    /// The reason text of the close frame, at most 123 bytes.
    pub(crate) const fn text(self) -> &'static str {
        match self {
            WebSocketCloseReason::Binary => "binary frames are not served",
            WebSocketCloseReason::Fragmented => "fragmented messages are not served",
            WebSocketCloseReason::Unmasked => "client frames must be masked",
            WebSocketCloseReason::TooLarge => "the message is larger than 1 MiB",
            WebSocketCloseReason::InvalidUtf8 => "the text is not UTF-8",
            WebSocketCloseReason::Protocol => "the frame breaks the WebSocket protocol",
            WebSocketCloseReason::PingTimeout => "the client did not answer the pings",
        }
    }
}

impl fmt::Display for WebSocketCloseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text())
    }
}

/// The reason the connection closes, which the writer puts into its close frame. The
/// first reason recorded wins.
#[derive(Debug, Clone, Default)]
pub(crate) struct CloseSlot(Arc<Mutex<Option<WebSocketCloseReason>>>);

impl CloseSlot {
    pub(crate) fn set(&self, reason: WebSocketCloseReason) {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        slot.get_or_insert(reason);
    }

    fn get(&self) -> Option<WebSocketCloseReason> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One item of the reader's queue.
type Item = Result<Decoded, TransportError>;

/// Starts the reader and the keepalive on a WebSocket whose handshake is done, and
/// returns the connection's source and sink. `stopping` stops the connection from the
/// daemon's side: the writer then drops the frames still queued and closes with 1001.
pub(crate) fn start<R, W>(
    read: R,
    write: W,
    clock: Arc<dyn Clock>,
    stopping: CancellationToken,
) -> (WsSource, WsSink<BufWriter<W>>)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (mut read, write) = after_handshake_split(read, BufWriter::new(write), Role::Server);
    // NOTE: fastwebsockets refuses a frame whose length is at least its limit, so a
    // message of exactly 1 MiB needs a limit one byte larger.
    read.set_max_message_size(MAX_INBOUND_MESSAGE + 1);
    // The reader answers pings itself, so it can check their mask first.
    read.set_auto_pong(false);
    let (items, frames) = mpsc::channel(INBOUND_FRAMES);
    let (control_tx, control) = mpsc::channel(CONTROL_FRAMES);
    let close = CloseSlot::default();
    let pings = Arc::new(Pings::default());
    let dead = CancellationToken::new();
    let reader = tokio::spawn(
        read_task(read, items, control_tx.clone(), Arc::clone(&pings), close.clone(), dead.clone())
            .in_current_span(),
    );
    let keepalive = tokio::spawn(
        keepalive::run(clock, pings, control_tx, close.clone(), dead).in_current_span(),
    );
    let source = WsSource { frames, reader, keepalive };
    (source, WsSink { write, control, close, stopping })
}

/// The connection's source on a WebSocket: the reader's queue. Dropping it stops the
/// reader and the keepalive.
#[derive(Debug)]
pub(crate) struct WsSource {
    frames: mpsc::Receiver<Item>,
    reader: JoinHandle<()>,
    keepalive: JoinHandle<()>,
}

impl FrameSource for WsSource {
    // NOTE: `Receiver::recv` is cancel safe: an item that was not returned stays queued.
    fn next_frame(&mut self) -> impl Future<Output = Option<Item>> + Send {
        self.frames.recv()
    }
}

impl Drop for WsSource {
    fn drop(&mut self) {
        self.reader.abort();
        self.keepalive.abort();
    }
}

/// Reads frames until the peer closes, breaks a rule or stops answering pings, then
/// queues the error that ends the connection, if any.
async fn read_task<R>(
    mut read: WebSocketRead<R>,
    items: mpsc::Sender<Item>,
    control: mpsc::Sender<Frame<'static>>,
    pings: Arc<Pings>,
    close: CloseSlot,
    dead: CancellationToken,
) where
    R: AsyncRead + Unpin,
{
    // NOTE: the read loop is cancelled only here, and never polled again after, so the
    // parser's state cannot break.
    let last = tokio::select! {
        biased;
        () = dead.cancelled() => Some(closed(WebSocketCloseReason::PingTimeout)),
        last = read_loop(&mut read, &items, &control, &pings) => last,
    };
    if let Some(error) = last {
        if let TransportError::WebSocketClosed { reason } = &error {
            close.set(*reason);
        }
        let _ = items.send(Err(error)).await;
    }
}

/// The frames of the peer, until one ends the connection. Returns the error to report,
/// or `None` when the peer closed or the connection went away.
async fn read_loop<R>(
    read: &mut WebSocketRead<R>,
    items: &mpsc::Sender<Item>,
    control: &mpsc::Sender<Frame<'static>>,
    pings: &Pings,
) -> Option<TransportError>
where
    R: AsyncRead + Unpin,
{
    // NOTE: the parser asks for the echo of a close through this closure; the writer
    // sends it in order with everything else.
    let mut obligated = |frame: Frame<'static>| {
        let control = control.clone();
        async move { control.send(frame).await.map_err(|_| WebSocketError::ConnectionClosed) }
    };
    loop {
        let mut frame = match read.read_frame(&mut obligated).await {
            Ok(frame) => frame,
            Err(error) => return failure(error),
        };
        if !is_masked(&mut frame) {
            return Some(closed(WebSocketCloseReason::Unmasked));
        }
        match frame.opcode {
            OpCode::Text if frame.fin => {
                let decoded = ClientFrame::from_json(&frame.payload);
                // NOTE: a frame can carry a password typed for input.respond, so the
                // payload is zeroed once it is decoded. Not zeroed: the bytes left in
                // the parser's read buffer when it grows (fastwebsockets reserves and
                // can move it), and serde_json's scratch buffer for a string with an
                // escape.
                frame.payload.to_mut().zeroize();
                if items.send(Ok(decoded)).await.is_err() {
                    return None;
                }
            }
            OpCode::Text | OpCode::Continuation => {
                return Some(closed(WebSocketCloseReason::Fragmented));
            }
            OpCode::Binary => return Some(closed(WebSocketCloseReason::Binary)),
            OpCode::Ping => {
                let pong = Frame::pong(Payload::Owned(frame.payload.to_vec()));
                if control.send(pong).await.is_err() {
                    return None;
                }
            }
            OpCode::Pong => pings.answer(&frame.payload),
            // The parser has queued the echo; the peer sends nothing more.
            OpCode::Close => return None,
        }
    }
}

/// The error to report for a failed read: a close reason for a broken rule, the IO
/// error, or nothing when the peer just went away.
fn failure(error: WebSocketError) -> Option<TransportError> {
    let reason = match error {
        WebSocketError::UnexpectedEOF
        | WebSocketError::ConnectionClosed
        | WebSocketError::SendError(_) => return None,
        WebSocketError::IoError(source) => return Some(TransportError::Io { source }),
        WebSocketError::FrameTooLarge => WebSocketCloseReason::TooLarge,
        WebSocketError::InvalidUTF8 => WebSocketCloseReason::InvalidUtf8,
        _ => WebSocketCloseReason::Protocol,
    };
    Some(closed(reason))
}

fn closed(reason: WebSocketCloseReason) -> TransportError {
    TransportError::WebSocketClosed { reason }
}

/// True when the client masked `frame`.
///
/// NOTE: fastwebsockets 0.10.0 accepts unmasked client frames and keeps the mask key
/// private, but it keeps the key on the frame after it unmasks the payload, and
/// `fmt_head` writes the mask bit from it. The tests send an unmasked frame to hold
/// this.
fn is_masked(frame: &mut Frame<'_>) -> bool {
    let mut head = [0_u8; HEAD_BYTES];
    frame.fmt_head(&mut head);
    head.get(1).is_some_and(|byte| byte & 0x80 != 0)
}

/// The connection's sink on a WebSocket: the writer.
pub(crate) struct WsSink<W> {
    write: WebSocketWrite<W>,
    control: mpsc::Receiver<Frame<'static>>,
    close: CloseSlot,
    stopping: CancellationToken,
}

impl<W> fmt::Debug for WsSink<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsSink").field("closed", &self.write.is_closed()).finish_non_exhaustive()
    }
}

impl<W> FrameSink for WsSink<W>
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    async fn drain(self, mut queue: mpsc::Receiver<EncodedFrame>) -> io::Result<()> {
        let WsSink { mut write, mut control, close, stopping } = self;
        let mut control_open = true;
        loop {
            tokio::select! {
                biased;
                // NOTE: once the connection is stopped from the daemon's side (a revoked
                // device, or the daemon's shutdown), the frames still queued are
                // dropped: the phone resubscribes after the last seq it has.
                () = stopping.cancelled() => break,
                frame = control.recv(), if control_open => match frame {
                    Some(frame) => {
                        send(&mut write, frame).await?;
                        flush(&mut write).await?;
                    }
                    None => control_open = false,
                },
                frame = queue.recv() => {
                    let Some(frame) = frame else {
                        break;
                    };
                    send(&mut write, text(&frame)).await?;
                    while !stopping.is_cancelled()
                        && let Ok(frame) = queue.try_recv()
                    {
                        send(&mut write, text(&frame)).await?;
                    }
                    flush(&mut write).await?;
                }
            }
        }
        let (code, reason) = match close.get() {
            Some(reason) => (reason.code(), reason.text()),
            None if stopping.is_cancelled() => (1001, "the server closed the connection"),
            None => (1000, ""),
        };
        send(&mut write, Frame::close(code, reason.as_bytes())).await?;
        flush(&mut write).await
    }
}

/// A text message with the bare JSON body of `frame`.
fn text(frame: &EncodedFrame) -> Frame<'_> {
    Frame::text(Payload::Borrowed(frame.body()))
}

/// Writes `frame`, unless a close frame was written already: after it nothing more may
/// be sent (RFC 6455, section 5.5.1).
async fn send<W>(write: &mut WebSocketWrite<W>, frame: Frame<'_>) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    if write.is_closed() {
        return Ok(());
    }
    write.write_frame(frame).await.map_err(io_error)
}

async fn flush<W>(write: &mut WebSocketWrite<W>) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    write.flush().await.map_err(io_error)
}

fn io_error(error: WebSocketError) -> io::Error {
    match error {
        WebSocketError::IoError(source) => source,
        other => io::Error::other(other),
    }
}
