use std::path::PathBuf;
use std::time::Duration;

use efr_permissions::{
    Action, CommandPattern, ConversationPolicy, DecisionInput, Effect, Engine, Locations, Policy,
    Requirements, Resource, Rule,
};
use efr_protocol::{
    ApprovalDecision, EffectiveSettings, ErrorCode, Event, InputWait, Mode, ModelInfo, ModelSource,
    Origin, OverriddenSettings, ProjectId, Scope, TurnInterrupt, TurnSettings, TurnSteer, Usage,
};
use efr_provider::{Message, ProviderEvent, StopReason, TokenUsage};
use efr_scope::{Basis, Derivation, Repo};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{bounded_tail, provider_failure};
use crate::testing::{
    MODEL, Setup, answer, default_settings, done, expect_request, failure, find, hold, request,
    result_message, text_answer, tool_answer, tool_message, user_prompt,
};
use crate::{ConversationError, approvals};

fn kinds(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// A model list with the test model and two others, each with its efforts.
fn models() -> Vec<ModelInfo> {
    let model = |id: &str, efforts: &[&str]| ModelInfo {
        id: id.to_owned(),
        efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
        default_effort: Some("medium".to_owned()),
        default: id == MODEL,
        source: ModelSource::Builtin,
    };
    vec![
        model(MODEL, &["low", "medium"]),
        model("gpt-5.4", &["low", "medium", "high"]),
        model("test-model-2", &["medium"]),
    ]
}

#[tokio::test]
async fn a_prompts_settings_reach_the_request_and_are_recorded() {
    let mut setup = Setup::new();
    setup.config.models = models();
    let mut state = setup.live_state(&setup.cwd, "hello");
    state.mode = Mode::Auto;
    let mut expected = request(vec![setup.prompt(&state, "hello")]);
    expected.model = "gpt-5.4".to_owned();
    expected.provider_options.insert("reasoning_effort".to_owned(), json!("high"));
    let records = vec![expect_request(expected), answer(&text_answer("Hi there."))];
    let mut h = setup.start(records).await;
    let cwd = h.cwd.clone();
    let mut params = h.prompt_params(&cwd, "hello");
    let asked = TurnSettings {
        mode: Some(Mode::Auto),
        model: Some("gpt-5.4".to_owned()),
        effort: Some("high".to_owned()),
    };
    params.settings = asked.clone();

    let sent = h.handle.send_prompt(params, Origin::Shell).await.unwrap();
    h.wait_end(sent.turn_id).await;

    let effective = EffectiveSettings {
        mode: Mode::Auto,
        model: "gpt-5.4".to_owned(),
        effort: Some("high".to_owned()),
        overridden: OverriddenSettings { mode: true, model: true, effort: true },
    };
    assert_eq!(sent.settings, Some(effective.clone()));
    let events = h.events().await;
    let queued = find(&events, |e| matches!(e, Event::PromptQueued { .. }));
    assert!(matches!(&queued, Event::PromptQueued { settings, .. } if *settings == asked));
    let started = find(&events, |e| matches!(e, Event::TurnStarted { .. }));
    assert!(
        matches!(&started, Event::TurnStarted { settings: Some(settings), .. } if *settings == effective),
        "{started:?}"
    );
    h.finish();
}

#[tokio::test]
async fn a_prompt_with_a_model_outside_the_list_is_refused_before_anything_is_recorded() {
    let mut setup = Setup::new();
    setup.config.models = models();
    let mut h = setup.start(Vec::new()).await;
    let cwd = h.cwd.clone();
    let mut params = h.prompt_params(&cwd, "hello");
    params.settings.model = Some("gpt-4o".to_owned());

    let refused = h.handle.send_prompt(params, Origin::Shell).await;

    assert!(
        matches!(
            &refused,
            Err(ConversationError::InvalidSetting { setting: "model", choices, .. })
                if *choices == [MODEL, "gpt-5.4", "test-model-2"]
        ),
        "{refused:?}"
    );
    assert!(h.events().await.is_empty(), "nothing is recorded, not even the conversation");
    h.finish();
}

#[tokio::test]
async fn a_held_prompt_keeps_its_settings_and_fails_when_they_no_longer_fit() {
    let mut setup = Setup::new();
    setup.config.models = models();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&[ProviderEvent::TextDelta { text: "One.".to_owned() }]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
    ];
    let mut h = setup.start(records).await;

    let first = h.prompt("first").await;
    h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;
    let cwd = h.cwd.clone();
    let mut params = h.prompt_params(&cwd, "second");
    params.settings.model = Some("gpt-5.4".to_owned());
    let second = h.handle.send_prompt(params, Origin::Shell).await.unwrap();
    assert!(second.queued);
    assert_eq!(second.settings.as_ref().map(|settings| settings.model.as_str()), Some("gpt-5.4"));
    // The config drops gpt-5.4 while the prompt waits.
    let mut changed = (**h.settings.borrow()).clone();
    changed.models.retain(|model| model.id != "gpt-5.4");
    h.settings.send_replace(std::sync::Arc::new(changed));
    h.provider.handled_through(3);
    h.wait_end(first.turn_id).await;
    let end = h.wait_end(second.turn_id).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn fails, got {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Invalid);
    assert_eq!(
        error.message,
        "the model gpt-5.4 is not in the model list; choose one of: test-model, test-model-2"
    );
    assert_eq!(
        error.data,
        Some(json!({
            "setting": "model",
            "value": "gpt-5.4",
            "choices": [MODEL, "test-model-2"],
        }))
    );
    let events = h.events().await;
    let held = events.iter().find(|e| {
        matches!(e, Event::PromptQueued { turn_id, settings, .. }
            if *turn_id == second.turn_id && settings.model.as_deref() == Some("gpt-5.4"))
    });
    assert!(held.is_some(), "the held prompt keeps the settings it asked for");
    let started = events
        .iter()
        .any(|e| matches!(e, Event::TurnStarted { turn_id, .. } if *turn_id == second.turn_id));
    assert!(!started, "a turn whose settings no longer fit never starts");
    h.finish();
}

