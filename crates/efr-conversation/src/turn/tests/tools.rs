//! The tools of a request: the edit tool that the turn's model knows, and nothing of
//! the other one.

use std::sync::Arc;

use async_trait::async_trait;
use efr_protocol::Origin;
use efr_provider::{
    EditTool, ModelInfo, Provider, ProviderError, ProviderId, ProviderStream, Request,
};
use efr_test_support::{Record, ReplayProvider, Transcript};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::ConversationStart;
use crate::testing::{
    FakeToolbox, Harness, MODEL, Setup, answer, expect_request, request, result_message,
    text_answer, tool_answer, tool_message,
};
use crate::turn::edit_tool;

/// Starts the actor of `setup` over `records`, with a provider whose model list says
/// that each of `models` changes files with its edit tool.
async fn start(setup: Setup, records: Vec<Record>, models: Vec<ModelInfo>) -> Harness {
    let transcript = Transcript::from_records(records);
    let provider =
        ReplayProvider::new(&transcript).expect("transcript").paced().with_models(models);
    let start = ConversationStart::New { origin: Origin::Shell, tty: None };
    setup.start_provider(Arc::new(provider), start, None).await
}

/// The request of the turn's model, which changes files with `edit`.
fn request_for(edit: EditTool, messages: Vec<efr_provider::Message>) -> Request {
    Request { tools: FakeToolbox::tools_for(edit), ..request(messages) }
}

fn names(request: &Request) -> Vec<&str> {
    request.tools.iter().map(|tool| tool.name.as_str()).collect()
}

#[test]
fn a_model_gets_one_edit_tool_in_the_same_place_of_the_list() {
    let patch = request_for(EditTool::ApplyPatch, Vec::new());
    let replace = request_for(EditTool::Replace, Vec::new());
    assert_eq!(names(&patch), ["read_file", "write_file", "shell", "hang", "note", "apply_patch"]);
    assert_eq!(names(&replace), ["read_file", "write_file", "shell", "hang", "note", "edit"]);
}

#[tokio::test]
async fn a_model_that_replaces_text_gets_edit_and_no_apply_patch() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "fix the notes");
    let notes = setup.scratch("fix the notes").join("notes.md");
    let input = json!({ "path": notes, "old_string": "draft", "new_string": "final" });
    let output = format!("edited {}", notes.display());
    let first = setup.prompt(&state, "fix the notes");
    let records = vec![
        expect_request(request_for(EditTool::Replace, vec![first.clone()])),
        answer(&tool_answer("call_1", "edit", &input)),
        expect_request(request_for(
            EditTool::Replace,
            vec![
                first,
                tool_message("call_1", "edit", &input),
                result_message("call_1", &output, false),
            ],
        )),
        answer(&text_answer("Fixed.")),
    ];
    let model = ModelInfo::new(MODEL).with_edit_tool(EditTool::Replace);
    let mut h = start(setup, records, vec![model]).await;

    let sent = h.prompt("fix the notes").await;
    h.wait_end(sent.turn_id).await;

    assert_eq!(h.toolbox.invoked(), vec![("edit".to_owned(), input)]);
    let judged: Vec<EditTool> = h.toolbox.judged().iter().map(|call| call.edit_tool).collect();
    assert_eq!(judged, [EditTool::Replace], "the call says which tool the request offered");
    h.finish();
}

#[tokio::test]
async fn a_model_that_patches_files_gets_apply_patch_and_no_edit() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "hello");
    let records = vec![
        expect_request(request_for(EditTool::ApplyPatch, vec![setup.prompt(&state, "hello")])),
        answer(&text_answer("Hi.")),
    ];
    let model = ModelInfo::new(MODEL).with_edit_tool(EditTool::ApplyPatch);
    let mut h = start(setup, records, vec![model]).await;

    let sent = h.prompt("hello").await;
    h.wait_end(sent.turn_id).await;

    h.finish();
}

#[test]
fn a_model_that_the_list_does_not_name_gets_the_tool_of_the_listed_models() {
    let provider = |models: Vec<ModelInfo>| {
        ReplayProvider::new(&Transcript::from_records(Vec::new()))
            .expect("transcript")
            .with_models(models)
    };
    let claude = |id: &str| ModelInfo::new(id).with_edit_tool(EditTool::Replace);

    let named = provider(vec![claude("claude-a"), ModelInfo::new(MODEL)]);
    assert_eq!(edit_tool(&named, "claude-a"), EditTool::Replace);
    assert_eq!(edit_tool(&named, MODEL), EditTool::ApplyPatch);
    assert_eq!(edit_tool(&named, "unknown"), EditTool::ApplyPatch, "the models disagree");
    let shared = provider(vec![claude("claude-a"), claude("claude-b")]);
    assert_eq!(edit_tool(&shared, "claude-new"), EditTool::Replace);
    assert_eq!(edit_tool(&provider(Vec::new()), "any"), EditTool::ApplyPatch);
}

/// A provider of Claude models whose list has not come yet.
#[derive(Debug)]
struct NoListYet(ProviderId);

#[async_trait]
impl Provider for NoListYet {
    fn id(&self) -> &ProviderId {
        &self.0
    }

    fn default_edit_tool(&self) -> EditTool {
        EditTool::Replace
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        Err(ProviderError::NotLoggedIn)
    }
}

#[test]
fn without_a_list_the_model_gets_the_providers_own_edit_tool() {
    let provider = NoListYet(ProviderId::new("anthropic-api").expect("id"));
    assert_eq!(edit_tool(&provider, "claude-opus-5-5"), EditTool::Replace);
}
