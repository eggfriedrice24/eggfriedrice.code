//! The log lines of one model call: how it ended (C2.8e) and its usage (DV12).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::Event;
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
    TokenUsage,
};
use serde_json::json;

use super::USAGE_TARGET;
use crate::testing::{Setup, answer, done, expect_request, failure, hold, request};

/// The lines of the log of a test, as plain text.
#[derive(Debug, Clone, Default)]
struct LogText(Arc<Mutex<Vec<u8>>>);

impl LogText {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    /// The lines that contain `needle`.
    fn lines(&self, needle: &str) -> Vec<String> {
        self.text().lines().filter(|line| line.contains(needle)).map(str::to_owned).collect()
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

/// A log at `debug` for the test's thread, and the guard that keeps it.
fn debug_log() -> (LogText, tracing::subscriber::DefaultGuard) {
    let log = LogText::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    // NOTE: the test's runtime has one thread, so the actor's task logs here too.
    (log, tracing::subscriber::set_default(subscriber))
}

/// A model that answers every request with one word and the same usage.
#[derive(Debug)]
struct Counted {
    id: ProviderId,
}

impl Counted {
    fn usage() -> TokenUsage {
        TokenUsage {
            input_tokens: 18_200,
            output_tokens: 1_100,
            cached_input_tokens: 16_600,
            cache_write_tokens: 1_200,
            cache_write_1h_tokens: 1_200,
            ..TokenUsage::default()
        }
    }
}

#[async_trait]
impl Provider for Counted {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let events = vec![
            ProviderEvent::TextDelta { text: "secret answer text".to_owned() },
            ProviderEvent::Usage(Counted::usage()),
            done(StopReason::EndTurn, None),
        ];
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

#[tokio::test]
async fn provider_call_logs_usage_and_gap() {
    let (log, _guard) = debug_log();
    let model = Arc::new(Counted { id: ProviderId::new("replay").unwrap() });
    let mut h = Setup::new().start_model(model).await;

    let first = h.prompt("my secret prompt").await;
    h.wait_end(first.turn_id).await;
    // NOTE: the conversation is idle and waits on no clock, so the move wakes nothing;
    // it only sets the time of the next call.
    h.clock.advance(Duration::from_secs(90));
    let second = h.prompt("my secret prompt").await;
    h.wait_end(second.turn_id).await;

    let lines = log.lines(&format!("{USAGE_TARGET}:"));
    assert_eq!(lines.len(), 2, "one line per model call: {}", log.text());
    assert!(lines.iter().all(|line| line.contains(" DEBUG ")), "{lines:?}");
    let numbers = "input=18200 output=1100 read=16600 write=1200 write_1h=1200";
    assert!(
        lines[0].contains(&format!(": usage gap_ms=none {numbers}")),
        "no call before the first one since the start: {}",
        lines[0]
    );
    assert!(
        lines[1].contains(&format!(": usage gap_ms=90000 {numbers}")),
        "the gap comes from the shared clock: {}",
        lines[1]
    );
    for line in &lines {
        assert!(!line.contains("secret"), "numbers only, never text: {line}");
    }
    let answered = log.lines("phase=model_answer");
    assert_eq!(answered.len(), 2, "{}", log.text());
    assert!(answered.iter().all(|line| line.contains("stop_reason=end_turn")), "{answered:?}");
    assert!(answered.iter().all(|line| !line.contains("outcome=")), "{answered:?}");
}

#[tokio::test]
async fn interrupted_call_logs_outcome_not_stop_reason() {
    let (log, _guard) = debug_log();
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "write a story");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "write a story")])),
        answer(&[ProviderEvent::TextDelta { text: "Once upon a time".to_owned() }]),
        hold(),
        answer(&[
            ProviderEvent::TextDelta { text: " there was more.".to_owned() },
            done(StopReason::EndTurn, None),
        ]),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("write a story").await;
    h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;
    let params = h.interrupt_params(Some(sent.turn_id), Vec::new(), Vec::new());
    h.handle.interrupt(params, efr_protocol::Origin::Shell).await.expect("interrupt accepted");
    let end = h.wait_end(sent.turn_id).await;

    assert!(matches!(end, Event::TurnInterrupted { .. }), "{end:?}");
    let answered = log.lines("phase=model_answer");
    assert_eq!(answered.len(), 1, "{}", log.text());
    assert!(answered[0].contains("outcome=interrupted"), "{}", answered[0]);
    assert!(
        !answered[0].contains("stop_reason"),
        "an interrupt is no stop reason: {}",
        answered[0]
    );
    assert_eq!(log.lines(&format!("{USAGE_TARGET}:")), Vec::<String>::new(), "no usage came");
}

#[tokio::test]
async fn failed_call_logs_outcome_error() {
    let (log, _guard) = debug_log();
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "hello");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "hello")])),
        failure(json!({ "kind": "unauthorized" })),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("hello").await;
    let end = h.wait_end(sent.turn_id).await;

    assert!(matches!(end, Event::TurnFailed { .. }), "{end:?}");
    let answered = log.lines("phase=model_answer");
    assert_eq!(answered.len(), 1, "{}", log.text());
    assert!(answered[0].contains("outcome=error"), "{}", answered[0]);
    assert!(!answered[0].contains("stop_reason"), "an error is no stop reason: {}", answered[0]);
    h.finish();
}