#[tokio::test]
async fn a_phone_turn_runs_with_at_most_cautious() {
    let mut setup = Setup::new();
    setup.config.mode = Mode::Auto;
    let state = setup.live_state(&setup.cwd, "hello");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "hello")])),
        answer(&text_answer("Hi.")),
    ];
    let mut h = setup.start(records).await;
    let cwd = h.cwd.clone();
    let mut params = h.prompt_params(&cwd, "hello");
    params.settings.mode = Some(Mode::Auto);

    let sent = h.handle.send_prompt(params, Origin::Phone).await.unwrap();
    h.wait_end(sent.turn_id).await;

    let settings = sent.settings.unwrap();
    assert_eq!(settings.mode, Mode::Cautious);
    assert!(settings.overridden.mode, "the prompt asked for a mode, capped for a phone");
    let events = h.events().await;
    let started = find(&events, |e| matches!(e, Event::TurnStarted { .. }));
    assert!(matches!(
        started,
        Event::TurnStarted { settings: Some(EffectiveSettings { mode: Mode::Cautious, .. }), .. }
    ));
    h.finish();
}

#[tokio::test]
async fn a_model_switch_sends_no_provider_items_of_the_other_model_and_a_switch_back_does() {
    let mut setup = Setup::new();
    setup.config.models = models();
    let state = setup.live_state(&setup.cwd, "first");
    let raw = json!([
        { "type": "reasoning", "id": "rs_1", "encrypted_content": "opaque" },
        { "type": "message", "id": "msg_1", "role": "assistant", "content": [] },
    ]);
    let mut first_answer = text_answer("One.");
    first_answer[1] = done(StopReason::EndTurn, Some(raw.clone()));
    let mut second = request(vec![
        Message::user("first"),
        Message::assistant("One."),
        setup.prompt(&state, "second"),
    ]);
    second.model = "test-model-2".to_owned();
    let third = request(vec![
        Message::user("first"),
        Message::assistant("One.").with_provider_raw(raw),
        Message::user("second"),
        Message::assistant("Two."),
        setup.prompt(&state, "third"),
    ]);
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&first_answer),
        expect_request(second),
        answer(&text_answer("Two.")),
        expect_request(third),
        answer(&text_answer("Three.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("first").await;
    h.wait_end(sent.turn_id).await;
    let cwd = h.cwd.clone();
    let mut params = h.prompt_params(&cwd, "second");
    params.settings.model = Some("test-model-2".to_owned());
    let sent = h.handle.send_prompt(params, Origin::Shell).await.unwrap();
    h.wait_end(sent.turn_id).await;
    let sent = h.prompt("third").await;
    h.wait_end(sent.turn_id).await;

    // The replay compares each request with the expected one in full: the second
    // carries no item of the first model, the third, on the first model again, does.
    h.finish();
}

#[tokio::test]
async fn a_text_turn_records_the_answer_and_completes() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "hello");
    let scratch = setup.scratch("hello");
    let mut answer_events = text_answer("Hi there.");
    answer_events.insert(
        1,
        ProviderEvent::Usage(TokenUsage {
            input_tokens: 10,
            output_tokens: 4,
            ..TokenUsage::default()
        }),
    );
    let records =
        vec![expect_request(request(vec![setup.prompt(&state, "hello")])), answer(&answer_events)];
    let mut h = setup.start(records).await;

    let sent = h.prompt("hello").await;
    assert!(!sent.queued);
    assert_eq!(sent.seq.get(), 2, "conversation_created is 1, prompt_queued is 2");
    let end = h.wait_end(sent.turn_id).await;

    let usage = Some(Usage { input_tokens: 10, output_tokens: 4 });
    assert_eq!(end, Event::TurnCompleted { turn_id: sent.turn_id, usage });
    assert_eq!(
        h.kinds().await,
        kinds(&[
            "conversation_created",
            "prompt_queued",
            "turn_started",
            "assistant_message_updated",
            "assistant_message_completed",
            "turn_completed",
        ])
    );
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::TurnStarted { .. })),
        Event::TurnStarted {
            turn_id: sent.turn_id,
            cwd: h.cwd.clone(),
            scope: Scope::Machine,
            settings: default_settings(),
        }
    );
    assert_eq!(
        find(&events, |e| matches!(e, Event::AssistantMessageCompleted { .. })),
        Event::AssistantMessageCompleted {
            turn_id: sent.turn_id,
            index: 0,
            text: "Hi there.".to_owned()
        }
    );
    assert!(scratch.is_dir(), "the turn claimed $SCRATCH");
    h.finish();
}

