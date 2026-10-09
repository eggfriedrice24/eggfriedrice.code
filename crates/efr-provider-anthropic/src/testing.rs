//! Fakes shared by the unit tests of this crate: a clock whose sleeps end at once, a
//! random source, a token source of fixed keys, the entries of a model list, a stream
//! of server-sent events, the fixture loader of `fixtures/` and a log that a test
//! reads. Fake `POST /messages` and `GET /models` answers come from `wiremock`. No test
//! calls the real API.
//!
//! The clock stays here instead of coming from `efr-test-support`, as in
//! `efr-provider-openai`: its `TestClock` can stand in only for a clock that nobody
//! moves, not for one whose sleeps end at once, and `efr-test-support` brings
//! `efr-store` with its bundled SQLite build.

use std::future::ready;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use efr_http::{SseDecoder, SseEvent};
use efr_provider::{
    AccessToken, Completion, CompletionBuilder, ProviderError, SecretString, TokenSource,
};
use efr_stdx::rng::Rng;
use efr_stdx::time::{Clock, Sleep};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use crate::sse_events::EventMapper;

/// The key of the tests. It looks like a real key, so a test that finds it in a log or
/// an error finds a leak.
pub(crate) const KEY: &str = "sk-ant-api03-test-key-0123456789";

/// The instant the fake clock starts at: 2026-10-04T12:00:00Z.
pub(crate) fn start() -> Timestamp {
    Timestamp::from_second(1_791_115_200).unwrap()
}

/// A clock whose sleeps finish at once and move `now` forward by their duration.
#[derive(Debug)]
pub(crate) struct InstantClock {
    now: Mutex<Timestamp>,
    sleeps: Mutex<Vec<Duration>>,
}

impl InstantClock {
    pub(crate) fn new() -> Self {
        InstantClock { now: Mutex::new(start()), sleeps: Mutex::new(Vec::new()) }
    }

    /// Every sleep so far, in order.
    pub(crate) fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().unwrap().clone()
    }
}

impl Clock for InstantClock {
    fn now(&self) -> Timestamp {
        *self.now.lock().unwrap()
    }

    fn sleep(&self, duration: Duration) -> Sleep {
        self.sleeps.lock().unwrap().push(duration);
        let mut now = self.now.lock().unwrap();
        *now = now.checked_add(SignedDuration::try_from(duration).unwrap()).unwrap();
        Box::pin(ready(()))
    }
}

/// A generator that always draws the same number.
#[derive(Debug)]
pub(crate) struct FixedRng(pub(crate) u64);

impl Rng for FixedRng {
    fn fill_bytes(&self, dest: &mut [u8]) {
        for (index, byte) in dest.iter_mut().enumerate() {
            *byte = self.0.to_le_bytes()[index % 8];
        }
    }

    fn next_u64(&self) -> u64 {
        self.0
    }
}

/// A token source of one key that counts the calls. It cannot refresh, as an API key
/// cannot, unless a test asks for [`FakeTokens::refreshing`].
#[derive(Debug)]
pub(crate) struct FakeTokens {
    keys: Vec<&'static str>,
    refreshable: bool,
    invalidations: AtomicUsize,
}

impl FakeTokens {
    /// A source of [`KEY`].
    pub(crate) fn key() -> Self {
        FakeTokens { keys: vec![KEY], refreshable: false, invalidations: AtomicUsize::new(0) }
    }

    /// A source of no key, as before a login.
    pub(crate) fn none() -> Self {
        FakeTokens { keys: Vec::new(), ..FakeTokens::key() }
    }

