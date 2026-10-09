//! A conversation that moves to another provider and model: a history that grew on a
//! model with a large window and goes on with a model of a smaller one.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use efr_protocol::{CompactionTrigger, Event, ModelInfo, ModelSource, Origin};
use efr_provider::{
    ContentBlock, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request,
    Role, StopReason, TokenUsage,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::compaction::{is_results, request_tokens};
use crate::testing::{Harness, SUMMARY, Setup, big_text, compactions, done, tool_answer};
use crate::{CompactionConfig, ContextLimits, HistoryLimits};

/// The model of the turns before the switch, with a window of one million tokens.
const WIDE: &str = "claude-wide";

/// The model after the switch, with a window of 272000 tokens.
const SMALL: &str = "gpt-small";

const WIDE_WINDOW: u64 = 1_000_000;
const SMALL_WINDOW: u64 = 272_000;

/// A provider that counts each request as the conversation estimates it and refuses
/// one above the window of its model, as a real API refuses a request that does not
/// fit. It answers a summary request with [`SUMMARY`], any other request with `ok`, and
/// reports the estimate as the input of the call. With `reads`, it answers a prompt
/// with a call of `read_file` of that file whose input carries that many more bytes,
/// so each turn is a tool loop. With `reports`, it reports that input in place of the
/// estimate.
#[derive(Debug)]
struct Windowed {
    id: ProviderId,
    windows: HashMap<String, u64>,
    reads: Option<(PathBuf, usize)>,
    reports: Option<u64>,
    requests: Mutex<Vec<Request>>,
    refused: Mutex<Vec<Request>>,
}

impl Windowed {
    fn new(id: &str) -> Arc<Self> {
        Arc::new(Windowed::build(id, None))
    }

    /// A provider that reports `tokens` as the input of every call.
    fn reporting(id: &str, tokens: u64) -> Arc<Self> {
        Arc::new(Windowed { reports: Some(tokens), ..Windowed::build(id, None) })
    }

    /// A provider whose turns read the file `path` in a tool loop before they answer,
    /// with a call whose input carries `pad` more bytes.
    fn reading(id: &str, path: PathBuf, pad: usize) -> Arc<Self> {
        Arc::new(Windowed::build(id, Some((path, pad))))
    }

    fn build(id: &str, reads: Option<(PathBuf, usize)>) -> Self {
        let windows =
            HashMap::from([(WIDE.to_owned(), WIDE_WINDOW), (SMALL.to_owned(), SMALL_WINDOW)]);
        Windowed {
            id: ProviderId::new(id).expect("provider id"),
            windows,
            reads,
            reports: None,
            requests: Mutex::new(Vec::new()),
            refused: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn refused(&self) -> Vec<Request> {
        self.refused.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl Provider for Windowed {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        let tokens = request_tokens(&request);
        let window = self.windows.get(&request.model).copied().unwrap_or(0);
        if tokens > window {
            self.refused.lock().unwrap_or_else(PoisonError::into_inner).push(request);
            return Err(ProviderError::api(
                Some(400),
                Some("context_length_exceeded".to_owned()),
                "Your input exceeds the context window of this model.".to_owned(),
            ));
        }
        let text = if request.side_call { SUMMARY } else { "ok" };
        let prompted = request.messages.last().is_some_and(|last| !is_results(last));
        let reads = self.reads.clone().filter(|_| !request.side_call && prompted);
        let mut requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
        requests.push(request);
        let call_id = format!("call_{}", requests.len());
        drop(requests);
        let input_tokens = self.reports.unwrap_or(tokens);
        let usage = TokenUsage { input_tokens, output_tokens: 1, ..TokenUsage::default() };
        let mut events = match reads {
            Some((path, pad)) => {
                tool_answer(&call_id, "read_file", &json!({ "path": path, "pad": big_text(pad) }))
            }
            None => vec![
                ProviderEvent::TextDelta { text: text.to_owned() },
                done(StopReason::EndTurn, None),
            ],
        };
        events.insert(events.len() - 1, ProviderEvent::Usage(usage));
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

fn model(id: &str, window: u64, default: bool) -> ModelInfo {
    ModelInfo {
        id: id.to_owned(),
        efforts: Vec::new(),
        default_effort: None,
        default,
        source: ModelSource::Builtin,
        context_window: Some(window),
        max_context_window: None,
        prefer_websockets: false,
    }
}

/// A setup with the two models, the small one the default, and the default history
/// limits.
fn two_models() -> Setup {
    let mut setup = Setup::new();
    setup.config.model = SMALL.to_owned();
    setup.config.models = vec![model(WIDE, WIDE_WINDOW, false), model(SMALL, SMALL_WINDOW, true)];
    setup.config.history = HistoryLimits::default();
    setup
}

/// Sends `text` with the model `model` and waits for the end of its turn.
async fn turn_on(h: &mut Harness, model: &str, text: &str) -> Event {
    let cwd = h.cwd.clone();
    let mut params = h.prompt_params(&cwd, text);
    params.settings.model = Some(model.to_owned());
    let sent = h.handle.send_prompt(params, Origin::Shell).await.expect("prompt accepted");
    h.wait_end(sent.turn_id).await
}

/// Five turns of 100000 estimated tokens each on the wide model of `claude`, then a
/// restart with `openai` and its small model.
async fn grown_then_switched(claude: &Arc<Windowed>, openai: &Arc<Windowed>) -> Harness {
    grown_with(two_models(), claude, openai, 400_000).await
}

/// Five turns of `setup` on the wide model of `claude`, each with a prompt of
/// `prompt_bytes` bytes, to about 500000 estimated tokens, then a restart with `openai`
/// and its small model.
async fn grown_with(
    setup: Setup,
    claude: &Arc<Windowed>,
    openai: &Arc<Windowed>,
    prompt_bytes: usize,
) -> Harness {
    let mut h = setup.start_model(claude.clone()).await;
    for turn in 1..=5 {
        let end = turn_on(&mut h, WIDE, &format!("part {turn}\n{}", big_text(prompt_bytes))).await;
        assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    }
    let grown = claude.requests().last().map(request_tokens).unwrap_or(0);
    assert!((500_000..520_000).contains(&grown), "about 500k tokens: {grown}");
    h.restart_model(openai.clone()).await
}

#[tokio::test]
async fn a_switch_to_a_smaller_window_compacts_first_and_every_request_fits() {
    let (claude, openai) = (Windowed::new("anthropic-api"), Windowed::new("openai-api"));
    let mut h = grown_then_switched(&claude, &openai).await;
    let small = ContextLimits::new(Some(SMALL_WINDOW), CompactionConfig::default());

    let end = turn_on(&mut h, SMALL, "go on").await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    let refused = openai.refused();
    assert!(refused.is_empty(), "no request may pass the window: {:?}", refused.len());
    let requests = openai.requests();
    let [summary, call] = requests.as_slice() else {
        panic!("a summary request, then the turn's call: {}", requests.len());
    };
    // The first request on the small model is the summary request, so the hard cap of
    // the small window holds before any call of the turn.
    assert!(summary.side_call);
    assert!(request_tokens(summary) < small.trigger, "{}", request_tokens(summary));
    assert!(!call.side_call);
    assert!(request_tokens(call) < small.trigger, "{}", request_tokens(call));
    let compacted = compactions(&h).await;
    let [compaction] = compacted.as_slice() else {
        panic!("one compaction: {compacted:?}");
    };
    assert_eq!(compaction.trigger, CompactionTrigger::Auto);
    assert_eq!(compaction.model, SMALL);
    assert_eq!(compaction.window, SMALL_WINDOW);
    assert!(compaction.omitted_messages > 0, "{compaction:?}");
    assert!(compaction.tokens_after < small.trigger, "no breaker miss: {compaction:?}");
    h.finish();
}

#[tokio::test]
async fn the_summary_request_drops_the_oldest_turns_and_says_so_in_its_prompt() {
    let (claude, openai) = (Windowed::new("anthropic-api"), Windowed::new("openai-api"));
    let mut h = grown_then_switched(&claude, &openai).await;

    turn_on(&mut h, SMALL, "go on").await;

    let requests = openai.requests();
    let summary = requests.first().expect("a summary request");
    let texts: Vec<String> = summary.messages.iter().map(efr_provider::Message::text).collect();
    // The oldest prompts go first; the newest stay.
    assert!(!texts.iter().any(|text| text.contains("part 1\n")), "{:?}", short(&texts));
    assert!(texts.iter().any(|text| text.contains("part 5\n")), "{:?}", short(&texts));
    let compaction = compactions(&h).await.pop().expect("a compaction");
    let left_out = compaction.omitted_messages;
    let note =
        format!("{left_out} earlier messages are omitted: they did not fit in this request.");
    assert_eq!(texts.first(), Some(&note), "the note takes the place of the messages");
    let prompt = summary.messages.last().expect("the summary prompt");
    assert_eq!(prompt.role, Role::User);
    let said = format!(
        "This request leaves out {left_out} earlier messages of the conversation: they did not \
         fit in the model's context. Say so under the first heading."
    );
    assert!(prompt.text().contains(&said), "{}", prompt.text());
    // Never a tool result without its call: the request starts after the note with a
    // message that holds no result.
    let first = summary.messages.get(1).expect("a message after the note");
    assert!(
        !first.content.iter().any(|block| matches!(block, ContentBlock::ToolResult { .. })),
        "{first:?}"
    );
    h.finish();
}

#[tokio::test]
async fn the_first_call_on_the_new_model_meets_the_hard_cap_of_its_window() {
    let (claude, openai) = (Windowed::new("anthropic-api"), Windowed::new("openai-api"));
    let mut h = grown_then_switched(&claude, &openai).await;
    let mut config = (**h.settings.borrow()).clone();
    config.compaction = CompactionConfig::new(false, 76);
    h.settings.send_replace(Arc::new(config));

    let end = turn_on(&mut h, SMALL, "go on").await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn fails at the cap: {end:?}");
    };
    // The cap of the small window, 95% of 272k, and not the cap of the wide one.
    assert!(
        error.message.ends_with(
            "of 272k tokens, above the cap of 258k; run ,compact or start a new conversation"
        ),
        "{}",
        error.message
    );
    assert!(openai.requests().is_empty(), "nothing goes out");
    assert!(openai.refused().is_empty(), "nothing goes out");
    h.finish();
}

#[tokio::test]
async fn a_summary_request_of_tool_loops_never_parts_a_call_from_its_result() {
    // Each turn is a short prompt and a tool loop whose call is about 100000 tokens and
    // whose result is short: pruning frees nothing, so a summary runs, and the request
    // fits first without the oldest calls, where a result would come first.
    let setup = two_models();
    let claude = Windowed::reading("anthropic-api", setup.home().join("notes.md"), 400_000);
    let openai = Windowed::new("openai-api");
    let mut h = grown_with(setup, &claude, &openai, 100).await;

    let end = turn_on(&mut h, SMALL, "go on").await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert!(openai.refused().is_empty(), "no request may pass the window");
    let requests = openai.requests();
    let summary = requests.first().expect("a summary request");
    assert!(summary.side_call);
    let compaction = compactions(&h).await.pop().expect("a compaction");
    assert!(compaction.omitted_messages > 0, "the cut falls in the tool loops: {compaction:?}");
    for request in &requests {
        // The message after the note holds no result, and every result has its call
        // earlier in the request, and every call its result.
        let after_note = request
            .messages
            .iter()
            .position(|message| message.text().contains("earlier messages are omitted"));
        if let Some(note) = after_note {
            let next = request.messages.get(note + 1).expect("a message after the note");
            assert!(!is_results(next), "{next:?}");
        }
        let mut calls = Vec::new();
        let mut results = Vec::new();
        for block in request.messages.iter().flat_map(|message| &message.content) {
            match block {
                ContentBlock::ToolCall { call_id, .. } => calls.push(call_id.clone()),
                ContentBlock::ToolResult { call_id, .. } => {
                    assert!(calls.contains(call_id), "a result without its call: {call_id}");
                    results.push(call_id.clone());
                }
                _ => {}
            }
        }
        assert_eq!(calls, results, "every call has its result");
    }
    h.finish();
}

#[tokio::test]
async fn the_count_of_the_old_model_is_no_base_for_the_first_turn_on_the_new_one() {
    // The old model reported a context above the hard cap of the new window, and every
    // message of the history fits in the tail, so a compaction frees nothing.
    let claude = Windowed::reporting("anthropic-api", 500_000);
    let openai = Windowed::new("openai-api");
    let mut h = two_models().start_model(claude.clone()).await;
    let end = turn_on(&mut h, WIDE, "hello").await;
    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    let mut h = h.restart_model(openai.clone()).await;

    let end = turn_on(&mut h, SMALL, "go on").await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "the estimate of the request: {end:?}");
    assert_eq!(openai.requests().len(), 1, "the turn's call, no compaction");
    assert!(compactions(&h).await.is_empty());
    // The next turn on the same model counts from the new model's own count again.
    let end = turn_on(&mut h, SMALL, "and then").await;
    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    h.finish();
}

/// The first 40 bytes of each text, for a readable failure.
fn short(texts: &[String]) -> Vec<String> {
    texts.iter().map(|text| text.chars().take(40).collect()).collect()
}