#[tokio::test]
async fn an_allowed_tool_call_runs_and_its_result_goes_back_to_the_model() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "read my notes");
    let notes = setup.home().join("notes.txt");
    let input = json!({ "path": notes });
    let first = setup.prompt(&state, "read my notes");
    let output = format!("contents of {}", notes.display());
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "read_file", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "read_file", &input),
            result_message("call_1", &output, false),
        ])),
        answer(&text_answer("Your notes say hi.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("read my notes").await;
    h.wait_end(sent.turn_id).await;

    assert_eq!(h.toolbox.invoked(), vec![("read_file".to_owned(), input.clone())]);
    assert_eq!(
        h.kinds().await,
        kinds(&[
            "conversation_created",
            "prompt_queued",
            "turn_started",
            "tool_call_started",
            "tool_call_completed",
            "assistant_message_updated",
            "assistant_message_completed",
            "turn_completed",
        ])
    );
    let events = h.events().await;
    let call_id = h.call_ids().await[0];
    assert_eq!(
        find(&events, |e| matches!(e, Event::ToolCallStarted { .. })),
        Event::ToolCallStarted {
            turn_id: sent.turn_id,
            call_id,
            tool: "read_file".to_owned(),
            input
        }
    );
    assert_eq!(
        find(&events, |e| matches!(e, Event::ToolCallCompleted { .. })),
        Event::ToolCallCompleted {
            turn_id: sent.turn_id,
            call_id,
            output,
            truncated: false,
            is_error: false,
            exit_code: None
        }
    );
    h.finish();
}

#[tokio::test]
async fn a_denied_tool_call_never_runs_and_the_model_reads_the_path_class() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "read my key");
    let key = setup.home().join(".ssh/id_ed25519");
    let input = json!({ "path": key });
    let engine = Engine::with_defaults(Locations::new(setup.home()).expect("home"));
    let decision = engine.decide(&DecisionInput {
        requirements: Requirements::none().with_read(&key),
        scope: Scope::Machine,
        origin: Origin::Shell,
        mode: Mode::Cautious,
        conversation_policy: ConversationPolicy::new(setup.scratch("read my key")),
    });
    assert_eq!(decision.effect(), Effect::Deny);
    let denial = approvals::denial("read_file", &decision);
    assert!(denial.contains("(secrets)"), "the model reads the path class: {denial}");
    let first = setup.prompt(&state, "read my key");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "read_file", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "read_file", &input),
            result_message("call_1", &denial, true),
        ])),
        answer(&text_answer("I may not read keys.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("read my key").await;
    h.wait_end(sent.turn_id).await;

    assert!(h.toolbox.invoked().is_empty(), "a denied call never reaches the toolbox");
    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })));
    let completed = find(&events, |e| matches!(e, Event::ToolCallCompleted { .. }));
    assert!(matches!(completed, Event::ToolCallCompleted { is_error: true, .. }));
    h.finish();
}

#[tokio::test]
async fn an_asked_tool_call_waits_for_approval_and_runs_once_approved() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "set an alias");
    let zshrc = setup.home().join(".zshrc");
    let input = json!({ "path": zshrc, "content": "alias ll='ls -l'" });
    let first = setup.prompt(&state, "set an alias");
    let output = format!("written {}", zshrc.display());
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "write_file", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "write_file", &input),
            result_message("call_1", &output, false),
        ])),
        answer(&text_answer("Added.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("set an alias").await;
    let call_id = h.wait_approval().await;
    assert!(h.toolbox.invoked().is_empty(), "nothing runs before the answer");
    assert_eq!(h.handle.state().await.expect("state").pending_approvals, vec![call_id]);
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(sent.turn_id).await;

    assert_eq!(h.toolbox.invoked(), vec![("write_file".to_owned(), input)]);
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::ApprovalRequested { .. })),
        Event::ApprovalRequested {
            turn_id: sent.turn_id,
            call_id,
            summary: format!("write_file: write {} (user config)", zshrc.display()),
            diff_preview: Some("+\"alias ll='ls -l'\"".to_owned()),
        }
    );
    assert_eq!(
        find(&events, |e| matches!(e, Event::ApprovalResolved { .. })),
        Event::ApprovalResolved {
            turn_id: sent.turn_id,
            call_id,
            decision: ApprovalDecision::Allow,
            origin: Origin::Shell
        }
    );
    h.finish();
}

#[tokio::test]
async fn an_asked_tool_call_that_the_user_denies_never_runs() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "set an alias");
    let zshrc = setup.home().join(".zshrc");
    let input = json!({ "path": zshrc, "content": "x" });
    let first = setup.prompt(&state, "set an alias");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "write_file", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "write_file", &input),
            result_message("call_1", "The user denied the write_file call; it did not run.", true),
        ])),
        answer(&text_answer("Understood.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("set an alias").await;
    let call_id = h.wait_approval().await;
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(sent.turn_id).await;

    assert!(h.toolbox.invoked().is_empty());
    h.finish();
}

#[tokio::test]
async fn a_turn_from_the_phone_asks_before_reading_outside_scratch() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "read my notes");
    let notes = setup.home().join("notes.txt");
    let input = json!({ "path": notes });
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "read my notes")])),
        answer(&tool_answer("call_1", "read_file", &input)),
    ];
    let mut h = setup.start(records).await;

    let cwd = h.cwd.clone();
    let params = h.prompt_params(&cwd, "read my notes");
    let sent = h.handle.send_prompt(params, Origin::Phone).await.expect("prompt accepted");
    let call_id = h.wait_approval().await;

    assert!(h.toolbox.invoked().is_empty(), "the shell would have read it without asking");
    let interrupt = TurnInterrupt {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        turn_id: Some(sent.turn_id),
    };
    h.handle.interrupt(interrupt, Origin::Phone).await.expect("interrupt accepted");
    h.wait_end(sent.turn_id).await;
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::ApprovalExpired { .. })),
        Event::ApprovalExpired { turn_id: sent.turn_id, call_id }
    );
    h.finish();
}

