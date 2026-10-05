//! The tokio codec of the daemon side: client frames in, server frames out.
//!
//! The byte format belongs to `efr_protocol::framing`; this module only adapts its push
//! decoder and its encoder to `tokio_util::codec`, so a change of encoding still
//! touches one module in the protocol crate.

use std::collections::VecDeque;

use bytes::BytesMut;
use efr_protocol::framing::{self, Decoder as FrameDecoder};
use efr_protocol::{ClientFrame, ProtocolError, ServerFrame};
use tokio_util::codec::{Decoder, Encoder};
use zeroize::{Zeroize as _, Zeroizing};

use crate::TransportError;

/// Decodes client frames and encodes server frames on a byte stream.
///
/// Each decoded item is the result of reading one frame payload. A payload that is not
/// a valid client frame is an `Err` item, not a codec error, because the length prefix
/// was valid and the next frame still starts at the right byte; the connection answers
/// it with `invalid` and reads on. A codec error (an oversized prefix, a stream that
/// ends inside a frame, an IO failure) leaves the stream out of step and ends it.
///
/// A client frame can carry a password typed for `input.respond`, so the bytes it read
/// are overwritten with zeros once they are consumed, and a payload once it is decoded
/// or dropped.
#[derive(Debug, Default)]
pub struct ServerCodec {
    framing: FrameDecoder,
    ready: VecDeque<Zeroizing<Vec<u8>>>,
}

impl ServerCodec {
    /// A codec at the start of a stream.
    pub fn new() -> Self {
        ServerCodec::default()
    }
}

impl Decoder for ServerCodec {
    type Item = Result<ClientFrame, ProtocolError>;
    type Error = TransportError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let payload = next_payload(&mut self.framing, &mut self.ready, src)?;
        Ok(payload.map(|payload| ClientFrame::from_json(&payload)))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(frame) = self.decode(src)? {
            return Ok(Some(frame));
        }
        self.framing.finish()?;
        Ok(None)
    }
}

impl Encoder<ServerFrame> for ServerCodec {
    type Error = TransportError;

    fn encode(&mut self, frame: ServerFrame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        dst.extend_from_slice(&framing::encode(&frame)?);
        Ok(())
    }
}

/// The next whole payload, feeding every buffered byte to the push decoder first.
///
/// The push decoder copies what it needs, so `src` is always drained, and zeroed first.
/// It is called even when `src` is empty, because a chunk that completes some frames
/// and then announces an oversized one reports the error on the following call.
fn next_payload(
    framing: &mut FrameDecoder,
    ready: &mut VecDeque<Zeroizing<Vec<u8>>>,
    src: &mut BytesMut,
) -> Result<Option<Zeroizing<Vec<u8>>>, ProtocolError> {
    if ready.is_empty() {
        ready.extend(framing.push(src)?.into_iter().map(Zeroizing::new));
        src.as_mut().zeroize();
        src.clear();
    }
    Ok(ready.pop_front())
}

/// A server frame already encoded behind its length prefix.
///
/// Responders encode before they queue a frame, so a result too large for the wire
/// fails in the handler that made it instead of breaking the connection's writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EncodedFrame(Vec<u8>);

impl EncodedFrame {
    pub(crate) fn new(frame: &ServerFrame) -> Result<Self, ProtocolError> {
        framing::encode(frame).map(EncodedFrame)
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests;
