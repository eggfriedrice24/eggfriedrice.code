//! The length-prefix framing of the Unix socket, as pure functions over bytes.
//!
//! A frame is a 4-byte big-endian payload length followed by the payload, one JSON
//! object of at most [`MAX_FRAME_LEN`] bytes. The prefix is fixed-width rather than a
//! varint so that `socat` and `jq` can read a captured stream. Nothing here does IO:
//! `efr-transport` and `efr-client` wrap [`encode`] and [`Decoder`] in a tokio codec,
//! and this module is the only place a change of encoding would touch.
//!
//! ```
//! use efr_protocol::framing::{Decoder, encode};
//! use efr_protocol::{ClientFrame, RequestId};
//!
//! let frame = ClientFrame::Cancel { id: RequestId::new(7) };
//! let bytes = encode(&frame)?;
//! assert_eq!(&bytes[..4], &[0, 0, 0, 12]);
//!
//! // Bytes may arrive in any chunks; frames come out whole.
//! let mut decoder = Decoder::new();
//! assert!(decoder.push(&bytes[..6])?.is_empty());
//! let payloads = decoder.push(&bytes[6..])?;
//! assert_eq!(ClientFrame::from_json(&payloads[0])?, frame);
//! decoder.finish()?;
//! # Ok::<(), efr_protocol::ProtocolError>(())
//! ```

use serde::Serialize;

use crate::ProtocolError;

/// The length of the prefix in bytes.
pub const HEADER_LEN: usize = 4;

/// The largest payload a frame may carry: 16 MiB.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// How much the decoder reserves for a body before its bytes arrive, so a prefix that
/// announces 16 MiB does not allocate 16 MiB up front.
const INITIAL_BODY_CAPACITY: usize = 64 * 1024;

/// Encodes `frame` as JSON behind its length prefix, ready to write to the socket.
///
/// Fails when the frame cannot be written as JSON or is larger than [`MAX_FRAME_LEN`].
pub fn encode<F: Serialize + ?Sized>(frame: &F) -> Result<Vec<u8>, ProtocolError> {
    let mut out = vec![0; HEADER_LEN];
    serde_json::to_writer(&mut out, frame).map_err(|source| ProtocolError::Encode { source })?;
    let prefix = prefix_for(out.len().saturating_sub(HEADER_LEN))?;
    if let Some(head) = out.first_chunk_mut::<HEADER_LEN>() {
        *head = prefix;
    }
    Ok(out)
}

/// The prefix for a payload of `len` bytes.
fn prefix_for(len: usize) -> Result<[u8; HEADER_LEN], ProtocolError> {
    let too_large = ProtocolError::FrameTooLarge { len, max: MAX_FRAME_LEN };
    if len > MAX_FRAME_LEN {
        return Err(too_large);
    }
    let len = u32::try_from(len).map_err(|_| too_large)?;
    Ok(len.to_be_bytes())
}

/// Turns a byte stream that arrives in arbitrary chunks back into frame payloads.
///
/// Feed every chunk to [`push`](Self::push) in order, and call [`finish`](Self::finish)
/// when the stream ends. A prefix larger than [`MAX_FRAME_LEN`] is an error as soon as
/// its 4 bytes are in, before any of the body is buffered. After that error the stream
/// is out of step: the decoder returns the error for every later call, and the peer
/// closes the connection.
#[derive(Debug, Default)]
pub struct Decoder {
    state: State,
}

#[derive(Debug)]
enum State {
    /// Reading the prefix; `filled` of its bytes are in.
    Header { bytes: [u8; HEADER_LEN], filled: usize },
    /// Reading a body of `len` bytes.
    Body { len: usize, buf: Vec<u8> },
    /// A prefix announced a payload of `len` bytes, over the limit.
    Failed { len: usize },
}

impl Default for State {
    fn default() -> Self {
        State::Header { bytes: [0; HEADER_LEN], filled: 0 }
    }
}

impl Decoder {
    /// A decoder at the start of a stream.
    pub fn new() -> Self {
        Decoder::default()
    }

    /// Feeds the next chunk of the stream and returns the payloads of every frame that
    /// it completes, in order.
    ///
    /// When the chunk completes some frames and then hits an oversized prefix, this
    /// call returns those frames and the next call, `push(&[])` included, returns the
    /// error, so no complete frame is lost.
    pub fn push(&mut self, mut bytes: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let mut frames = Vec::new();
        loop {
            match &mut self.state {
                State::Failed { len } => {
                    let error = ProtocolError::FrameTooLarge { len: *len, max: MAX_FRAME_LEN };
                    return if frames.is_empty() { Err(error) } else { Ok(frames) };
                }
                State::Header { bytes: header, filled } => {
                    let Some(slot) = header.get_mut(*filled..) else {
                        return Ok(frames);
                    };
                    if bytes.is_empty() {
                        return Ok(frames);
                    }
                    let take = slot.len().min(bytes.len());
                    let (head, rest) = bytes.split_at(take);
                    slot[..take].copy_from_slice(head);
                    *filled += take;
                    bytes = rest;
                    if *filled == HEADER_LEN {
                        let len =
                            usize::try_from(u32::from_be_bytes(*header)).unwrap_or(usize::MAX);
                        self.state = if len > MAX_FRAME_LEN {
                            State::Failed { len }
                        } else {
                            State::Body {
                                len,
                                buf: Vec::with_capacity(len.min(INITIAL_BODY_CAPACITY)),
                            }
                        };
                    }
                }
                State::Body { len, buf } => {
                    let take = len.saturating_sub(buf.len()).min(bytes.len());
                    let (head, rest) = bytes.split_at(take);
                    buf.extend_from_slice(head);
                    bytes = rest;
                    if buf.len() < *len {
                        return Ok(frames);
                    }
                    frames.push(std::mem::take(buf));
                    self.state = State::default();
                }
            }
        }
    }

    /// How many bytes of an unfinished frame the decoder holds, prefix included.
    pub fn buffered(&self) -> usize {
        match &self.state {
            State::Header { filled, .. } => *filled,
            State::Body { buf, .. } => HEADER_LEN + buf.len(),
            State::Failed { .. } => 0,
        }
    }

    /// Checks the end of the stream: `Ok` when it ended between frames, an error when it
    /// ended inside one or after an oversized prefix.
    pub fn finish(&self) -> Result<(), ProtocolError> {
        match &self.state {
            State::Header { filled: 0, .. } => Ok(()),
            State::Failed { len } => {
                Err(ProtocolError::FrameTooLarge { len: *len, max: MAX_FRAME_LEN })
            }
            State::Header { .. } | State::Body { .. } => {
                Err(ProtocolError::Truncated { buffered: self.buffered() })
            }
        }
    }
}

#[cfg(test)]
mod tests;