#[tokio::test]
async fn the_scope_of_each_turn_decides_and_a_registered_project_writes_freely() {
    let mut setup = Setup::new();
    let project = ProjectId::from_uuid(efr_stdx::id::uuid_v7(
        &setup.clock,
        &efr_test_support::TestRng::new(3),
    ));
    let elsewhere = setup.dirs.create_dir("home/elsewhere").expect("directory");
    setup.project = Some((project, setup.cwd.clone()));
    setup.scope = std::mem::take(&mut setup.scope).with(
        setup.cwd.clone(),
        Derivation { scope: Scope::Project(project), basis: Basis::Registered, repo: None },
    );
    let main_rs = setup.cwd.join("main.rs");
    let input = json!({ "path": main_rs, "content": "fn main() {}" });
    let state = setup.live_state(&setup.cwd, "write main");
    let first = setup.prompt(&state, "write main");
    let output = format!("written {}", main_rs.display());
    let second_state = setup.live_state(&elsewhere, "write main");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "write_file", &input)),
        expect_request(request(vec![
            first.clone(),
            tool_message("call_1", "write_file", &input),
            result_message("call_1", &output, false),
        ])),
        answer(&text_answer("Written.")),
        expect_request(request(vec![
            Message::user("write main"),
            tool_message("call_1", "write_file", &input),
            result_message("call_1", &output, false),
            Message::assistant("Written."),
            setup.prompt(&second_state, "write it again"),
        ])),
        answer(&tool_answer("call_2", "write_file", &input)),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("write main").await;
    h.wait_end(sent.turn_id).await;
    assert_eq!(h.toolbox.invoked().len(), 1, "inside the project the write needs no approval");

    let again = h.prompt_in(&elsewhere, "write it again").await;
    h.wait_approval().await;
    assert_eq!(h.toolbox.invoked().len(), 1, "from another directory the same write asks");
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::ScopeChanged { .. })),
        Event::ScopeChanged {
            turn_id: again.turn_id,
            from: Scope::Project(project),
            to: Scope::Machine
        }
    );
    h.handle.shutdown().await.expect("the actor stops");
    h.finish();
}

#[tokio::test]
async fn an_interrupt_mid_stream_completes_the_partial_text_and_ends_the_turn() {
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
    let interrupt = TurnInterrupt {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        turn_id: Some(sent.turn_id),
    };
    let requested = h.handle.interrupt(interrupt, Origin::Shell).await.expect("interrupt accepted");
    assert_eq!(requested.turn_id, sent.turn_id);
    let end = h.wait_end(sent.turn_id).await;

    assert_eq!(end, Event::TurnInterrupted { turn_id: sent.turn_id });
    assert_eq!(
        h.kinds().await,
        kinds(&[
            "conversation_created",
            "prompt_queued",
            "turn_started",
            "assistant_message_updated",
            "turn_interrupt_requested",
            "assistant_message_completed",
            "turn_interrupted",
        ])
    );
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::AssistantMessageCompleted { .. })),
        Event::AssistantMessageCompleted {
            turn_id: sent.turn_id,
            index: 0,
            text: "Once upon a time".to_owned()
        }
    );
    h.finish();
}

#[tokio::test]
async fn an_interrupt_while_a_tool_runs_stops_it_through_the_toolbox() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "wait");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "wait")])),
        answer(&tool_answer("call_1", "hang", &json!({}))),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("wait").await;
    h.toolbox.hang_started.notified().await;
    let interrupt = TurnInterrupt {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        turn_id: None,
    };
    h.handle.interrupt(interrupt, Origin::Shell).await.expect("interrupt accepted");
    h.wait_end(sent.turn_id).await;

    let call_id = h.call_ids().await[0];
    assert_eq!(h.toolbox.cancelled(), vec![call_id]);
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::ToolCallCompleted { .. })),
        Event::ToolCallCompleted {
            turn_id: sent.turn_id,
            call_id,
            output: super::STOPPED.to_owned(),
            truncated: false,
            is_error: true,
            exit_code: None
        }
    );
    h.finish();
}

#[tokio::test]
async fn an_approval_that_times_out_expires_and_the_model_hears_it() {
    let mut setup = Setup::new();
    setup.config.approval_timeout = Some(Duration::from_secs(60));
    let state = setup.live_state(&setup.cwd, "set an alias");
    let zshrc = setup.home().join(".zshrc");
    let input = json!({ "path": zshrc, "content": "x" });
    let first = setup.prompt(&state, "set an alias");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "write_file", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "write_file", &input),
            result_message("call_1", super::EXPIRED, true),
        ])),
        answer(&text_answer("Nobody answered.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("set an alias").await;
    let call_id = h.wait_approval().await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(60));
    h.wait_end(sent.turn_id).await;

    assert!(h.toolbox.invoked().is_empty());
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::ApprovalExpired { .. })),
        Event::ApprovalExpired { turn_id: sent.turn_id, call_id }
    );
    h.finish();
}

