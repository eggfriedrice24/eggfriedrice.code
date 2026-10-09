use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{StreamExt as _, stream};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{EditTool, ModelInfo, Provider, ProviderStream};
use crate::{ContentBlock, Message, ProviderError, ProviderEvent, ProviderId, Request, StopReason};

/// A provider that answers every request with the same events and remembers the
/// requests, the way `ReplayProvider` works in `efr-test-support`. It implements only
/// the required methods.
#[derive(Debug)]
struct FakeProvider {
    id: ProviderId,
    answer: Option<Vec<ProviderEvent>>,
    seen: Mutex<Vec<Request>>,
}

impl FakeProvider {
    fn answering(answer: Vec<ProviderEvent>) -> Self {
        FakeProvider {
            id: ProviderId::new("replay").unwrap(),
            answer: Some(answer),
            seen: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Provider for FakeProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        self.seen.lock().unwrap().push(request);
        let Some(answer) = self.answer.clone() else {
            return Err(ProviderError::Unauthorized { message: None });
        };
        Ok(Box::pin(stream::iter(answer.into_iter().map(Ok))))
    }
}

fn tool_call_answer() -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::TextDelta { text: "Listing.".to_owned() },
        ProviderEvent::ToolCallStart {
            call_id: "call_1".to_owned(),
            name: "shell".to_owned(),
            freeform: false,
        },
        ProviderEvent::ToolCallEnd {
            call_id: "call_1".to_owned(),
            arguments: "{\"command\":\"ls\"}".to_owned(),
        },
        ProviderEvent::Done {
            stop_reason: StopReason::ToolUse,
            provider_raw: Some(json!([{"type": "function_call", "call_id": "call_1"}])),
        },
    ]
}

#[test]
fn the_trait_object_is_shareable_and_the_stream_can_move() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    fn assert_send<T: Send>() {}
    assert_send_sync::<dyn Provider>();
    // A stream is polled by one task at a time, so `Send` is all it needs.
    assert_send::<ProviderStream>();
}

#[tokio::test]
async fn complete_collects_the_stream_into_one_message() {
    let fake = Arc::new(FakeProvider::answering(tool_call_answer()));
    let provider: Arc<dyn Provider> = fake.clone();
    let mut request = Request::new("gpt-5-codex");
    request.messages.push(Message::user("list the files"));

    let completion = provider.complete(request.clone()).await.unwrap();

    assert_eq!(completion.stop_reason, StopReason::ToolUse);
    assert_eq!(
        completion.message.content,
        [
            ContentBlock::Text { text: "Listing.".to_owned() },
            ContentBlock::ToolCall {
                call_id: "call_1".to_owned(),
                name: "shell".to_owned(),
                input: json!({"command": "ls"}),
                freeform: false,
            },
        ]
    );
    assert_eq!(
        completion.message.provider_raw,
        Some(json!([{"type": "function_call", "call_id": "call_1"}]))
    );
    assert_eq!(*fake.seen.lock().unwrap(), [request]);
}

#[tokio::test]
async fn stream_yields_the_events_in_order() {
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::answering(tool_call_answer()));
    let events: Vec<ProviderEvent> = provider
        .stream(Request::new("m"))
        .await
        .unwrap()
        .map(|event| event.unwrap())
        .collect()
        .await;
    assert_eq!(events, tool_call_answer());
}

#[tokio::test]
async fn complete_passes_on_a_failure_to_start() {
    let provider = FakeProvider { answer: None, ..FakeProvider::answering(Vec::new()) };
    let result = provider.complete(Request::new("m")).await;
    assert!(matches!(result, Err(ProviderError::Unauthorized { .. })));
}

#[test]
fn a_provider_names_itself_and_lists_no_models_by_default() {
    let provider = FakeProvider::answering(Vec::new());
    assert_eq!(provider.id().as_str(), "replay");
    assert_eq!(provider.models(), Vec::<ModelInfo>::new());
}

#[test]
fn model_info_records_known_limits() {
    let model =
        ModelInfo::new("gpt-5-codex").with_context_window(400_000).with_max_output_tokens(128_000);
    assert_eq!(model.id, "gpt-5-codex");
    assert_eq!(model.context_window, Some(400_000));
    assert_eq!(model.max_output_tokens, Some(128_000));
    assert_eq!(ModelInfo::new("m").context_window, None);
}

#[test]
fn model_info_records_the_facts_of_a_catalog() {
    let model = ModelInfo::new("gpt-6.1-sol")
        .with_context_window(272_000)
        .with_max_context_window(872_000)
        .with_freeform_tools(true)
        .with_prefer_websockets(true);
    assert_eq!(model.context_window, Some(272_000));
    assert_eq!(model.max_context_window, Some(872_000));
    assert!(model.freeform_tools);
    assert!(model.prefer_websockets);
    let plain = ModelInfo::new("m");
    assert_eq!(plain.max_context_window, None);
    assert!(!plain.freeform_tools);
    assert!(!plain.prefer_websockets);
}

#[test]
fn a_model_changes_files_with_apply_patch_unless_it_says_otherwise() {
    assert_eq!(ModelInfo::new("gpt-6.1-sol").edit_tool, EditTool::ApplyPatch);
    assert_eq!(EditTool::default(), EditTool::ApplyPatch);
    let claude = ModelInfo::new("claude-opus-5-5").with_edit_tool(EditTool::Replace);
    assert_eq!(claude.edit_tool, EditTool::Replace);
}

#[test]
fn model_info_records_its_efforts() {
    let model = ModelInfo::new("gpt-5.5").with_efforts(["low", "medium", "high"], Some("medium"));
    assert_eq!(model.efforts, ["low", "medium", "high"]);
    assert_eq!(model.default_effort.as_deref(), Some("medium"));
    let unknown = ModelInfo::new("m");
    assert!(unknown.efforts.is_empty());
    assert_eq!(unknown.default_effort, None);
}
