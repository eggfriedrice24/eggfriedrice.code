use std::sync::Arc;
use std::time::Duration;

use efr_provider::{
    ContentBlock, Provider, ProviderError, ProviderEvent, ProviderId, Request, StopReason,
};
use futures::{FutureExt as _, StreamExt as _};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::{ReplayProvider, first_difference};
use crate::{Inbound, Outbound, Record, Redactor, TestSupportError, Transcript, fixtures};

const CWD: &str = "/tmp/efr-test-x/home/project";

fn request(text: &str) -> Request {
    let mut request = Request::new("gpt-5-codex");
    request.messages.push(efr_provider::Message::user(text));
    request
}

fn request_json(text: &str) -> Value {
    serde_json::to_value(request(text)).unwrap()
}

/// SSE text with one `message` event per canonical event.
fn sse(events: &[ProviderEvent]) -> String {
    events
        .iter()
        .map(|event| format!("data: {}\n\n", serde_json::to_string(event).unwrap()))
        .collect()
}

fn text(text: &str) -> ProviderEvent {
    ProviderEvent::TextDelta { text: text.to_owned() }
}

fn done() -> ProviderEvent {
    ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None }
}

fn ask(body: Value) -> Record {
    Record::ExpectOutbound(Outbound::ProviderRequest(body))
}

fn answer(body: impl Into<String>) -> Record {
    Record::EmitInbound(Inbound::ProviderSse(body.into()))
}

fn frame() -> Record {
    Record::EmitInbound(Inbound::ClientFrame(json!({"cancel": 1})))
}

/// One exchange per prompt, each answered with the prompt's text and `done`.
fn echo(prompts: &[&str]) -> ReplayProvider {
    let records = prompts
        .iter()
        .flat_map(|prompt| [ask(request_json(prompt)), answer(sse(&[text(prompt), done()]))]);
    ReplayProvider::new(&Transcript::from_records(records)).unwrap()
}

async fn events(provider: &ReplayProvider, request: Request) -> Vec<ProviderEvent> {
    provider.stream(request).await.unwrap().map(|event| event.unwrap()).collect().await
}

fn api_code(error: &ProviderError) -> Option<&str> {
    match error {
        ProviderError::Api { code, .. } => code.as_deref(),
        _ => None,
    }
}

#[tokio::test]
async fn the_fixture_replays_with_redaction_and_restored_placeholders() {
    let path = fixtures::path(file!(), "transcripts/single_exchange.ndjson").unwrap();
    let provider = ReplayProvider::new(&Transcript::read(path).unwrap())
        .unwrap()
        .with_redactor(Redactor::new().cwd(CWD).hostname("box"));
    let provider: Arc<dyn Provider> = Arc::new(provider);
    let mut request = request("list the files");
    request.system = Some(format!("You run in {CWD} on box."));

    let completion = provider.complete(request).await.unwrap();

    assert_eq!(completion.stop_reason, StopReason::EndTurn);
    assert_eq!(completion.message.text(), format!("The files in {CWD} are: none."));
}

#[tokio::test]
async fn exchanges_are_served_in_order_and_finish_is_clean() {
    let provider = echo(&["one", "two"]);
    assert_eq!(provider.remaining(), 2);
    assert_eq!(events(&provider, request("one")).await, [text("one"), done()]);
    assert_eq!(events(&provider, request("two")).await, [text("two"), done()]);
    assert_eq!(provider.served(), 2);
    assert_eq!(provider.remaining(), 0);
    provider.finish().unwrap();
}