#[tokio::test]
async fn the_approval_timeout_is_read_at_each_call_of_a_running_turn() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "set an alias");
    let zshrc = setup.home().join(".zshrc");
    let input = json!({ "path": zshrc, "content": "x" });
    let first = setup.prompt(&state, "set an alias");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        hold(),
        answer(&tool_answer("call_1", "write_file", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "write_file", &input),
            result_message("call_1", super::EXPIRED, true),
        ])),
        answer(&text_answer("Nobody answered.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("set an alias").await;
    h.wait_for(|e| matches!(e, Event::TurnStarted { .. })).await;
    let mut changed = (**h.settings.borrow()).clone();
    changed.approval_timeout = Some(Duration::from_secs(60));
    h.settings.send_replace(std::sync::Arc::new(changed));
    h.provider.handled_through(2);
    h.wait_approval().await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(60));
    h.wait_end(sent.turn_id).await;

    let events = h.events().await;
    assert!(events.iter().any(|e| matches!(e, Event::ApprovalExpired { .. })), "{events:?}");
    h.finish();
}

#[tokio::test]
async fn a_provider_401_fails_the_turn_as_unauthorized() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "hello");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "hello")])),
        failure(json!({ "kind": "unauthorized" })),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("hello").await;
    let end = h.wait_end(sent.turn_id).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn failed, got {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert_eq!(error.message, "the provider rejected the credentials");
    assert_eq!(
        h.kinds().await,
        kinds(&["conversation_created", "prompt_queued", "turn_started", "turn_failed"])
    );
    h.finish();
}

#[tokio::test]
async fn a_cwd_move_between_turns_changes_the_scope_and_the_preamble() {
    let mut setup = Setup::new();
    let elsewhere = setup.dirs.create_dir("home/elsewhere").expect("directory");
    let repo = Repo { root: setup.cwd.clone(), branch: Some("main".to_owned()) };
    setup.scope = std::mem::take(&mut setup.scope).with(
        setup.cwd.clone(),
        Derivation {
            scope: Scope::Path(setup.cwd.clone()),
            basis: Basis::WorkTree,
            repo: Some(repo.clone()),
        },
    );
    let mut first_state = setup.live_state(&setup.cwd, "first");
    first_state.repo = Some(repo);
    let second_state = setup.live_state(&elsewhere, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&first_state, "first")])),
        answer(&text_answer("One.")),
        expect_request(request(vec![
            Message::user("first"),
            Message::assistant("One."),
            setup.prompt(&second_state, "second"),
        ])),
        answer(&text_answer("Two.")),
    ];
    let mut h = setup.start(records).await;

    let first = h.prompt("first").await;
    h.wait_end(first.turn_id).await;
    let second = h.prompt_in(&elsewhere, "second").await;
    h.wait_end(second.turn_id).await;

    let cwd = h.cwd.clone();
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::ScopeChanged { .. })),
        Event::ScopeChanged { turn_id: second.turn_id, from: Scope::Path(cwd), to: Scope::Machine }
    );
    assert_eq!(
        find(
            &events,
            |e| matches!(e, Event::TurnStarted { turn_id, .. } if *turn_id == second.turn_id)
        ),
        Event::TurnStarted {
            turn_id: second.turn_id,
            cwd: elsewhere,
            scope: Scope::Machine,
            settings: default_settings(),
        }
    );
    h.finish();
}

#[tokio::test]
async fn the_preamble_names_where_the_live_hidden_shell_is() {
    let setup = Setup::new();
    let mut state = setup.live_state(&setup.cwd, "where");
    state.agent_cwd = Some(PathBuf::from("/var/log"));
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "where")])),
        answer(&text_answer("There.")),
    ];
    let mut h = setup.start(records).await;
    *h.toolbox.shell_cwd.lock().unwrap() = Some(PathBuf::from("/var/log"));

    let sent = h.prompt("where").await;
    h.wait_end(sent.turn_id).await;

    h.finish();
}

#[tokio::test]
async fn the_check_point_judges_a_command_from_where_the_hidden_shell_is() {
    let mut setup = Setup::new();
    let rule =
        Rule::new(Action::Execute, Resource::Command(CommandPattern::new("make")), Effect::Allow);
    setup.config.policy = Policy::new(vec![rule]).expect("policy");
    let mut state = setup.live_state(&setup.cwd, "build");
    state.agent_cwd = Some(PathBuf::from("/var/log"));
    let input = json!({ "command": "make" });
    let first = setup.prompt(&state, "build");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "shell", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "shell", &input),
            result_message("call_1", "done", false),
        ])),
        answer(&text_answer("Built.")),
    ];
    let cwd = setup.cwd.clone();
    let mut h = setup.start(records).await;
    *h.toolbox.shell_cwd.lock().unwrap() = Some(PathBuf::from("/var/log"));

    let sent = h.prompt("build").await;
    h.wait_end(sent.turn_id).await;

    let judged = h.toolbox.judged();
    assert_eq!(judged.len(), 1);
    assert_eq!(judged[0].shell_cwd, Some(PathBuf::from("/var/log")));
    assert_eq!(judged[0].cwd, cwd);
    h.finish();
}

