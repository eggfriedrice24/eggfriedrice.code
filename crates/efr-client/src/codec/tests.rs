use bytes::BytesMut;
use efr_protocol::framing::{self, MAX_FRAME_LEN};
use efr_protocol::{
    ClientFrame, ConversationsList, ErrorBody, ErrorCode, Method, ProtocolError, RequestId,
    ServerFrame,
};
use efr_transport::ServerCodec;
use pretty_assertions::assert_eq;
use proptest::prelude::{any, prop, prop_assert_eq, proptest};
use serde_json::json;
use tokio_util::codec::{Decoder as _, Encoder as _};

use super::ClientCodec;
use crate::ClientError;

fn server_frames() -> Vec<ServerFrame> {
    vec![
        ServerFrame::Ack { id: RequestId::new(1) },
        ServerFrame::Item { id: RequestId::new(1), item: json!({ "kind": "event", "seq": 3 }) },
        ServerFrame::end(RequestId::new(1)),
        ServerFrame::error(
            Some(RequestId::new(2)),
            ErrorBody::new(ErrorCode::Busy, "the daemon is starting"),
        ),
    ]
}

/// Feeds `chunks` in order the way `FramedRead` does and collects every item.
fn decode_all(chunks: &[&[u8]]) -> Vec<Result<ServerFrame, ProtocolError>> {
    let mut codec = ClientCodec::new();
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
fn client_frames_decode_on_the_daemon_side() {
    let frames = vec![
        ClientFrame::Request {
            id: RequestId::new(7),
            method: Method::ConversationsList(ConversationsList { cursor: None, limit: Some(2) }),
        },
        ClientFrame::Cancel { id: RequestId::new(7) },
    ];
    let mut buffer = BytesMut::new();
    let mut client = ClientCodec::new();
    for frame in &frames {
        client.encode(frame.clone(), &mut buffer).unwrap();
    }
    let mut server = ServerCodec::new();
    let mut decoded = Vec::new();
    while let Some(frame) = server.decode(&mut buffer).unwrap() {
        decoded.push(frame.unwrap());
    }
    assert_eq!(decoded, frames);
}

#[test]
fn daemon_frames_decode_on_the_client_side() {
    let mut buffer = BytesMut::new();
    let mut server = ServerCodec::new();
    for frame in server_frames() {
        server.encode(frame, &mut buffer).unwrap();
    }
    let decoded: Vec<ServerFrame> =
        decode_all(&[&buffer[..]]).into_iter().map(Result::unwrap).collect();
    assert_eq!(decoded, server_frames());
}

#[test]
fn a_malformed_payload_is_an_item_that_keeps_its_id() {
    let payload = br#"{"id": 4, "item": 1, "end": true}"#;
    let mut bytes = u32::try_from(payload.len()).unwrap().to_be_bytes().to_vec();
    bytes.extend_from_slice(payload);
    bytes.extend(framing::encode(&ServerFrame::end(RequestId::new(5))).unwrap());
    let items = decode_all(&[&bytes]);
    assert!(matches!(items[0], Err(ProtocolError::Decode { id: Some(id), .. }) if id.get() == 4));
    assert_eq!(items[1].as_ref().unwrap(), &ServerFrame::end(RequestId::new(5)));
}

#[test]
fn an_oversized_prefix_is_a_codec_error() {
    let mut codec = ClientCodec::new();
    let mut buffer = BytesMut::from(&u32::try_from(MAX_FRAME_LEN + 1).unwrap().to_be_bytes()[..]);
    assert!(matches!(
        codec.decode(&mut buffer).unwrap_err(),
        ClientError::Protocol { source: ProtocolError::FrameTooLarge { .. } }
    ));
}

#[test]
fn a_stream_that_ends_inside_a_frame_is_a_codec_error() {
    let bytes = framing::encode(&ServerFrame::end(RequestId::new(1))).unwrap();
    let mut codec = ClientCodec::new();
    let mut buffer = BytesMut::from(&bytes[..3]);
    assert!(codec.decode(&mut buffer).unwrap().is_none());
    assert!(matches!(
        codec.decode_eof(&mut buffer).unwrap_err(),
        ClientError::Protocol { source: ProtocolError::Truncated { .. } }
    ));
}

proptest! {
    /// Any split of a stream into reads yields the same frames as one read.
    #[test]
    fn any_split_decodes_to_the_same_frames(
        cuts in proptest::collection::vec(any::<prop::sample::Index>(), 0..8),
    ) {
        let bytes: Vec<u8> = server_frames()
            .iter()
            .flat_map(|frame| framing::encode(frame).unwrap())
            .collect();
        let mut points: Vec<usize> = cuts.iter().map(|cut| cut.index(bytes.len())).collect();
        points.sort_unstable();
        let mut chunks = Vec::new();
        let mut start = 0;
        for point in points {
            chunks.push(&bytes[start..point]);
            start = point;
        }
        chunks.push(&bytes[start..]);
        let decoded: Vec<ServerFrame> =
            decode_all(&chunks).into_iter().map(Result::unwrap).collect();
        prop_assert_eq!(decoded, server_frames());
    }
}
