use std::time::Duration;

use bytes::Bytes;
use futures::{StreamExt as _, stream};
use pretty_assertions::assert_eq;
use proptest::prelude::{Just, Strategy, prop, proptest};

use super::{SseDecoder, SseEvent, SseStream};
use crate::HttpError;

fn event(event: &str, data: &str, id: Option<&str>) -> SseEvent {
    SseEvent { event: event.to_owned(), data: data.to_owned(), id: id.map(str::to_owned) }
}

fn message(data: &str) -> SseEvent {
    event("message", data, None)
}

/// Feeds `chunks` one after another to a fresh decoder.
fn decode_chunks(chunks: &[&[u8]]) -> Vec<SseEvent> {
    let mut decoder = SseDecoder::new();
    let mut events = Vec::new();
    for chunk in chunks {
        events.extend(decoder.push(chunk).unwrap());
    }
    events
}

fn decode(input: &str) -> Vec<SseEvent> {
    decode_chunks(&[input.as_bytes()])
}

#[test]
fn single_event() {
    assert_eq!(decode("data: hello\n\n"), [message("hello")]);
}

#[test]
fn multi_line_data_joins_with_newlines() {
    assert_eq!(
        decode("data: first\ndata: second\ndata:\ndata: fourth\n\n"),
        [message("first\nsecond\n\nfourth")]
    );
}

#[test]
fn event_type_applies_to_one_event_only() {
    let events =
        decode("event: response.output_text.delta\ndata: {\"delta\":\"hi\"}\n\ndata: plain\n\n");
    assert_eq!(
        events,
        [event("response.output_text.delta", "{\"delta\":\"hi\"}", None), message("plain")]
    );
}

#[test]
fn comments_are_dropped() {
    assert_eq!(decode(": keep-alive\n\n:\ndata: x\n: inside an event\n\n"), [message("x")]);
}

#[test]
fn event_id_carries_over_until_it_changes() {
    let mut decoder = SseDecoder::new();
    let events =
        decoder.push(b"id: 1\ndata: a\n\ndata: b\n\nid: 2\ndata: c\n\nid\ndata: d\n\n").unwrap();
    assert_eq!(
        events,
        [
            event("message", "a", Some("1")),
            event("message", "b", Some("1")),
            event("message", "c", Some("2")),
            event("message", "d", None),
        ]
    );
    assert_eq!(decoder.last_event_id(), None);
}

#[test]
fn an_id_without_data_still_sets_the_last_event_id() {
    let mut decoder = SseDecoder::new();
    assert_eq!(decoder.push(b"id: 7\n\n").unwrap(), []);
    assert_eq!(decoder.last_event_id(), Some("7"));
}

#[test]
fn an_id_holding_nul_is_ignored() {
    let mut decoder = SseDecoder::new();
    let events = decoder.push(b"id: 1\n\nid: 2\0x\ndata: a\n\n").unwrap();
    assert_eq!(events, [event("message", "a", Some("1"))]);
}

#[test]
fn crlf_cr_and_lf_all_end_lines() {
    let expected = [message("a"), message("b"), message("c")];
    assert_eq!(decode("data: a\r\n\r\ndata: b\r\rdata: c\n\n"), expected);
}

#[test]
fn crlf_split_across_chunks_is_one_terminator() {
    // Were the LF a second terminator, it would end the event after `data: a`.
    let events = decode_chunks(&[b"data: a\r", b"\ndata: b\r", b"\n\r", b"\n"]);
    assert_eq!(events, [message("a\nb")]);
}

#[test]
fn a_lone_cr_at_a_chunk_end_does_not_eat_the_next_line() {
    let events = decode_chunks(&[b"data: a\r", b"data: b\r\r"]);
    assert_eq!(events, [message("a\nb")]);
}

#[test]
fn an_empty_chunk_keeps_a_pending_cr() {
    let events = decode_chunks(&[b"data: a\r", b"", b"\ndata: b\n\n"]);
    assert_eq!(events, [message("a\nb")]);
}

#[test]
fn split_inside_a_line_and_a_utf8_sequence() {
    let input = "data: \u{e9}t\u{e9} \u{1f600}\n\n".as_bytes();
    let (head, tail) = input.split_at(10);
    assert_eq!(decode_chunks(&[head, tail]), [message("\u{e9}t\u{e9} \u{1f600}")]);
}

#[test]
fn byte_order_mark_is_stripped_once_even_when_split() {
    let events = decode_chunks(&[b"\xEF", b"\xBB", b"\xBFdata: a\n\n\xEF\xBB\xBFdata: b\n\n"]);
    // Only the leading mark is special; a later one makes an unknown field name.
    assert_eq!(events, [message("a")]);
}

#[test]
fn field_parsing_rules() {
    // No space after the colon keeps the value whole; only one space is removed; a
    // line without a colon is a field with an empty value; unknown fields are ignored.
    let events = decode("data:tight\ndata:  two spaces\ndata\nfoo: bar\n\n");
    assert_eq!(events, [message("tight\n two spaces\n")]);
}

#[test]
fn an_event_without_data_is_not_dispatched() {
    assert_eq!(decode("event: ping\n\ndata: x\n\n"), [message("x")]);
}

