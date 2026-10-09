//! The tools of a request: the edit tool that the turn's model knows, and nothing of
//! the other one.

use std::sync::Arc;

use async_trait::async_trait;
use efr_protocol::{Event, Origin};
use efr_provider::{
    EditTool, Message, ModelInfo, Provider, ProviderError, ProviderId, ProviderStream, Request,
};
use efr_test_support::{Record, ReplayProvider, Transcript};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::ConversationStart;
use crate::preamble::LiveState;
use crate::testing::{
    FakeToolbox, Harness, MODEL, Setup, answer, expect_request, freeform_answer, freeform_message,
    request, result_message, text_answer, tool_answer, tool_message,
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
fn request_for(edit: EditTool, messages: Vec<Message>) -> Request {
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

/// The model of the turns that change files with `edit`.
const CLAUDE: &str = "claude-a";

/// The request of a turn on [`CLAUDE`].
fn on_claude_model(messages: Vec<Message>) -> Request {
    Request { model: CLAUDE.to_owned(), ..request_for(EditTool::Replace, messages) }
}

/// The provider's models: [`CLAUDE`] with `edit` and [`MODEL`] with `apply_patch`.
fn both_models() -> Vec<ModelInfo> {
    vec![ModelInfo::new(CLAUDE).with_edit_tool(EditTool::Replace), ModelInfo::new(MODEL)]
}

/// Sends `text` with the model `model` and waits for the end of its turn.
async fn turn_on(h: &mut Harness, model: &str, text: &str) {
    let cwd = h.cwd.clone();
    let mut params = h.prompt_params(&cwd, text);
    params.settings.model = Some(model.to_owned());
    let sent = h.handle.send_prompt(params, Origin::Shell).await.expect("prompt accepted");
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
}

/// The JSON of each request's messages without the closing bracket is the start of
/// the next request's messages.
fn assert_each_starts_the_next(requests: &[Request]) {
    for pair in requests.windows(2) {
        let before = serde_json::to_string(&pair[0].messages).expect("json");
        let after = serde_json::to_string(&pair[1].messages).expect("json");
        let open = before.strip_suffix(']').expect("a list");
        assert!(after.starts_with(open), "\n{before}\nis not a prefix of\n{after}");
    }
}

#[tokio::test]
async fn an_edit_call_of_a_claude_turn_goes_to_an_openai_model_as_text() {
    let setup = Setup::new();
    let claude = LiveState { model: CLAUDE.to_owned(), ..setup.live_state(&setup.cwd, "fix") };
    let state = setup.live_state(&setup.cwd, "fix");
    let notes = setup.scratch("fix").join("notes.md");
    let input = json!({ "path": notes, "old_string": "draft", "new_string": "final" });
    let output = format!("edited {}", notes.display());
    let fix = setup.prompt(&claude, "fix");
    let call = tool_message("call_1", "edit", &input);
    let result = result_message("call_1", &output, false);
    // The text of the call and of its result, as an OpenAI model with `apply_patch`
    // reads them.
    let call_text = Message::assistant(format!(
        "Earlier tool call `edit` (id call_1), shown as text: this request does not offer \
         the tool. Its input:\n{input}"
    ));
    let result_text =
        Message::user(format!("Result of the earlier tool call `edit` (id call_1):\n{output}"));
    let (next, then) = (setup.prompt(&state, "next"), setup.prompt(&state, "then"));
    let on_openai = vec![
        request_for(
            EditTool::ApplyPatch,
            vec![
                fix.clone(),
                call_text.clone(),
                result_text.clone(),
                Message::assistant("Fixed."),
                next.clone(),
            ],
        ),
        request_for(
            EditTool::ApplyPatch,
            vec![
                fix.clone(),
                call_text,
                result_text,
                Message::assistant("Fixed."),
                next,
                Message::assistant("Next."),
                then,
            ],
        ),
    ];
    let records = vec![
        expect_request(on_claude_model(vec![fix.clone()])),
        answer(&tool_answer("call_1", "edit", &input)),
        expect_request(on_claude_model(vec![fix, call, result])),
        answer(&text_answer("Fixed.")),
        expect_request(on_openai[0].clone()),
        answer(&text_answer("Next.")),
        expect_request(on_openai[1].clone()),
        answer(&text_answer("Then.")),
    ];
    let mut h = start(setup, records, both_models()).await;

    turn_on(&mut h, CLAUDE, "fix").await;
    turn_on(&mut h, MODEL, "next").await;
    turn_on(&mut h, MODEL, "then").await;

    assert_each_starts_the_next(&on_openai);
    h.finish();
}

#[tokio::test]
async fn an_apply_patch_call_of_an_openai_turn_goes_to_a_claude_model_as_text_and_back() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "add");
    let claude = LiveState { model: CLAUDE.to_owned(), ..state.clone() };
    let notes = setup.scratch("add").join("notes.md");
    let patch =
        &format!("*** Begin Patch\n*** Add File: {}\n+hello\n*** End Patch", notes.display());
    let add = setup.prompt(&state, "add");
    let call = freeform_message("call_1", "apply_patch", patch);
    let result = result_message("call_1", "patched", false);
    let call_text = Message::assistant(format!(
        "Earlier tool call `apply_patch` (id call_1), shown as text: this request does not \
         offer the tool. Its input:\n{patch}"
    ));
    let result_text =
        Message::user("Result of the earlier tool call `apply_patch` (id call_1):\npatched");
    let (next, then) = (setup.prompt(&claude, "next"), setup.prompt(&claude, "then"));
    let back = setup.prompt(&state, "back");
    let on_claude = vec![
        on_claude_model(vec![
            add.clone(),
            call_text.clone(),
            result_text.clone(),
            Message::assistant("Added."),
            next.clone(),
        ]),
        on_claude_model(vec![
            add.clone(),
            call_text,
            result_text,
            Message::assistant("Added."),
            next.clone(),
            Message::assistant("Next."),
            then.clone(),
        ]),
    ];
    let records = vec![
        expect_request(request_for(EditTool::ApplyPatch, vec![add.clone()])),
        answer(&freeform_answer("call_1", "apply_patch", patch)),
        expect_request(request_for(
            EditTool::ApplyPatch,
            vec![add.clone(), call.clone(), result.clone()],
        )),
        answer(&text_answer("Added.")),
        expect_request(on_claude[0].clone()),
        answer(&text_answer("Next.")),
        expect_request(on_claude[1].clone()),
        answer(&text_answer("Then.")),
        // Back on a model with `apply_patch`, the call goes in its own form again.
        expect_request(request_for(
            EditTool::ApplyPatch,
            vec![
                add,
                call,
                result,
                Message::assistant("Added."),
                next,
                Message::assistant("Next."),
                then,
                Message::assistant("Then."),
                back,
            ],
        )),
        answer(&text_answer("Back.")),
    ];
    let mut h = start(setup, records, both_models()).await;

    turn_on(&mut h, MODEL, "add").await;
    turn_on(&mut h, CLAUDE, "next").await;
    turn_on(&mut h, CLAUDE, "then").await;
    turn_on(&mut h, MODEL, "back").await;

    assert_each_starts_the_next(&on_claude);
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
