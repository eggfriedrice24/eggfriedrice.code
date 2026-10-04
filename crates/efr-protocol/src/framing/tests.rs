use pretty_assertions::assert_eq;
use proptest::collection::vec;
use proptest::prelude::{ProptestConfig, any, prop, prop_assert, prop_assert_eq, proptest};
use serde_json::json;

use super::{Decoder, HEADER_LEN, MAX_FRAME_LEN, encode};
use crate::{AdminStatus, ClientFrame, Method, ProtocolError, RequestId, ServerFrame};

/// A frame around raw payload bytes, which need not be JSON: the framing does not care.
fn framed(payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).unwrap();
    let mut out = len.to_be_bytes().to_vec();
    out.extend_from_slice(payload);
    out
}

/// Splits `stream` at the given cut points, in any order and with repeats.
fn split<'a>(stream: &'a [u8], cuts: &[prop::sample::Index]) -> Vec<&'a [u8]> {
    let mut points: Vec<usize> = cuts.iter().map(|cut| cut.index(stream.len() + 1)).collect();
    points.push(0);
    points.push(stream.len());
    points.sort_unstable();
    points.dedup();
    points.windows(2).map(|pair| &stream[pair[0]..pair[1]]).collect()
}

/// Pushes every chunk and collects the frames, stopping at the first error.
fn decode_chunks(decoder: &mut Decoder, chunks: &[&[u8]]) -> (Vec<Vec<u8>>, Option<ProtocolError>) {
    let mut frames = Vec::new();
    for chunk in chunks.iter().copied().chain([&[][..]]) {
        match decoder.push(chunk) {
            Ok(found) => frames.extend(found),
            Err(error) => return (frames, Some(error)),
        }
    }
    (frames, None)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn any_split_of_a_stream_decodes_to_the_same_frames(
        payloads in vec(vec(any::<u8>(), 0..300), 0..6),
        cuts in vec(any::<prop::sample::Index>(), 0..16),
    ) {
        let stream: Vec<u8> = payloads.iter().flat_map(|payload| framed(payload)).collect();
        let mut decoder = Decoder::new();
        let (frames, error) = decode_chunks(&mut decoder, &split(&stream, &cuts));
        prop_assert!(error.is_none());
        prop_assert_eq!(frames, payloads);
        prop_assert!(decoder.finish().is_ok());
        prop_assert_eq!(decoder.buffered(), 0);
    }

    #[test]
    fn an_oversized_prefix_fails_after_every_frame_before_it(
        payloads in vec(vec(any::<u8>(), 0..64), 0..4),
        announced in (MAX_FRAME_LEN as u64 + 1)..=u64::from(u32::MAX),
        trailing in vec(any::<u8>(), 0..32),
        cuts in vec(any::<prop::sample::Index>(), 0..16),
    ) {
        let mut stream: Vec<u8> = payloads.iter().flat_map(|payload| framed(payload)).collect();
        stream.extend_from_slice(&u32::try_from(announced).unwrap().to_be_bytes());
        stream.extend_from_slice(&trailing);
        let mut decoder = Decoder::new();
        let (frames, error) = decode_chunks(&mut decoder, &split(&stream, &cuts));
        prop_assert_eq!(frames, payloads);
        let too_large = matches!(
            error,
            Some(ProtocolError::FrameTooLarge { len, max: MAX_FRAME_LEN })
                if u64::try_from(len).unwrap() == announced
        );
        prop_assert!(too_large);
        prop_assert!(decoder.finish().is_err());
    }

    #[test]
    fn a_stream_cut_inside_a_frame_is_truncated(
        payload in vec(any::<u8>(), 1..200),
        cut in any::<prop::sample::Index>(),
    ) {
        let stream = framed(&payload);
        let end = 1 + cut.index(stream.len() - 1);
        let mut decoder = Decoder::new();
        prop_assert!(decoder.push(&stream[..end]).unwrap().is_empty());
        let truncated = matches!(
            decoder.finish(),
            Err(ProtocolError::Truncated { buffered }) if buffered == end
        );
        prop_assert!(truncated);
    }
}

#[test]
fn encode_writes_the_big_endian_length_then_the_json() {
    let bytes = encode(&json!({ "a": 1 })).unwrap();
    assert_eq!(bytes, b"\x00\x00\x00\x07{\"a\":1}");
}

#[test]
fn an_empty_payload_is_a_frame() {
    let mut decoder = Decoder::new();
    assert_eq!(decoder.push(&[0, 0, 0, 0]).unwrap(), vec![Vec::<u8>::new()]);
    assert!(decoder.finish().is_ok());
}

#[test]
fn a_prefix_of_exactly_the_limit_is_accepted() {
    let mut decoder = Decoder::new();
    let prefix = u32::try_from(MAX_FRAME_LEN).unwrap().to_be_bytes();
    assert!(decoder.push(&prefix).unwrap().is_empty());
    assert_eq!(decoder.buffered(), HEADER_LEN);
    assert!(matches!(decoder.finish(), Err(ProtocolError::Truncated { buffered: HEADER_LEN })));
}

#[test]
fn a_failed_decoder_keeps_failing() {
    let mut decoder = Decoder::new();
    assert!(decoder.push(&[0xff, 0xff, 0xff, 0xff]).is_err());
    assert!(decoder.push(&[]).is_err());
    assert!(decoder.push(b"{}").is_err());
    assert_eq!(decoder.buffered(), 0);
}

#[test]
fn frames_before_an_oversized_prefix_in_one_chunk_are_not_lost() {
    let mut chunk = framed(b"{}");
    chunk.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
    let mut decoder = Decoder::new();
    assert_eq!(decoder.push(&chunk).unwrap(), vec![b"{}".to_vec()]);
    assert!(matches!(decoder.push(&[]), Err(ProtocolError::FrameTooLarge { .. })));
}

#[test]
fn a_frame_over_the_limit_is_not_encoded() {
    let text = "x".repeat(MAX_FRAME_LEN);
    let err = encode(&text).unwrap_err();
    assert!(matches!(
        err,
        ProtocolError::FrameTooLarge { len, max: MAX_FRAME_LEN } if len == MAX_FRAME_LEN + 2
    ));
}

#[test]
fn a_value_that_is_not_json_is_an_encode_error() {
    let mut map = std::collections::BTreeMap::new();
    map.insert(vec![1_u8], 1);
    assert!(matches!(encode(&map), Err(ProtocolError::Encode { .. })));
}

#[test]
fn typed_frames_survive_encode_and_decode() {
    let request =
        ClientFrame::Request { id: RequestId::new(1), method: Method::AdminStatus(AdminStatus {}) };
    let end = ServerFrame::end(RequestId::new(1));
    let mut stream = encode(&request).unwrap();
    stream.extend(encode(&end).unwrap());

    let mut decoder = Decoder::new();
    let payloads = decoder.push(&stream).unwrap();
    assert_eq!(payloads.len(), 2);
    assert_eq!(ClientFrame::from_json(&payloads[0]).unwrap(), request);
    assert_eq!(ServerFrame::from_json(&payloads[1]).unwrap(), end);
}