#[tokio::test]
async fn a_request_that_differs_fails_and_finish_names_the_line_and_place() {
    let provider = echo(&["list the files"]);
    let error = provider.stream(request("delete the files")).await.err().unwrap();
    assert_eq!(api_code(&error), Some(ReplayProvider::MISMATCH_CODE));

    let failure = provider.finish().unwrap_err();
    assert_eq!(
        failure.to_string(),
        "the request does not match the provider_request at line 1; the first difference is at \
         \"/messages/0/content/0/text\""
    );
    match failure {
        TestSupportError::RequestMismatch { line, expected, actual, .. } => {
            assert_eq!(line, 1);
            assert_eq!(expected, request_json("list the files"));
            assert_eq!(actual, request_json("delete the files"));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn after_a_failure_every_request_fails_the_same_way() {
    let provider = echo(&["one", "two"]);
    assert!(provider.stream(request("wrong")).await.is_err());
    let error = provider.stream(request("two")).await.err().unwrap();
    assert_eq!(api_code(&error), Some(ReplayProvider::MISMATCH_CODE));
    assert_eq!(provider.served(), 0);
}

#[tokio::test]
async fn a_request_after_the_last_exchange_fails() {
    let provider = echo(&["one"]);
    events(&provider, request("one")).await;
    let error = provider.stream(request("again")).await.err().unwrap();
    assert_eq!(api_code(&error), Some(ReplayProvider::EXHAUSTED_CODE));
    let failure = provider.finish().unwrap_err();
    assert!(
        matches!(&failure, TestSupportError::UnexpectedRequest { request }
            if *request == request_json("again")),
        "{failure:?}"
    );
}

#[tokio::test]
async fn exchanges_that_were_never_requested_fail_finish() {
    let provider = echo(&["one", "two", "three"]);
    events(&provider, request("one")).await;
    let failure = provider.finish().unwrap_err();
    assert_eq!(
        failure.to_string(),
        "the replay provider served 1 requests, but its transcript has 2 more, the next at line 3"
    );
}

#[tokio::test]
async fn timestamps_and_registered_values_do_not_count_as_differences() {
    let mut expected = request("what changed since <TIMESTAMP>?");
    expected.system = Some("cwd: <CWD>".to_owned());
    let transcript = Transcript::from_records([
        ask(serde_json::to_value(&expected).unwrap()),
        answer(sse(&[done()])),
    ]);
    let provider =
        ReplayProvider::new(&transcript).unwrap().with_redactor(Redactor::new().cwd(CWD));
    let mut actual = request("what changed since 2026-10-04T11:58:00Z?");
    actual.system = Some(format!("cwd: {CWD}"));
    assert_eq!(events(&provider, actual).await, [done()]);
    provider.finish().unwrap();
}

#[tokio::test]
async fn records_of_other_kinds_are_skipped() {
    let transcript = Transcript::from_records([
        frame(),
        ask(request_json("one")),
        Record::ClockAdvance(Duration::from_secs(1)),
        answer(sse(&[text("a")])),
        Record::ExpectOutbound(Outbound::Event(json!({"kind": "turn_started"}))),
        answer(sse(&[text("b"), done()])),
    ]);
    let provider = ReplayProvider::new(&transcript).unwrap();
    assert_eq!(events(&provider, request("one")).await, [text("a"), text("b"), done()]);
}

/// Whether an error is the one a case expects.
type Check = fn(&ProviderError) -> bool;

#[tokio::test]
async fn error_events_end_the_answer_with_a_provider_error() {
    let cases: [(&str, Check); 6] = [
        (r#"{"kind":"api","code":"context_length_exceeded","message":"too long"}"#, |e| {
            matches!(e, ProviderError::ContextOverflow { status: None, code: Some(code), .. }
                if code == "context_length_exceeded")
        }),
        (r#"{"kind":"unauthorized"}"#, |e| matches!(e, ProviderError::Unauthorized)),
        (r#"{"kind":"not_logged_in"}"#, |e| matches!(e, ProviderError::NotLoggedIn)),
        (r#"{"kind":"incomplete"}"#, |e| matches!(e, ProviderError::Incomplete)),
        (r#"{"kind":"rate_limited","retry_after_ms":1500}"#, |e| {
            matches!(e, ProviderError::RateLimited { retry_after: Some(d) }
                if *d == Duration::from_millis(1500))
        }),
        (r#"{"kind":"api","status":500,"code":"server_error","message":"down in <CWD>"}"#, |e| {
            matches!(e, ProviderError::Api { status: Some(500), code: Some(code), message }
                if code == "server_error" && *message == format!("down in {CWD}"))
        }),
    ];
    for (data, expected) in cases {
        let body = format!("{}event: error\ndata: {data}\n\n", sse(&[text("partial")]));
        let transcript = Transcript::from_records([ask(request_json("one")), answer(body)]);
        let provider =
            ReplayProvider::new(&transcript).unwrap().with_redactor(Redactor::new().cwd(CWD));
        let items: Vec<_> = provider.stream(request("one")).await.unwrap().collect().await;
        assert_eq!(items.len(), 2, "{data}");
        assert_eq!(items[0].as_ref().unwrap(), &text("partial"));
        let error = items[1].as_ref().unwrap_err();
        assert!(expected(error), "{data}: {error:?}");
    }
}

#[tokio::test]
async fn a_dropped_answer_leaves_the_next_exchange_in_place() {
    let provider = echo(&["one", "two"]);
    let mut stream = provider.stream(request("one")).await.unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap(), text("one"));
    drop(stream);
    assert_eq!(events(&provider, request("two")).await, [text("two"), done()]);
    provider.finish().unwrap();
}

#[tokio::test]
async fn a_paced_answer_waits_for_the_records_before_each_piece() {
    // Line 3 is the client frame between the two pieces of the answer.
    let transcript = Transcript::from_records([
        ask(request_json("one")),
        answer(sse(&[text("a")])),
        frame(),
        Record::ClockAdvance(Duration::from_secs(1)),
        answer(sse(&[text("b"), done()])),
    ]);
    let provider = ReplayProvider::new(&transcript).unwrap().paced();
    let mut stream = provider.stream(request("one")).await.unwrap();

    assert_eq!(stream.next().await.unwrap().unwrap(), text("a"));
    assert!(stream.next().now_or_never().is_none());
    provider.handled_through(3);
    assert!(stream.next().now_or_never().is_none());
    provider.handled_through(4);
    assert_eq!(stream.next().await.unwrap().unwrap(), text("b"));
    assert_eq!(stream.next().await.unwrap().unwrap(), done());
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn progress_reported_before_the_request_is_kept() {
    let transcript = Transcript::from_records([
        ask(request_json("one")),
        frame(),
        answer(sse(&[text("a"), done()])),
    ]);
    let provider = ReplayProvider::new(&transcript).unwrap().paced();
    provider.handled_through(2);
    provider.handled_through(1);
    assert_eq!(events(&provider, request("one")).await, [text("a"), done()]);
}

#[tokio::test]
async fn a_provider_that_is_not_paced_holds_nothing() {
    let transcript = Transcript::from_records([
        ask(request_json("one")),
        answer(sse(&[text("a")])),
        frame(),
        answer(sse(&[done()])),
    ]);
    let provider = ReplayProvider::new(&transcript).unwrap();
    provider.handled_through(99);
    let mut stream = provider.stream(request("one")).await.unwrap();
    assert_eq!(stream.next().now_or_never().unwrap().unwrap().unwrap(), text("a"));
    assert_eq!(stream.next().now_or_never().unwrap().unwrap().unwrap(), done());
}

#[test]
fn invalid_provider_records_fail_when_the_provider_is_built() {
    let cases: Vec<(Vec<Record>, usize, &str)> = vec![
        (vec![frame(), answer(sse(&[done()]))], 2, "must follow a provider_request record"),
        (vec![ask(request_json("one")), answer("data: {}\n")], 2, "ends inside an event"),
        (
            vec![ask(request_json("one")), answer("event: ping\ndata: {}\n\n")],
            2,
            "the type message or error",
        ),
        (
            vec![
                ask(request_json("one")),
                answer("event: error\ndata: {\"kind\":\"incomplete\"}\n\n"),
                answer(sse(&[done()])),
            ],
            3,
            "events after its error event",
        ),
        (
            vec![
                ask(request_json("one")),
                answer(format!(
                    "event: error\ndata: {{\"kind\":\"incomplete\"}}\n\n{}",
                    sse(&[done()])
                )),
            ],
            2,
            "events after its error event",
        ),
    ];
    for (records, expected_line, expected_problem) in cases {
        let err = ReplayProvider::new(&Transcript::from_records(records)).err().unwrap();
        match err {
            TestSupportError::InvalidRecord { line, problem } => {
                assert_eq!(line, expected_line, "{problem}");
                assert!(problem.contains(expected_problem), "{problem}");
            }
            other => panic!("{expected_problem}: {other:?}"),
        }
    }
}

#[test]
fn provider_records_that_are_not_canonical_fail_when_the_provider_is_built() {
    let cases: Vec<(Vec<Record>, usize)> = vec![
        (vec![ask(json!({"model": "m", "messages": "not a list"}))], 1),
        (vec![ask(request_json("one")), answer("data: not json\n\n")], 2),
        (vec![ask(request_json("one")), answer("data: {\"kind\":\"text_deltaa\"}\n\n")], 2),
        (
            vec![ask(request_json("one")), answer("event: error\ndata: {\"kind\":\"teapot\"}\n\n")],
            2,
        ),
        (
            vec![
                ask(request_json("one")),
                answer("event: error\ndata: {\"kind\":\"unauthorized\",\"extra\":1}\n\n"),
            ],
            2,
        ),
    ];
    for (records, expected_line) in cases {
        let err = ReplayProvider::new(&Transcript::from_records(records)).err().unwrap();
        assert!(
            matches!(err, TestSupportError::ProviderRecord { line, .. } if line == expected_line),
            "{err:?}"
        );
    }
}

#[test]
fn an_empty_transcript_expects_no_requests() {
    let provider = ReplayProvider::new(&Transcript::default()).unwrap();
    assert_eq!(provider.remaining(), 0);
    provider.finish().unwrap();
}

#[test]
fn the_default_id_is_replay_and_can_be_changed() {
    let provider = ReplayProvider::new(&Transcript::default()).unwrap();
    assert_eq!(provider.id().as_str(), "replay");
    let other = provider.with_id(ProviderId::new("openai-subscription").unwrap());
    assert_eq!(other.id().as_str(), "openai-subscription");
    assert_eq!(other.models(), []);
}

#[test]
fn debug_shows_progress_not_the_transcript() {
    let provider = echo(&["one"]).paced();
    assert_eq!(
        format!("{provider:?}"),
        "ReplayProvider { id: ProviderId(\"replay\"), served: 0, remaining: 1, failed: false, \
         paced: true }"
    );
}

#[test]
fn first_difference_points_at_the_first_differing_member() {
    let cases = [
        (json!({"a": 1}), json!({"a": 1}), ""),
        (json!({"a": 1}), json!({"a": 2}), "/a"),
        (json!({"a": 1, "b": [1, 2]}), json!({"a": 1, "b": [1, 3]}), "/b/1"),
        (json!({"a": [1]}), json!({"a": [1, 2]}), "/a/1"),
        (json!({"a": 1}), json!({"a": 1, "z": 2}), "/z"),
        (json!({"a/b": {"c~d": 1}}), json!({"a/b": {"c~d": 2}}), "/a~1b/c~0d"),
        (json!({"a": 1}), json!([1]), ""),
    ];
    for (expected, actual, pointer) in cases {
        assert_eq!(first_difference(&expected, &actual), pointer, "{expected} {actual}");
    }
}

#[tokio::test]
async fn a_tool_call_answer_collects_into_a_message() {
    let call = [
        ProviderEvent::ToolCallStart {
            call_id: "call_1".to_owned(),
            name: "shell".to_owned(),
            freeform: false,
        },
        ProviderEvent::ToolCallEnd {
            call_id: "call_1".to_owned(),
            arguments: "{\"command\":\"ls <CWD>\"}".to_owned(),
        },
        ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: Some(json!([1])) },
    ];
    let transcript = Transcript::from_records([ask(request_json("ls")), answer(sse(&call))]);
    let provider =
        ReplayProvider::new(&transcript).unwrap().with_redactor(Redactor::new().cwd(CWD));
    let completion = provider.complete(request("ls")).await.unwrap();
    assert_eq!(completion.stop_reason, StopReason::ToolUse);
    assert_eq!(
        completion.message.content,
        [ContentBlock::ToolCall {
            call_id: "call_1".to_owned(),
            name: "shell".to_owned(),
            input: json!({"command": format!("ls {CWD}")}),
            freeform: false,
        }]
    );
    assert_eq!(completion.message.provider_raw, Some(json!([1])));
}
