use std::pin::pin;
use std::task::{Context, Waker};

use bytes::Bytes;
use futures::{StreamExt as _, stream};
use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::JsonLines;
use crate::{ByteStream, HttpError};

const URL: &str = "http://local-tailscaled.sock/localapi/v0/watch-ipn-bus?mask=16386";

fn body(chunks: Vec<&'static [u8]>) -> ByteStream {
    Box::pin(stream::iter(chunks.into_iter().map(|chunk| Ok(Bytes::from_static(chunk)))))
}

fn reader(chunks: Vec<&'static [u8]>, max: usize) -> JsonLines {
    JsonLines::new(body(chunks), URL.to_owned(), max)
}

async fn all_lines(mut lines: JsonLines) -> Result<Vec<Vec<u8>>, HttpError> {
    let mut out = Vec::new();
    while let Some(line) = lines.next_line().await? {
        out.push(line.to_vec());
    }
    Ok(out)
}

#[tokio::test]
async fn lines_end_at_lf_and_drop_a_cr_before_it() {
    let lines = reader(vec![b"one\r\ntwo\n\nthree"], 100);
    let got = all_lines(lines).await.unwrap();
    assert_eq!(got, [b"one".to_vec(), b"two".to_vec(), Vec::new(), b"three".to_vec()]);
}

#[tokio::test]
async fn a_line_split_over_chunks_is_one_line() {
    let lines = reader(vec![b"{\"a\"", b":1", b"}\r", b"\n{\"a\":2}\n"], 100);
    let got = all_lines(lines).await.unwrap();
    assert_eq!(got, [b"{\"a\":1}".to_vec(), b"{\"a\":2}".to_vec()]);
}

#[tokio::test]
async fn next_json_skips_blank_lines_and_parses_each_value() {
    let mut lines = reader(vec![b"{\"a\":1}\n \r\n\n{\"a\":2}\n"], 100);
    let first: serde_json::Value = lines.next_json().await.unwrap().unwrap();
    let second: serde_json::Value = lines.next_json().await.unwrap().unwrap();
    assert_eq!((first["a"].clone(), second["a"].clone()), (1.into(), 2.into()));
    assert!(lines.next_json::<serde_json::Value>().await.unwrap().is_none());
}

#[tokio::test]
async fn a_line_that_is_not_json_is_a_decode_error_with_the_url() {
    let mut lines = reader(vec![b"{not json}\n"], 100);
    match lines.next_json::<serde_json::Value>().await {
        Err(HttpError::DecodeJson { url, .. }) => assert_eq!(url, URL),
        other => panic!("unexpected result {other:?}"),
    }
}

#[tokio::test]
async fn a_line_of_exactly_the_cap_passes_and_one_byte_more_fails() {
    let got = all_lines(reader(vec![b"abcd\r\n", b"ef"], 4)).await.unwrap();
    assert_eq!(got, [b"abcd".to_vec(), b"ef".to_vec()]);

    match all_lines(reader(vec![b"ab", b"cde\n"], 4)).await {
        Err(HttpError::LineTooLarge { url, limit }) => assert_eq!((url.as_str(), limit), (URL, 4)),
        other => panic!("unexpected result {other:?}"),
    }
    // A last line without a line end has the same cap.
    assert!(matches!(
        all_lines(reader(vec![b"abcde"], 4)).await,
        Err(HttpError::LineTooLarge { limit: 4, .. })
    ));
}

#[tokio::test]
async fn a_long_line_fails_before_its_end_arrives() {
    // The body never ends: the cap must fire on the open line alone.
    let chunks = stream::iter([Ok(Bytes::from_static(b"0123456789"))]).chain(stream::pending());
    let mut lines = JsonLines::new(Box::pin(chunks), URL.to_owned(), 8);
    assert!(matches!(lines.next_line().await, Err(HttpError::LineTooLarge { limit: 8, .. })));
}

#[tokio::test]
async fn an_error_of_the_body_is_returned() {
    let chunks: ByteStream = Box::pin(stream::iter([
        Ok(Bytes::from_static(b"ok\n")),
        Err(HttpError::Timeout { url: URL.to_owned() }),
    ]));
    let mut lines = JsonLines::new(chunks, URL.to_owned(), 8);
    assert_eq!(lines.next_line().await.unwrap().unwrap(), Bytes::from_static(b"ok"));
    assert!(matches!(lines.next_line().await, Err(HttpError::Timeout { .. })));
}

#[tokio::test]
async fn a_dropped_call_loses_no_bytes() {
    let (sender, receiver) = futures::channel::mpsc::unbounded::<Result<Bytes, HttpError>>();
    let mut lines = JsonLines::new(Box::pin(receiver), URL.to_owned(), 64);
    sender.unbounded_send(Ok(Bytes::from_static(b"{\"a\":"))).unwrap();
    {
        // Poll once: it takes the chunk and then waits for more.
        let mut call = pin!(lines.next_line());
        let mut context = Context::from_waker(Waker::noop());
        assert!(call.as_mut().poll(&mut context).is_pending());
    }
    sender.unbounded_send(Ok(Bytes::from_static(b"1}\n"))).unwrap();
    drop(sender);
    assert_eq!(lines.next_line().await.unwrap().unwrap(), Bytes::from_static(b"{\"a\":1}"));
    assert!(lines.next_line().await.unwrap().is_none());
}

#[test]
fn debug_shows_no_body() {
    let lines = reader(vec![b"secret\n"], 8);
    let text = format!("{lines:?}");
    assert!(text.contains("max_line_bytes: 8"), "{text}");
    assert!(!text.contains("secret"), "{text}");
}

proptest! {
    #[test]
    fn any_split_of_the_body_gives_the_same_lines(
        lines in proptest::collection::vec("[a-z{}:\"\r ]{0,12}", 0..8),
        cuts in proptest::collection::vec(0usize..200, 0..10),
    ) {
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let bytes = text.into_bytes();
        let mut points: Vec<usize> = cuts.into_iter().map(|cut| cut % (bytes.len() + 1)).collect();
        points.push(0);
        points.push(bytes.len());
        points.sort_unstable();
        points.dedup();
        let chunks: Vec<Result<Bytes, HttpError>> = points
            .windows(2)
            .map(|pair| Ok(Bytes::copy_from_slice(&bytes[pair[0]..pair[1]])))
            .collect();
        let whole = JsonLines::new(
            Box::pin(stream::iter([Ok(Bytes::from(bytes.clone()))])),
            URL.to_owned(),
            64,
        );
        let split = JsonLines::new(Box::pin(stream::iter(chunks)), URL.to_owned(), 64);
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (whole, split) = runtime.block_on(async {
            (all_lines(whole).await.unwrap(), all_lines(split).await.unwrap())
        });
        let expected: Vec<Vec<u8>> = lines
            .iter()
            .map(|line| line.strip_suffix('\r').unwrap_or(line).as_bytes().to_vec())
            .collect();
        prop_assert_eq!(&split, &whole);
        prop_assert_eq!(split, expected);
    }
}