    /// A source that can refresh: it hands out `keys` in order, moving to the next one
    /// on each `invalidate`.
    pub(crate) fn refreshing(keys: &[&'static str]) -> Self {
        FakeTokens { keys: keys.to_vec(), refreshable: true, invalidations: AtomicUsize::new(0) }
    }

    pub(crate) fn invalidations(&self) -> usize {
        self.invalidations.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl TokenSource for FakeTokens {
    async fn access_token(&self) -> Result<AccessToken, ProviderError> {
        let index = self.invalidations().min(self.keys.len().saturating_sub(1));
        let Some(key) = self.keys.get(index) else {
            return Err(ProviderError::NotLoggedIn);
        };
        Ok(AccessToken::new(SecretString::from(*key)))
    }

    async fn invalidate(&self) {
        self.invalidations.fetch_add(1, Ordering::SeqCst);
    }

    fn refreshable(&self) -> bool {
        self.refreshable
    }
}

/// A base URL on a local port where nothing listens, so a connection is refused.
/// A dropped `wiremock` server goes back to its pool and still answers, so it cannot
/// stand in.
pub(crate) fn closed_base_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{address}/v1")
}

/// An entry of `GET /models` in the API's form: `lifecycle` `active`, a window of
/// 1M tokens, 128K output tokens and the efforts `low` to `max`.
pub(crate) fn model_entry(id: &str) -> Value {
    let supported = json!({"supported": true});
    json!({
        "type": "model",
        "id": id,
        "display_name": id,
        "created_at": "2026-09-01T00:00:00Z",
        "max_input_tokens": 1_000_000,
        "max_tokens": 128_000,
        "lifecycle": "active",
        "capabilities": {
            "batch": supported,
            "effort": {
                "supported": true,
                "low": supported,
                "medium": supported,
                "high": supported,
                "xhigh": supported,
                "max": supported,
            },
            "thinking": {"supported": true, "types": {"adaptive": supported}},
        },
    })
}

/// One page of `GET /models`.
pub(crate) fn models_page(entries: &[Value], has_more: bool) -> Value {
    let id = |entry: Option<&Value>| entry.map_or(Value::Null, |entry| entry["id"].clone());
    json!({
        "data": entries,
        "has_more": has_more,
        "first_id": id(entries.first()),
        "last_id": id(entries.last()),
    })
}

/// The server-sent events of `events`, each `(type, data)`, with the type in the data
/// as the API sends it.
pub(crate) fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(kind, data)| {
            let mut data = data.clone();
            data["type"] = json!(kind);
            format!("event: {kind}\ndata: {data}\n\n")
        })
        .collect()
}

/// A whole answer that streams `text` and ends the turn.
pub(crate) fn text_answer(text: &str) -> String {
    sse(&[
        (
            "message_start",
            json!({"message": {
                "id": "msg_01", "type": "message", "role": "assistant", "content": [],
                "model": "claude-opus-5-5", "stop_reason": null,
                "usage": {"input_tokens": 12, "cache_read_input_tokens": 4000, "output_tokens": 1},
            }}),
        ),
        ("ping", json!({})),
        ("content_block_start", json!({"index": 0, "content_block": {"type": "text", "text": ""}})),
        ("content_block_delta", json!({"index": 0, "delta": {"type": "text_delta", "text": text}})),
        ("content_block_stop", json!({"index": 0})),
        (
            "message_delta",
            json!({"delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 9}}),
        ),
        ("message_stop", json!({})),
    ])
}

/// A log that a test reads: the `tracing` lines written while it is the default.
#[derive(Debug, Clone, Default)]
pub(crate) struct LogText(Arc<Mutex<Vec<u8>>>);

impl LogText {
    /// A subscriber that writes every line at every level here.
    pub(crate) fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        tracing_subscriber::fmt()
            .with_writer(self.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish()
    }

    /// Everything written so far.
    pub(crate) fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl std::io::Write for LogText {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogText {
    type Writer = LogText;

    fn make_writer(&'a self) -> LogText {
        self.clone()
    }
}

/// The text of the stream fixture `name` under `fixtures/messages/`.
pub(crate) fn messages_fixture(name: &str) -> String {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures").join("messages").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The server-sent events of the stream fixture `name`.
pub(crate) fn fixture_events(name: &str) -> Vec<SseEvent> {
    SseDecoder::new().push(messages_fixture(name).as_bytes()).unwrap()
}

/// The answer of the stream fixture `name`, as the conversation folds it.
pub(crate) fn fixture_completion(name: &str) -> Completion {
    let mut mapper = EventMapper::new();
    let mut builder = CompletionBuilder::new();
    for event in fixture_events(name) {
        for mapped in mapper.map(&event).unwrap() {
            builder.push(&mapped).unwrap();
        }
    }
    builder.finish().unwrap()
}