#[test]
fn an_empty_data_field_dispatches_an_empty_event() {
    assert_eq!(decode("data\n\n"), [message("")]);
}

#[test]
fn an_unterminated_event_is_discarded() {
    assert_eq!(decode("data: complete\n\ndata: cut off"), [message("complete")]);
    assert_eq!(decode("data: no blank line\n"), []);
}

#[test]
fn retry_takes_only_digits() {
    let mut decoder = SseDecoder::new();
    decoder.push(b"retry: 2500\n\n").unwrap();
    assert_eq!(decoder.retry(), Some(Duration::from_millis(2500)));
    decoder.push(b"retry: 1s\nretry: -1\nretry:\n\n").unwrap();
    assert_eq!(decoder.retry(), Some(Duration::from_millis(2500)));
    decoder.push(b"retry: 99999999999999999999999\n\n").unwrap();
    assert_eq!(decoder.retry(), Some(Duration::from_millis(2500)), "overflow is ignored");
}

#[test]
fn a_line_over_the_limit_is_an_error() {
    let mut decoder = SseDecoder::with_max_event_bytes(16);
    assert!(decoder.push(b"data: 0123456789").is_ok());
    let result = decoder.push(b"abcdef");
    assert!(matches!(result, Err(HttpError::SseEventTooLarge { limit: 16 })), "{result:?}");
}

#[test]
fn data_over_the_limit_is_an_error() {
    let mut decoder = SseDecoder::with_max_event_bytes(16);
    let result = decoder.push(b"data: 01234567\ndata: 01234567\n");
    assert!(matches!(result, Err(HttpError::SseEventTooLarge { .. })), "{result:?}");
}

#[test]
fn invalid_utf8_becomes_replacement_characters() {
    assert_eq!(decode_chunks(&[b"data: a\xFFb\n\n"]), [message("a\u{fffd}b")]);
}

fn byte_stream(
    chunks: Vec<Result<&'static [u8], HttpError>>,
) -> impl futures::Stream<Item = Result<Bytes, HttpError>> + Unpin {
    stream::iter(chunks.into_iter().map(|chunk| chunk.map(Bytes::from_static)))
}

#[tokio::test]
async fn stream_yields_events_across_chunks() {
    let inner = byte_stream(vec![Ok(b"data: a\n"), Ok(b"\nevent: e\ndata"), Ok(b": b\n\n")]);
    let events: Vec<_> = SseStream::new(inner).map(Result::unwrap).collect().await;
    assert_eq!(events, [message("a"), event("e", "b", None)]);
}

#[tokio::test]
async fn stream_ends_after_an_inner_error() {
    let inner = byte_stream(vec![
        Ok(b"data: a\n\n"),
        Err(HttpError::SseEventTooLarge { limit: 1 }),
        Ok(b"data: never\n\n"),
    ]);
    let mut sse = SseStream::new(inner);
    assert_eq!(sse.next().await.unwrap().unwrap(), message("a"));
    assert!(sse.next().await.unwrap().is_err());
    assert!(sse.next().await.is_none());
}

#[tokio::test]
async fn stream_reports_the_decoder_state() {
    let inner = byte_stream(vec![Ok(b"id: 9\nretry: 10\ndata: a\n\n")]);
    let mut sse = SseStream::new(inner);
    assert_eq!(sse.next().await.unwrap().unwrap(), event("message", "a", Some("9")));
    assert_eq!(sse.decoder().last_event_id(), Some("9"));
    assert_eq!(sse.decoder().retry(), Some(Duration::from_millis(10)));
}

/// A stream built from lines with every kind of terminator, comments, ids and events.
fn sse_text() -> impl Strategy<Value = Vec<u8>> {
    let field = prop::sample::select(vec!["data", "event", "id", "retry", ":", "x"]);
    let value = "[a-z0-9 {}\":é]{0,12}";
    let terminator = prop::sample::select(vec!["\n", "\r\n", "\r"]);
    let line = (field, value, terminator.clone()).prop_map(|(field, value, end)| {
        if field == ":" { format!(":{value}{end}") } else { format!("{field}: {value}{end}") }
    });
    let blank = terminator.prop_map(str::to_owned);
    let item = prop::strategy::Union::new_weighted(vec![(4, line.boxed()), (1, blank.boxed())]);
    (prop::collection::vec(item, 0..40), prop::bool::ANY).prop_map(|(items, bom)| {
        let mut text = if bom { "\u{feff}".to_owned() } else { String::new() };
        text.extend(items);
        text.into_bytes()
    })
}

proptest! {
    /// Any split of a stream into chunks yields the events of the unsplit stream: the
    /// property that lets the decoder sit on a network stream.
    #[test]
    fn any_split_yields_the_same_events(
        (text, mut cuts) in sse_text().prop_flat_map(|text| {
            let len = text.len();
            (Just(text), prop::collection::vec(0..=len, 0..8))
        }),
    ) {
        let whole = SseDecoder::new().push(&text).unwrap();
        cuts.sort_unstable();
        let mut decoder = SseDecoder::new();
        let mut split = Vec::new();
        let mut start = 0;
        for cut in cuts.into_iter().chain([text.len()]) {
            split.extend(decoder.push(&text[start..cut]).unwrap());
            start = cut;
        }
        assert_eq!(split, whole);
    }
}