#[tokio::test]
async fn the_check_point_decides_by_the_permission_mode_of_the_settings() {
    // `rm` is a writer program: the auto table lets it run, and the path rules judge
    // what it writes (here nothing is declared), while cautious asks for it.
    let mut setup = Setup::new();
    setup.config.mode = Mode::Auto;
    let mut state = setup.live_state(&setup.cwd, "clean up");
    state.mode = Mode::Auto;
    let input = json!({ "command": "rm build.log" });
    let first = setup.prompt(&state, "clean up");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "shell", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "shell", &input),
            result_message("call_1", "done", false),
        ])),
        answer(&text_answer("Removed.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("clean up").await;
    h.wait_end(sent.turn_id).await;

    assert_eq!(h.toolbox.invoked(), vec![("shell".to_owned(), input)]);
    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })));
    h.finish();
}

#[tokio::test]
async fn the_cautious_mode_asks_for_a_writer_program() {
    let setup = Setup::new();
    assert_eq!(setup.config.mode, Mode::Cautious, "cautious is the default");
    let state = setup.live_state(&setup.cwd, "clean up");
    let input = json!({ "command": "rm build.log" });
    let first = setup.prompt(&state, "clean up");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "shell", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "shell", &input),
            result_message("call_1", "The user denied the shell call; it did not run.", true),
        ])),
        answer(&text_answer("Kept.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("clean up").await;
    let call_id = h.wait_approval().await;
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(sent.turn_id).await;

    assert!(h.toolbox.invoked().is_empty());
    h.finish();
}

#[tokio::test]
async fn provider_items_go_back_to_the_same_provider_and_not_to_another() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let raw = json!([{ "type": "reasoning", "encrypted_content": "opaque" }]);
    let notes = setup.home().join("notes.txt");
    let input = json!({ "path": notes });
    let output = format!("contents of {}", notes.display());
    let mut final_answer = text_answer("One.");
    final_answer[1] = done(StopReason::EndTurn, Some(raw.clone()));
    let first = setup.prompt(&state, "first");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "read_file", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "read_file", &input),
            result_message("call_1", &output, false),
        ])),
        answer(&final_answer),
        expect_request(request(vec![
            Message::user("first"),
            tool_message("call_1", "read_file", &input),
            result_message("call_1", &output, false),
            Message::assistant("One.").with_provider_raw(raw),
            setup.prompt(&state, "second"),
        ])),
        answer(&text_answer("Two.")),
    ];
    let mut h = setup.start(records).await;
    let sent = h.prompt("first").await;
    h.wait_end(sent.turn_id).await;
    let sent = h.prompt("second").await;
    h.wait_end(sent.turn_id).await;
    h.finish();

    // After a restart the cache is gone and another provider answers: the history comes
    // from the saved messages, as it would from the cache, without provider items.
    let third = vec![
        Message::user("first"),
        tool_message("call_1", "read_file", &input),
        result_message("call_1", &output, false),
        Message::assistant("One."),
        Message::user("second"),
        Message::assistant("Two."),
        user_prompt(&state, "third"),
    ];
    let cwd = h.cwd.clone();
    let records = vec![expect_request(request(third)), answer(&text_answer("Three."))];
    let mut h = h.restart(records, "other").await;
    let sent = h.prompt_in(&cwd, "third").await;
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnCompleted { .. }), "got {end:?}");
    let created =
        h.events().await.iter().filter(|e| matches!(e, Event::ConversationCreated { .. })).count();
    assert_eq!(created, 1, "an existing conversation is not created again");
    h.finish();
}

/// The second request of a conversation whose first turn called a tool and answered
/// with provider items: the first turn exactly as the model saw it, then the prompt.
fn second_request_after_raw_items(
    setup: &Setup,
    state: &crate::preamble::LiveState,
    input: &serde_json::Value,
    output: &str,
    raw: &serde_json::Value,
) -> Vec<Message> {
    vec![
        Message::user("first"),
        tool_message("call_1", "read_file", input).with_provider_raw(json!([{ "id": "fc_1" }])),
        result_message("call_1", output, false),
        Message::assistant("One.").with_provider_raw(raw.clone()),
        setup.prompt(state, "second"),
    ]
}

#[tokio::test]
async fn a_restart_between_turns_sends_the_same_provider_items_as_no_restart() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let raw = json!([{ "type": "reasoning", "id": "rs_1", "encrypted_content": "opaque" }]);
    let notes = setup.home().join("notes.txt");
    let input = json!({ "path": notes });
    let output = format!("contents of {}", notes.display());
    let mut call_answer = tool_answer("call_1", "read_file", &input);
    let last = call_answer.len() - 1;
    call_answer[last] = done(StopReason::ToolUse, Some(json!([{ "id": "fc_1" }])));
    let mut final_answer = text_answer("One.");
    final_answer[1] = done(StopReason::EndTurn, Some(raw.clone()));
    let first = setup.prompt(&state, "first");
    let second = second_request_after_raw_items(&setup, &state, &input, &output, &raw);
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&call_answer),
        expect_request(request(vec![
            first,
            tool_message("call_1", "read_file", &input)
                .with_provider_raw(json!([{ "id": "fc_1" }])),
            result_message("call_1", &output, false),
        ])),
        answer(&final_answer),
    ];
    let mut h = setup.start(records).await;
    let sent = h.prompt("first").await;
    h.wait_end(sent.turn_id).await;
    h.finish();
    let cwd = h.cwd.clone();

    // The same provider and model answer after the restart, so the request carries the
    // first turn's items exactly as the cache would have.
    let records = vec![expect_request(request(second)), answer(&text_answer("Two."))];
    let mut h = h.restart(records, "replay").await;
    let sent = h.prompt_in(&cwd, "second").await;
    let end = h.wait_end(sent.turn_id).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "got {end:?}");
    h.finish();
}

