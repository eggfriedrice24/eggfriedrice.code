//! Fakes shared by the unit tests of this crate: a clock whose sleeps end at once, a
//! token source of fixed keys, the fixture loader of `fixtures/`, and fake
//! `POST /messages` and `GET /models` answers on `wiremock`. No test calls the real API.

use std::path::PathBuf;

use efr_http::{SseDecoder, SseEvent};
use efr_provider::{Completion, CompletionBuilder};

use crate::sse_events::EventMapper;

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
