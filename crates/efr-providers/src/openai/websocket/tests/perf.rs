//! The bench of the Responses request body on a long history (the plan's W2-F): the
//! HTTP body, and the `response.create` message of the WebSocket with the parts that
//! the connection keeps, for the whole input and for a continuation. Ignored;
//! `just bench-perf` runs it in release.
//!
//! The test binary has no counting allocator, so the bench reports the time and the
//! growth of the peak resident set over one build of each body.

use std::hint::black_box;
use std::time::Duration;

use efr_provider::{Message, Request, ToolDefinition};
use efr_test_support::bench::{self, Memory, Report};
use efr_test_support::{PerfCorpus, PerfSize};
use serde_json::{Value, json};

use crate::openai::OpenAiConfig;
use crate::openai::convert::{ResponsesBody, request_body};
use crate::openai::websocket::continuation::{Continuation, Plan, plan};

/// The seed of the corpus.
const SEED: u64 = 42;

/// The ladder: at least so many input items and so many bytes of input JSON. The
/// plan names a 3 MB input; the items of the corpus are smaller than that plan's, so
/// the third size takes the bytes.
const LADDER: [(usize, usize); 3] = [(200, 0), (2_000, 0), (0, 3_000_000)];

const KEY: &str = "0192f0c1-conversation";

fn shell_tool() -> ToolDefinition {
    ToolDefinition::function(
        "shell",
        "Run a command in the user's shell.",
        json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        }),
    )
}

fn request(messages: Vec<Message>) -> Request {
    let mut request = Request::new("gpt-5.5");
    request.system = Some("You are efr, an agent in the user's shell.".to_owned());
    request.tools = vec![shell_tool()];
    request.messages = messages;
    request.provider_options.insert("prompt_cache_key".to_owned(), json!(KEY));
    request
}

fn body(messages: &[Message]) -> ResponsesBody {
    request_body(&request(messages.to_vec()), &OpenAiConfig::subscription(), None)
}

/// A history of the corpus.
struct History {
    /// Whole turns, then the next turn's prompt: the request of that turn's first
    /// model call.
    messages: Vec<Message>,
    /// The whole turns.
    turns: usize,
    /// The next turn's first answer and its tool result: with `messages`, the request
    /// of its second model call.
    answer: Message,
    result: Message,
}

/// Whole turns of the corpus until the input holds at least `items` items and `bytes`
/// bytes of JSON.
fn history(items: usize, bytes: usize) -> History {
    let mut messages = Vec::new();
    let mut corpus = PerfCorpus::new(PerfSize::L, SEED);
    let (mut held_items, mut held_bytes, mut turns) = (0, 0, 0);
    loop {
        let turn = corpus.next().unwrap();
        if held_items >= items && held_bytes >= bytes {
            let mut turn = turn.messages.into_iter();
            messages.push(turn.next().unwrap());
            let (answer, result) = (turn.next().unwrap(), turn.next().unwrap());
            return History { messages, turns, answer, result };
        }
        // NOTE: the items of one message do not depend on the other messages.
        let input = body(&turn.messages).input().to_vec();
        held_items += input.len();
        held_bytes += serde_json::to_vec(&input).unwrap().len();
        turns += 1;
        messages.extend(turn.messages);
    }
}

/// What a WebSocket call builds after `request_body`, as `Sockets::stream` does: the
/// plan, the `response.create` message, and the settings and the input that the
/// connection keeps for the next continuation.
fn websocket_parts(
    last: Option<&Continuation>,
    body: &ResponsesBody,
) -> (String, ResponsesBody, Vec<Value>) {
    let (previous, input) = match plan(last, body) {
        Plan::Full => (None, body.input()),
        Plan::Incremental { previous, input } => (Some(previous), input),
    };
    let message = serde_json::to_string(&body.websocket_create(previous, input)).unwrap();
    (message, body.settings(), body.input().to_vec())
}

/// The growth of the peak resident set over one run of `f`.
fn peak_of(f: impl FnOnce()) -> u64 {
    bench::reset_peak();
    let before = Memory::of_self().unwrap();
    f();
    let after = Memory::of_self().unwrap();
    after.peak.saturating_sub(before.rss)
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_ws_body() {
    let mut report = Report::new("bench_perf_ws_body");
    let mut https = Vec::new();
    let mut fulls = Vec::new();
    let mut incrementals = Vec::new();
    for (items, bytes) in LADDER {
        let History { messages, turns, answer, result } = history(items, bytes);
        let first = body(&messages);
        let items = first.input().len();
        let name = format!("{items} items");
        report.count(&format!("{name}: input items"), items as u64);
        report.count(&format!("{name}: turns of history"), turns as u64);
        report.bytes(
            &format!("{name}: input as JSON"),
            serde_json::to_vec(first.input()).unwrap().len() as u64,
        );

        // The HTTP path: the body, then its bytes.
        let http = || {
            let body = body(&messages);
            black_box(serde_json::to_vec(&body).unwrap());
        };
        report.bytes(&format!("{name}: http peak growth"), peak_of(http));
        let samples = bench::run(2, 20, http);
        https.push((items as u64, report.time(&format!("{name}: http body"), &samples).p50));
        let samples = bench::run(2, 20, || {
            black_box(body(&messages));
        });
        report.time(&format!("{name}:   of it request_body"), &samples);

        // The WebSocket with the whole input: no previous answer on the connection.
        let full = || {
            let body = body(&messages);
            black_box(websocket_parts(None, &body));
        };
        report.bytes(&format!("{name}: websocket full peak growth"), peak_of(full));
        let samples = bench::run(2, 20, full);
        fulls.push((items as u64, report.time(&format!("{name}: websocket full"), &samples).p50));

        // The turn's second call on the same connection: the first answer completed, so
        // only its tool result goes.
        let raw = match &answer.provider_raw {
            Some(Value::Array(raw)) => raw.clone(),
            _ => unreachable!("an answer of the corpus has its items"),
        };
        let mut known = first.input().to_vec();
        known.extend(raw);
        let last = Continuation {
            settings: first.settings(),
            input: known,
            response_id: "resp_1".to_owned(),
        };
        let mut next = messages.clone();
        next.extend([answer, result]);
        let second = body(&next);
        let (message, _, _) = websocket_parts(Some(&last), &second);
        assert!(message.contains("\"previous_response_id\":\"resp_1\""), "an incremental call");
        report.bytes(&format!("{name}: incremental message"), message.len() as u64);
        let incremental = || {
            let body = body(&next);
            black_box(websocket_parts(Some(&last), &body));
        };
        report.bytes(&format!("{name}: websocket incremental peak growth"), peak_of(incremental));
        let samples = bench::run(2, 20, incremental);
        incrementals.push((
            items as u64,
            report.time(&format!("{name}: websocket incremental"), &samples).p50,
        ));
        let samples = bench::run(2, 20, || {
            black_box(websocket_parts(Some(&last), &second));
        });
        let stats = report.time(&format!("{name}:   of it plan, message and kept parts"), &samples);
        if stats.p50 > Duration::from_secs(10) {
            report.count("stopped_at_n", items as u64);
            break;
        }
    }
    report.slope("http body over the items", &https);
    report.slope("websocket full over the items", &fulls);
    report.slope("websocket incremental over the items", &incrementals);
    report.finish();
}