#[tokio::test]
async fn steering_reaches_the_model_at_its_next_step() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "tidy up");
    let first = setup.prompt(&state, "tidy up");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&[ProviderEvent::TextDelta { text: "Working.".to_owned() }]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
        expect_request(request(vec![
            first,
            Message::assistant("Working."),
            Message::user("also empty the trash"),
        ])),
        answer(&text_answer("Trash emptied too.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("tidy up").await;
    h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;
    let steer = TurnSteer {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        turn_id: Some(sent.turn_id),
        text: "also empty the trash".to_owned(),
    };
    let steered = h.handle.steer(steer).await.expect("steer accepted");
    assert_eq!(steered.turn_id, sent.turn_id);
    h.provider.handled_through(3);
    h.wait_end(sent.turn_id).await;

    assert_eq!(
        h.kinds().await,
        kinds(&[
            "conversation_created",
            "prompt_queued",
            "turn_started",
            "assistant_message_updated",
            "turn_steered",
            "assistant_message_completed",
            "assistant_message_updated",
            "assistant_message_completed",
            "turn_completed",
        ])
    );
    h.finish();
}

#[tokio::test]
async fn a_running_turn_keeps_its_settings_and_the_next_turn_reads_the_new_ones() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "tidy up");
    let first = setup.prompt(&state, "tidy up");
    let mut next = request(vec![
        Message::user("tidy up"),
        Message::assistant("Working."),
        Message::user("also empty the trash"),
        Message::assistant("Done."),
        setup.prompt(&state, "next"),
    ]);
    next.model = "test-model-2".to_owned();
    next.system = Some("new rules".to_owned());
    next.max_output_tokens = Some(64);
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&[ProviderEvent::TextDelta { text: "Working.".to_owned() }]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
        expect_request(request(vec![
            first,
            Message::assistant("Working."),
            Message::user("also empty the trash"),
        ])),
        answer(&text_answer("Done.")),
        expect_request(next),
        answer(&text_answer("Next.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("tidy up").await;
    h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;
    let mut changed = (**h.settings.borrow()).clone();
    changed.model = "test-model-2".to_owned();
    changed.system_prompt = Some("new rules".to_owned());
    changed.max_output_tokens = Some(64);
    h.settings.send_replace(std::sync::Arc::new(changed));
    let steer = TurnSteer {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        turn_id: Some(sent.turn_id),
        text: "also empty the trash".to_owned(),
    };
    h.handle.steer(steer).await.expect("steer accepted");
    h.provider.handled_through(3);
    h.wait_end(sent.turn_id).await;
    let second = h.prompt("next").await;
    h.wait_end(second.turn_id).await;

    h.finish();
}

#[tokio::test]
async fn text_updates_are_coalesced_on_the_clock() {
    let mut setup = Setup::new();
    setup.config.update_interval = Duration::from_millis(200);
    let state = setup.live_state(&setup.cwd, "count");
    let delta = |text: &str| ProviderEvent::TextDelta { text: text.to_owned() };
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "count")])),
        answer(&[delta("one"), delta(" two"), delta(" three")]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("count").await;
    let first = h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;
    assert!(
        matches!(first, Event::AssistantMessageUpdated { offset: 0, ref delta, .. } if delta == "one"),
        "{first:?}"
    );
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_millis(200));
    let flushed = h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;
    assert!(
        matches!(flushed, Event::AssistantMessageUpdated { offset: 3, ref delta, .. } if delta == " two three"),
        "the held text is sent when the interval ends, without what went before: {flushed:?}"
    );
    h.provider.handled_through(3);
    h.wait_end(sent.turn_id).await;

    let updates = h
        .events()
        .await
        .iter()
        .filter(|e| matches!(e, Event::AssistantMessageUpdated { .. }))
        .count();
    assert_eq!(updates, 2);
    h.finish();
}

#[tokio::test]
async fn a_tool_s_output_updates_are_recorded_and_conversation_rules_allow_a_command() {
    let mut setup = Setup::new();
    // `make` is not one of the read-only commands that the defaults allow.
    let rule =
        Rule::new(Action::Execute, Resource::Command(CommandPattern::new("make")), Effect::Allow);
    setup.config.policy = Policy::new(vec![rule]).expect("policy");
    let state = setup.live_state(&setup.cwd, "list");
    let input = json!({ "command": "make" });
    let first = setup.prompt(&state, "list");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "shell", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "shell", &input),
            result_message("call_1", "done", false),
        ])),
        answer(&text_answer("Listed.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("list").await;
    h.wait_end(sent.turn_id).await;

    let call_id = h.call_ids().await[0];
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::ToolCallOutputUpdated { .. })),
        Event::ToolCallOutputUpdated {
            turn_id: sent.turn_id,
            call_id,
            tail: "partial".to_owned(),
            bytes: 7
        }
    );
    assert!(matches!(
        find(&events, |e| matches!(e, Event::ToolCallCompleted { .. })),
        Event::ToolCallCompleted { exit_code: Some(0), is_error: false, .. }
    ));
    h.finish();
}

