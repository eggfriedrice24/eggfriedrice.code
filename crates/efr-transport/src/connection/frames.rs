//! The two ends of a connection's wire: where client frames come from and where server
//! frames go.
//!
//! The connection loop does not know the wire. The Unix socket carries length-prefixed
//! frames through [`ServerCodec`] and [`PrefixedSink`]; the WebSocket carries one JSON
//! frame per text message through the source and sink of `crate::ws`. Both get the
//! same request table, cancellation, outbound queue and close grace.

use std::future::Future;
use std::io;

use efr_protocol::{ClientFrame, ProtocolError};
use futures::StreamExt as _;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _, BufWriter};
use tokio::sync::mpsc;
use tokio_util::codec::FramedRead;

use crate::TransportError;
use crate::codec::{EncodedFrame, ServerCodec};

/// What reading one frame payload gives: the client frame, or why the payload is not a
/// valid client frame. The stream is still in step after either.
pub(crate) type Decoded = Result<ClientFrame, ProtocolError>;

/// Where a connection reads its client frames.
pub(crate) trait FrameSource: Send + 'static {
    /// The next frame. `None` once the peer has closed its side, and an error when the
    /// wire is out of step or broken; nothing more can be read after either.
    ///
    /// It must be cancel safe: the connection loop races it against its other work,
    /// and a frame that was not returned must come with the next call.
    fn next_frame(
        &mut self,
    ) -> impl Future<Output = Option<Result<Decoded, TransportError>>> + Send;
}

/// Where a connection writes its server frames.
pub(crate) trait FrameSink: Send + 'static {
    /// Writes each frame of `queue` in order until every sender is gone, then ends the
    /// wire. An error ends the connection.
    fn drain(
        self,
        queue: mpsc::Receiver<EncodedFrame>,
    ) -> impl Future<Output = io::Result<()>> + Send;
}

// NOTE: `FramedRead::next` is cancel safe: the codec keeps every byte it has read in
// its buffer until a whole frame is in.
impl<R> FrameSource for FramedRead<R, ServerCodec>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    fn next_frame(
        &mut self,
    ) -> impl Future<Output = Option<Result<Decoded, TransportError>>> + Send {
        self.next()
    }
}

/// The sink of the Unix socket: each frame with its length prefix, batched into one
/// flush with whatever is already queued.
#[derive(Debug)]
pub(crate) struct PrefixedSink<W> {
    write_half: W,
}

impl<W> PrefixedSink<W> {
    pub(crate) fn new(write_half: W) -> Self {
        PrefixedSink { write_half }
    }
}

impl<W> FrameSink for PrefixedSink<W>
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    async fn drain(self, mut queue: mpsc::Receiver<EncodedFrame>) -> io::Result<()> {
        let mut writer = BufWriter::new(self.write_half);
        while let Some(frame) = queue.recv().await {
            writer.write_all(frame.as_bytes()).await?;
            while let Ok(frame) = queue.try_recv() {
                writer.write_all(frame.as_bytes()).await?;
            }
            writer.flush().await?;
        }
        writer.shutdown().await
    }
}
