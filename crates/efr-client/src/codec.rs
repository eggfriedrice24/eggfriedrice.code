//! The tokio codec of the client side: client frames out, server frames in.
//!
//! The byte format belongs to `efr_protocol::framing`; this module adapts its push
//! decoder and its encoder to `tokio_util::codec`. `efr-transport` has the mirror image
//! for the daemon; the two cannot share code because the client may not depend on the
//! transport.

use std::collections::VecDeque;

use bytes::BytesMut;
use efr_protocol::framing::{self, Decoder as FrameDecoder};
use efr_protocol::{ClientFrame, ProtocolError, ServerFrame};
use tokio_util::codec::{Decoder, Encoder};

use crate::ClientError;

/// Encodes client frames and decodes server frames on a byte stream.
///
/// Each decoded item is the result of reading one frame payload. A payload that is not
/// a valid server frame is an `Err` item, not a codec error, because the next frame
/// still starts at the right byte; the client fails the one request it names. A codec
/// error (an oversized prefix, a stream that ends inside a frame, an IO failure) leaves
/// the stream out of step and ends it.
#[derive(Debug, Default)]
pub struct ClientCodec {
    framing: FrameDecoder,
    ready: VecDeque<Vec<u8>>,
}

impl ClientCodec {
    /// A codec at the start of a stream.
    pub fn new() -> Self {
        ClientCodec::default()
    }

    /// The next whole payload. Every buffered byte goes to the push decoder, which
    /// copies what it needs, so `src` is always drained. The decoder is fed even an
    /// empty buffer, because a chunk that completes some frames and then announces an
    /// oversized one reports the error on the following call.
    fn next_payload(&mut self, src: &mut BytesMut) -> Result<Option<Vec<u8>>, ProtocolError> {
        if self.ready.is_empty() {
            self.ready.extend(self.framing.push(src)?);
            src.clear();
        }
        Ok(self.ready.pop_front())
    }
}

impl Decoder for ClientCodec {
    type Item = Result<ServerFrame, ProtocolError>;
    type Error = ClientError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let payload = self.next_payload(src)?;
        Ok(payload.map(|payload| ServerFrame::from_json(&payload)))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(frame) = self.decode(src)? {
            return Ok(Some(frame));
        }
        self.framing.finish()?;
        Ok(None)
    }
}

impl Encoder<ClientFrame> for ClientCodec {
    type Error = ClientError;

    fn encode(&mut self, frame: ClientFrame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        dst.extend_from_slice(&framing::encode(&frame)?);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