#[tokio::test]
async fn input_waits_are_recorded_in_order_with_the_output_before_the_completion() {
    let mut setup = Setup::new();
    let rule = Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("ask-password")),
        Effect::Allow,
    );
    setup.config.policy = Policy::new(vec![rule]).expect("policy");
    let state = setup.live_state(&setup.cwd, "log in");
    let input = json!({ "command": "ask-password" });
    let first = setup.prompt(&state, "log in");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "shell", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "shell", &input),
            result_message("call_1", "pw: \nok\n", false),
        ])),
        answer(&text_answer("Logged in.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("log in").await;
    h.wait_end(sent.turn_id).await;

    let call_id = h.call_ids().await[0];
    let turn_id = sent.turn_id;
    let steps: Vec<Event> = h
        .events()
        .await
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                Event::ToolCallOutputUpdated { .. }
                    | Event::ToolCallInputChanged { .. }
                    | Event::ToolCallCompleted { .. }
            )
        })
        .collect();
    assert_eq!(
        steps,
        [
            Event::ToolCallOutputUpdated { turn_id, call_id, tail: "pw: ".to_owned(), bytes: 4 },
            Event::ToolCallInputChanged {
                turn_id,
                call_id,
                input: InputWait::Hidden,
                looks_secret: false,
            },
            Event::ToolCallOutputUpdated {
                turn_id,
                call_id,
                tail: "pw: \nok\n".to_owned(),
                bytes: 8
            },
            Event::ToolCallInputChanged {
                turn_id,
                call_id,
                input: InputWait::None,
                looks_secret: false,
            },
            Event::ToolCallCompleted {
                turn_id,
                call_id,
                output: "pw: \nok\n".to_owned(),
                truncated: false,
                is_error: false,
                exit_code: Some(0),
            },
        ]
    );
    h.finish();
}

#[tokio::test]
async fn an_unknown_tool_is_refused_with_the_toolbox_s_words() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "go");
    let first = setup.prompt(&state, "go");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "nope", &json!({}))),
        expect_request(request(vec![
            first,
            tool_message("call_1", "nope", &json!({})),
            result_message("call_1", "no tool is named \"nope\"", true),
        ])),
        answer(&text_answer("That tool does not exist.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("go").await;
    h.wait_end(sent.turn_id).await;

    assert!(h.toolbox.invoked().is_empty());
    h.finish();
}

#[tokio::test]
async fn a_turn_that_needs_more_model_calls_than_allowed_fails() {
    let mut setup = Setup::new();
    setup.config.max_model_calls = 1;
    let state = setup.live_state(&setup.cwd, "loop");
    let notes = setup.home().join("notes.txt");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "loop")])),
        answer(&tool_answer("call_1", "read_file", &json!({ "path": notes }))),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("loop").await;
    let end = h.wait_end(sent.turn_id).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn failed, got {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(error.message.contains("1 model calls"), "{}", error.message);
    h.finish();
}

#[tokio::test]
async fn a_missing_scratch_directory_is_made_again_under_the_same_name() {
    let setup = Setup::new();
    let scratch = setup.scratch("first");
    let first_state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&first_state, "first")])),
        answer(&text_answer("One.")),
        expect_request(request(vec![
            Message::user("first"),
            Message::assistant("One."),
            setup.prompt(&first_state, "second"),
        ])),
        answer(&text_answer("Two.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("first").await;
    h.wait_end(sent.turn_id).await;
    std::fs::remove_dir_all(&scratch).expect("remove scratch");
    let sent = h.prompt("second").await;
    h.wait_end(sent.turn_id).await;

    assert!(scratch.is_dir(), "the second turn made $SCRATCH again");
    h.finish();
}

#[tokio::test]
async fn a_scratch_root_that_cannot_be_made_fails_the_turn() {
    let mut setup = Setup::new();
    let blocker = setup.dirs.root().join("not-a-directory");
    std::fs::write(&blocker, "x").expect("file");
    setup.config.scratch_root = blocker.join("scratch");
    let mut h = setup.start(Vec::new()).await;

    let sent = h.prompt("hello").await;
    let end = h.wait_end(sent.turn_id).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn failed, got {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(
        h.kinds().await,
        kinds(&["conversation_created", "prompt_queued", "turn_started", "turn_failed"])
    );
    h.finish();
}

#[test]
fn provider_errors_map_to_codes_a_client_can_act_on() {
    use efr_provider::ProviderError;
    assert_eq!(provider_failure(&ProviderError::NotLoggedIn).code, ErrorCode::Unauthorized);
    let refused = ProviderError::Token { source: "the refresh grant was refused".into() };
    let refused = provider_failure(&refused);
    assert_eq!(refused.code, ErrorCode::Unauthorized);
    assert_eq!(refused.message, "the token source could not produce an access token");
    let unknown = provider_failure(&ProviderError::UnknownModel { model: "m".to_owned() });
    assert_eq!(unknown.code, ErrorCode::Invalid);
    assert_eq!(unknown.data, Some(json!({ "model": "m" })));
    assert_eq!(provider_failure(&ProviderError::Incomplete).code, ErrorCode::Internal);
    let limited =
        provider_failure(&ProviderError::RateLimited { retry_after: Some(Duration::from_secs(2)) });
    assert_eq!(limited.code, ErrorCode::Busy);
    assert_eq!(limited.data, Some(json!({ "retry_after_ms": 2000 })));
}

#[test]
fn an_output_tail_is_cut_on_a_character_boundary() {
    let long = format!("{}{}", "é".repeat(3000), "end");
    let tail = bounded_tail(&long);
    assert!(tail.len() <= super::TAIL_MAX);
    assert!(tail.ends_with("end"));
    assert_eq!(bounded_tail("short"), "short");
}
