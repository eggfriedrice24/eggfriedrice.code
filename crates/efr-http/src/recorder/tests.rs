use std::sync::{Arc, Mutex};

use bytes::Bytes;
use futures::{StreamExt as _, stream};
use pretty_assertions::assert_eq;

use super::{ExchangeId, Record, Recorder, record_body};
use crate::{ByteStream, HttpError};

/// Keeps a one-line summary of each record.
#[derive(Debug, Default)]
struct Log(Mutex<Vec<String>>);

impl Log {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl Recorder for Log {
    fn record(&self, record: &Record<'_>) {
        let line = match record {
            Record::BodyChunk { exchange, bytes } => {
                format!("{exchange} chunk {}", String::from_utf8_lossy(bytes))
            }
            Record::BodyEnd { exchange, end } => format!("{exchange} end {end:?}"),
            other => format!("{other:?}"),
        };
        self.0.lock().unwrap().push(line);
    }
}

fn body(chunks: Vec<Result<&'static [u8], HttpError>>) -> ByteStream {
    Box::pin(stream::iter(chunks.into_iter().map(|chunk| chunk.map(Bytes::from_static))))
}

#[tokio::test]
async fn chunks_and_a_complete_end_are_recorded() {
    let log = Arc::new(Log::default());
    let recorded = record_body(body(vec![Ok(b"data: a\n"), Ok(b"\n")]), log.clone(), ExchangeId(3));
    let chunks: Vec<_> = recorded.map(Result::unwrap).collect().await;
    assert_eq!(chunks, [Bytes::from_static(b"data: a\n"), Bytes::from_static(b"\n")]);
    assert_eq!(log.lines(), ["3 chunk data: a\n", "3 chunk \n", "3 end Complete"]);
}

#[tokio::test]
async fn a_failed_body_ends_once() {
    let log = Arc::new(Log::default());
    let chunks = vec![Ok(&b"a"[..]), Err(HttpError::SseEventTooLarge { limit: 1 })];
    let mut recorded = record_body(body(chunks), log.clone(), ExchangeId(1));
    while recorded.next().await.is_some() {}
    drop(recorded);
    assert_eq!(log.lines(), ["1 chunk a", "1 end Failed"]);
}

#[tokio::test]
async fn a_dropped_body_records_the_drop() {
    let log = Arc::new(Log::default());
    let mut recorded = record_body(body(vec![Ok(b"a"), Ok(b"b")]), log.clone(), ExchangeId(2));
    let _ = recorded.next().await;
    drop(recorded);
    assert_eq!(log.lines(), ["2 chunk a", "2 end Dropped"]);
}

#[test]
fn exchange_ids_show_their_number() {
    assert_eq!(ExchangeId(42).get(), 42);
    assert_eq!(ExchangeId(42).to_string(), "42");
}
