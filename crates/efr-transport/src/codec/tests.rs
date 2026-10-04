use bytes::BytesMut;
use efr_protocol::framing::{self, MAX_FRAME_LEN};
use efr_protocol::{
    ClientFrame, ConversationsList, ErrorBody, ErrorCode, Method, ProtocolError, RequestId,
    ServerFrame,
};
use pretty_assertions::assert_eq;
use proptest::prelude::{any, prop, prop_assert_eq, proptest};
use tokio_util::codec::{Decoder as _, Encoder as _};

use super::{EncodedFrame, ServerCodec};
use crate::TransportError;

fn list(id: u64) -> ClientFrame {
    ClientFrame::Request {
        id: RequestId::new(id),
        method: Method::ConversationsList(ConversationsList { cursor: None, limit: Some(5) }),
    }
}

fn cancel(id: u64) -> ClientFrame {
    ClientFrame::Cancel { id: RequestId::new(id) }
}

/// The length-prefixed bytes of a raw payload, for frames that are not valid JSON.
fn raw(payload: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(payload.len()).unwrap().to_be_bytes().to_vec();
    out.extend_from_slice(payload);
    out
}

/// Feeds `chunks` in order the way `FramedRead` does and collects every item.
fn decode_all(chunks: &[&[u8]]) -> Vec<Result<ClientFrame, ProtocolError>> {
    let mut codec = ServerCodec::new();
    let mut buffer = BytesMut::new();
    let mut items = Vec::new();
    for chunk in chunks {
        buffer.extend_from_slice(chunk);
        while let Some(item) = codec.decode(&mut buffer).unwrap() {
            items.push(item);
        }
    }
    while let Some(item) = codec.decode_eof(&mut buffer).unwrap() {
        items.push(item);
    }
    items
}

#[test]
fn frames_split_across_reads_come_out_whole_and_in_order() {
    let mut bytes = framing::encode(&list(1)).unwrap();
    bytes.extend(framing::encode(&cancel(1)).unwrap());
    let (first, rest) = bytes.split_at(7);
    let frames: Vec<ClientFrame> =
        decode_all(&[first, rest]).into_iter().map(Result::unwrap).collect();
    assert_eq!(frames, vec![list(1), cancel(1)]);
}

#[test]
fn a_malformed_payload_is_an_item_and_the_next_frame_still_decodes() {
    let mut bytes = raw(br#"{"id": 9, "method": "no.such_method", "params": {}}"#);
    bytes.extend(raw(b"not json"));
    bytes.extend(framing::encode(&list(2)).unwrap());
    let items = decode_all(&[&bytes]);
    assert_eq!(items.len(), 3);
    assert!(matches!(items[0], Err(ProtocolError::Decode { id: Some(id), .. }) if id.get() == 9));
    assert!(matches!(items[1], Err(ProtocolError::Decode { id: None, .. })));
    assert_eq!(items[2].as_ref().unwrap(), &list(2));
}

#[test]
fn an_oversized_prefix_ends_the_stream_after_the_frames_before_it() {
    let mut bytes = framing::encode(&list(3)).unwrap();
    bytes.extend(u32::try_from(MAX_FRAME_LEN + 1).unwrap().to_be_bytes());
    let mut codec = ServerCodec::new();
    let mut buffer = BytesMut::from(&bytes[..]);
    assert_eq!(codec.decode(&mut buffer).unwrap().unwrap().unwrap(), list(3));
    let error = codec.decode(&mut buffer).unwrap_err();
    assert!(matches!(
        error,
        TransportError::Protocol { source: ProtocolError::FrameTooLarge { .. } }
    ));
}

#[test]
fn a_stream_that_ends_inside_a_frame_is_an_error() {
    let bytes = framing::encode(&list(4)).unwrap();
    let mut codec = ServerCodec::new();
    let mut buffer = BytesMut::from(&bytes[..bytes.len() - 1]);
    assert!(codec.decode(&mut buffer).unwrap().is_none());
    let error = codec.decode_eof(&mut buffer).unwrap_err();
    assert!(matches!(error, TransportError::Protocol { source: ProtocolError::Truncated { .. } }));
}

#[test]
fn a_stream_that_ends_between_frames_ends_cleanly() {
    let mut codec = ServerCodec::new();
    assert!(codec.decode_eof(&mut BytesMut::new()).unwrap().is_none());
}

#[test]
fn server_frames_encode_exactly_as_the_framing_module_does() {
    let frame = ServerFrame::error(
        Some(RequestId::new(5)),
        ErrorBody::new(ErrorCode::Cancelled, "the request was cancelled"),
    );
    let mut buffer = BytesMut::new();
    ServerCodec::new().encode(frame.clone(), &mut buffer).unwrap();
    assert_eq!(&buffer[..], &framing::encode(&frame).unwrap()[..]);
    assert_eq!(EncodedFrame::new(&frame).unwrap().as_bytes(), &buffer[..]);
}

#[test]
fn an_encoded_frame_over_the_limit_is_refused() {
    let huge = "x".repeat(MAX_FRAME_LEN);
    let frame = ServerFrame::item(RequestId::new(6), &huge).unwrap();
    assert!(matches!(EncodedFrame::new(&frame), Err(ProtocolError::FrameTooLarge { .. })));
    let error = ServerCodec::new().encode(frame, &mut BytesMut::new()).unwrap_err();
    assert!(matches!(
        error,
        TransportError::Protocol { source: ProtocolError::FrameTooLarge { .. } }
    ));
}

proptest! {
    /// Any split of a stream into reads yields the same frames as one read.
    #[test]
    fn any_split_decodes_to_the_same_frames(
        ids in proptest::collection::vec(0_u64..1000, 1..6),
        cuts in proptest::collection::vec(any::<prop::sample::Index>(), 0..8),
    ) {
        let frames: Vec<ClientFrame> = ids
            .iter()
            .enumerate()
            .map(|(n, id)| if n % 2 == 0 { list(*id) } else { cancel(*id) })
            .collect();
        let bytes: Vec<u8> =
            frames.iter().flat_map(|frame| framing::encode(frame).unwrap()).collect();
        let mut points: Vec<usize> = cuts.iter().map(|cut| cut.index(bytes.len())).collect();
        points.sort_unstable();
        let mut chunks = Vec::new();
        let mut start = 0;
        for point in points {
            chunks.push(&bytes[start..point]);
            start = point;
        }
        chunks.push(&bytes[start..]);
        let decoded: Vec<ClientFrame> =
            decode_all(&chunks).into_iter().map(Result::unwrap).collect();
        prop_assert_eq!(decoded, frames);
    }
}
