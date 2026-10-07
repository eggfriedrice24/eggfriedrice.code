//! The check point in the `auto` sandbox: contained calls, exit questions and their
//! launches, the one-command rule, floor refusals, the fallback to `cautious` and the
//! quarantine question.

use std::path::Path;
use std::time::Duration;

use efr_permissions::{
    Action, CommandPattern, ConversationPolicy, DecisionInput, Effect, Engine, Locations, Policy,
    Requirements, Resource, Rule, WriteBind,
};
use efr_protocol::{
    ApprovalDecision, ErrorCode, Event, ExitKind, ExitSource, Grant, JudgeKind, Launch, Mode,
    ModeFallback, Origin, ProjectId, SandboxStatus, SandboxSurfaceRespond, Scope, Verdict,
};
use efr_scope::{Basis, Derivation};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::ConversationError;
use crate::approvals;
use crate::exit::{EXIT_DENIED, REFUSALS_STOPPED};
use crate::preamble::LiveState;
use crate::testing::{
    Harness, Setup, answer, expect_request, find, planted, request, result_message, text_answer,
    tool_answer, tool_message,
};

/// A setup whose turns run in `auto`, and the live state of its first turn.
fn auto(title: &str) -> (Setup, LiveState) {
    let mut setup = Setup::new();
    setup.config.mode = Mode::Auto;
    let mut state = setup.live_state(&setup.cwd, title);
    state.mode = Mode::Auto;
    (setup, state)
}

/// The records of a turn whose model calls one `shell` with `input`, reads `output`,
/// and answers `last`.
fn one_call(
    setup: &Setup,
    state: &LiveState,
    title: &str,
    input: &Value,
    output: &str,
    is_error: bool,
    last: &str,
) -> Vec<efr_test_support::Record> {
    let first = setup.prompt(state, title);
    vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "shell", input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "shell", input),
            result_message("call_1", output, is_error),
        ])),
        answer(&text_answer(last)),
    ]
}

/// The `launch` of the first `tool_call_started`.
async fn started_launch(h: &Harness) -> Option<Launch> {
    match find(&h.events().await, |e| matches!(e, Event::ToolCallStarted { .. })) {
        Event::ToolCallStarted { launch, .. } => launch,
        _ => unreachable!("find returns an event it accepted"),
    }
}

/// Every `exit_judged` of the log, as (judge, verdict).
async fn judgements(h: &Harness) -> Vec<(JudgeKind, Verdict)> {
    h.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::ExitJudged { judge, verdict, .. } => Some((judge, verdict)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn contain_runs_without_approval_and_sets_launch() {
    let (setup, state) = auto("run the tests");
    let input = json!({ "command": "cargo test" });
    let records = one_call(&setup, &state, "run the tests", &input, "done", false, "Passed.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("run the tests").await;
    h.wait_end(sent.turn_id).await;

    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })));
    assert!(!events.iter().any(|e| matches!(e, Event::ExitRequested { .. })));
    assert_eq!(started_launch(&h).await, Some(Launch::contained()));
    let ran = h.toolbox.ran();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].launch, Launch::contained());
    assert!(ran[0].exits.is_empty());
    assert!(!ran[0].approved_interactive);
    // The toolbox collects the facts that only auto reads.
    assert!(h.toolbox.judged()[0].auto);
    h.finish();
}

#[tokio::test]
async fn a_file_tool_in_auto_runs_directly_by_the_cautious_rules() {
    let (setup, state) = auto("read it");
    let path = setup.cwd.join("notes.txt");
    let input = json!({ "path": path });
    let first = setup.prompt(&state, "read it");
    let output = format!("contents of {}", path.display());
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "read_file", &input)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "read_file", &input),
            result_message("call_1", &output, false),
        ])),
        answer(&text_answer("Read.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("read it").await;
    h.wait_end(sent.turn_id).await;

    assert_eq!(h.toolbox.ran()[0].launch, Launch::Direct);
    assert_eq!(started_launch(&h).await, None, "only a launcher call names its launch");
    h.finish();
}

#[tokio::test]
async fn approved_network_exit_runs_with_open_network() {
    let (setup, state) = auto("install");
    let input = json!({ "command": "npm ci", "network": true });
    let records = one_call(&setup, &state, "install", &input, "done", false, "Installed.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("install").await;
    let call_id = h.wait_approval().await;
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(sent.turn_id).await;

    let network = Launch::Contained { grants: vec![Grant::OpenNetwork] };
    let events = h.events().await;
    // The record comes first, so a client shows the whole line in the question.
    let position = |kind: &str| events.iter().position(|e| e.kind() == kind).unwrap();
    assert!(position("exit_requested") < position("approval_requested"));
    let Event::ExitRequested { kinds, grants, source, record, .. } =
        find(&events, |e| matches!(e, Event::ExitRequested { .. }))
    else {
        unreachable!("find returns an event it accepted");
    };
    assert_eq!(kinds, vec![ExitKind::Host]);
    assert_eq!(grants, vec![Grant::OpenNetwork]);
    assert_eq!(source, ExitSource::Predicted);
    assert_eq!(record.action.line, "npm ci");
    assert_eq!(record.user_messages, vec!["install"]);
    let Event::ApprovalRequested { exit: Some(exit), .. } =
        find(&events, |e| matches!(e, Event::ApprovalRequested { .. }))
    else {
        panic!("the question shows the exit");
    };
    assert_eq!(exit.launch, network);
    assert!(exit.facts.is_empty(), "the launch says what opens: {:?}", exit.facts);
    assert!(!exit.user_only);
    assert_eq!(started_launch(&h).await, Some(network.clone()));
    assert_eq!(h.toolbox.ran()[0].launch, network);
    assert_eq!(judgements(&h).await, vec![(JudgeKind::User, Verdict::Allow)]);
    h.finish();
}

#[tokio::test]
async fn approved_write_exit_binds_one_path() {
    let (setup, state) = auto("note it");
    let notes = setup.home().join("notes");
    let line = format!("echo done >> {}/today.txt", notes.display());
    let input = json!({ "command": line });
    let records = one_call(&setup, &state, "note it", &input, "done", false, "Noted.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("note it").await;
    let call_id = h.wait_approval().await;
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(sent.turn_id).await;

    // No fact says whether the file exists, so it counts as an existing file: its
    // parent, not a shared directory, is bound, so an editor's rename works too.
    let ran = h.toolbox.ran();
    assert_eq!(
        ran[0].launch,
        Launch::Contained { grants: vec![Grant::Write { path: notes.clone() }] }
    );
    assert_eq!(ran[0].exits.len(), 1);
    assert_eq!(ran[0].exits[0].kind, ExitKind::Write);
    assert_eq!(ran[0].exits[0].bind, Some(WriteBind::Parent(notes)));
    h.finish();
}

#[tokio::test]
async fn privilege_exit_runs_unsandboxed() {
    let (setup, state) = auto("update");
    let input = json!({ "command": "sudo pacman -Syu" });
    let records = one_call(&setup, &state, "update", &input, "done", false, "Updated.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("update").await;
    let asked = h.wait_for(|e| matches!(e, Event::ApprovalRequested { .. })).await;
    let Event::ApprovalRequested { call_id, interactive, exit: Some(exit), .. } = asked else {
        panic!("the question shows the exit: {asked:?}");
    };
    assert!(interactive, "sudo may ask for a password");
    assert_eq!(exit.kinds, vec![ExitKind::Privilege]);
    assert_eq!(exit.launch, Launch::Unsandboxed);
    assert!(exit.grants.is_empty());
    assert!(exit.user_only);
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(sent.turn_id).await;

    let ran = h.toolbox.ran();
    assert_eq!(ran[0].launch, Launch::Unsandboxed);
    assert!(ran[0].approved_interactive, "the exit child keeps the input relay");
    assert_eq!(started_launch(&h).await, Some(Launch::Unsandboxed));
    h.finish();
}

#[tokio::test]
async fn a_denied_exit_never_runs_and_the_model_hears_it() {
    let (setup, state) = auto("push");
    let input = json!({ "command": "git push" });
    let records = one_call(&setup, &state, "push", &input, EXIT_DENIED, true, "Not pushed.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("push").await;
    let call_id = h.wait_approval().await;
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(sent.turn_id).await;

    assert!(h.toolbox.invoked().is_empty());
    assert_eq!(judgements(&h).await, vec![(JudgeKind::User, Verdict::Deny)]);
    h.finish();
}

#[tokio::test]
async fn multi_command_unsandboxed_exit_refused_without_question() {
    let (setup, state) = auto("set up");
    let line = "sudo -v && ./helper";
    let problem = efr_permissions::exits::unsandboxed_line_problem(line).expect("a problem");
    assert!(problem.starts_with(efr_permissions::exits::ONE_COMMAND), "{problem}");
    let input = json!({ "command": line });
    let records = one_call(&setup, &state, "set up", &input, &problem, true, "Split it.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("set up").await;
    h.wait_end(sent.turn_id).await;

    assert!(h.toolbox.invoked().is_empty());
    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })));
    assert!(judgements(&h).await.is_empty(), "it is not a refusal of an exit");
    assert_eq!(started_launch(&h).await, None);
    h.finish();
}

#[tokio::test]
async fn a_sudo_that_forgets_the_password_asks_once_with_the_sudo_beside_it() {
    let (setup, state) = auto("check sudo");
    let line = "sudo -k; sudo true";
    assert_eq!(efr_permissions::exits::unsandboxed_line_problem(line), None);
    let input = json!({ "command": line });
    let records = one_call(&setup, &state, "check sudo", &input, "done", false, "Checked.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("check sudo").await;
    let asked = h.wait_for(|e| matches!(e, Event::ApprovalRequested { .. })).await;
    let Event::ApprovalRequested { call_id, exit: Some(exit), .. } = asked else {
        panic!("the question shows the exit: {asked:?}");
    };
    assert_eq!(exit.kinds, vec![ExitKind::Privilege]);
    assert_eq!(exit.launch, Launch::Unsandboxed, "it runs with full rights");
    assert!(exit.user_only);
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(sent.turn_id).await;

    let events = h.events().await;
    let questions = events.iter().filter(|e| matches!(e, Event::ApprovalRequested { .. })).count();
    assert_eq!(questions, 1, "one question for the line");
    let ran = h.toolbox.ran();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].launch, Launch::Unsandboxed);
    h.finish();
}

#[tokio::test]
async fn a_write_beside_a_sudo_is_still_refused_without_question() {
    let (setup, state) = auto("set up");
    let line = "rm x; sudo true";
    let problem = efr_permissions::exits::unsandboxed_line_problem(line).expect("a problem");
    assert!(problem.starts_with(efr_permissions::exits::ONE_COMMAND), "{problem}");
    let input = json!({ "command": line });
    let records = one_call(&setup, &state, "set up", &input, &problem, true, "Split it.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("set up").await;
    h.wait_end(sent.turn_id).await;

    assert!(h.toolbox.invoked().is_empty());
    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })));
    h.finish();
}

#[tokio::test]
async fn user_ask_rule_launch_is_contained() {
    let (mut setup, state) = auto("build");
    let rule =
        Rule::new(Action::Execute, Resource::Command(CommandPattern::new("make")), Effect::Ask);
    setup.config.policy = Policy::new(vec![rule]).expect("policy");
    let input = json!({ "command": "make" });
    let records = one_call(&setup, &state, "build", &input, "done", false, "Built.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("build").await;
    let asked = h.wait_for(|e| matches!(e, Event::ApprovalRequested { .. })).await;
    let Event::ApprovalRequested { call_id, exit, .. } = asked else {
        unreachable!("wait_for returns an event it accepted");
    };
    assert_eq!(exit, None, "a rule's question is no exit");
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(sent.turn_id).await;

    assert_eq!(h.toolbox.ran()[0].launch, Launch::contained());
    assert!(judgements(&h).await.is_empty());
    h.finish();
}

/// The decision about a shell call of `line` that reads `read`, in a turn of `setup` in
/// `auto`, as the check point reaches it.
fn denial(setup: &Setup, title: &str, line: &str, read: &Path) -> String {
    let engine = Engine::with_defaults(Locations::new(setup.home()).unwrap());
    let decision = engine.decide(&DecisionInput {
        requirements: Requirements::none().with_command(line).with_read(read),
        scope: Scope::Machine,
        origin: Origin::Shell,
        mode: Mode::Auto,
        conversation_policy: ConversationPolicy::new(setup.scratch(title)),
    });
    assert_eq!(decision.effect(), Effect::Deny);
    approvals::denial("shell", &decision)
}

#[tokio::test]
async fn floor_refusals_stop_turn_at_three() {
    let (setup, state) = auto("find the key");
    let key = setup.home().join(".ssh/id_ed25519");
    let line = format!("cat {}", key.display());
    let input = json!({ "command": line, "reads": [key] });
    let refused = denial(&setup, "find the key", &line, &key);
    let first = setup.prompt(&state, "find the key");
    let mut messages = vec![first];
    let mut records = Vec::new();
    for n in 1..=3 {
        let call = format!("call_{n}");
        records.push(expect_request(request(messages.clone())));
        records.push(answer(&tool_answer(&call, "shell", &input)));
        messages.push(tool_message(&call, "shell", &input));
        messages.push(result_message(&call, &refused, true));
    }
    let mut h = setup.start(records).await;

    let sent = h.prompt("find the key").await;
    let end = h.wait_end(sent.turn_id).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn stops: {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(error.message, REFUSALS_STOPPED);
    // The text of efr's auto spec, section 14.7.
    assert_eq!(
        error.message,
        "auto stopped this turn: 3 actions were refused in a row. Read the answers, then \
         send a new prompt."
    );
    assert!(h.toolbox.invoked().is_empty());
    assert_eq!(judgements(&h).await, vec![(JudgeKind::Floor, Verdict::Deny); 3]);
    let events = h.events().await;
    // The person who follows the turn reads why, in a few words.
    let refusals: Vec<Option<String>> = events
        .iter()
        .filter_map(|e| match e {
            Event::ToolCallCompleted { refusal, .. } => Some(refusal.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(refusals, vec![Some("a secret (floor)".to_owned()); 3]);
    let Event::ExitRequested { record, .. } = events
        .iter()
        .rev()
        .find(|e| matches!(e, Event::ExitRequested { .. }))
        .cloned()
        .expect("each refusal has its record")
    else {
        unreachable!("the find accepts only exit_requested");
    };
    assert_eq!(record.facts.refusals_in_a_row, 2, "the record shows the refusals before it");
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })));
    h.finish();
}

#[tokio::test]
async fn a_user_answer_resets_the_refusals() {
    let (setup, state) = auto("look around");
    let key = setup.home().join(".ssh/id_ed25519");
    let secret = format!("cat {}", key.display());
    let refused_input = json!({ "command": secret, "reads": [key] });
    let refused = denial(&setup, "look around", &secret, &key);
    let push = json!({ "command": "git push" });
    let first = setup.prompt(&state, "look around");
    // Two floors, an answered exit, two floors: never three in a row.
    let inputs = [&refused_input, &refused_input, &push, &refused_input, &refused_input];
    let mut messages = vec![first];
    let mut records = Vec::new();
    for (n, input) in inputs.iter().enumerate() {
        let call = format!("call_{n}");
        records.push(expect_request(request(messages.clone())));
        records.push(answer(&tool_answer(&call, "shell", input)));
        messages.push(tool_message(&call, "shell", input));
        let output = if **input == push { EXIT_DENIED } else { refused.as_str() };
        messages.push(result_message(&call, output, true));
    }
    records.push(expect_request(request(messages)));
    records.push(answer(&text_answer("Done looking.")));
    let mut h = setup.start(records).await;

    let sent = h.prompt("look around").await;
    let call_id = h.wait_approval().await;
    h.answer(call_id, ApprovalDecision::Deny).await;
    let end = h.wait_end(sent.turn_id).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    h.finish();
}

#[tokio::test]
async fn nested_shell_is_denied_in_auto_without_a_refusal_count() {
    let (setup, state) = auto("root shell");
    let input = json!({ "command": "id", "nested_shell": true });
    let first = setup.prompt(&state, "root shell");
    let mut h_records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "shell", &input)),
    ];
    let engine = Engine::with_defaults(Locations::new(setup.home()).unwrap());
    let decision = engine.decide(&DecisionInput {
        requirements: Requirements::none().with_command("id").with_nested(),
        scope: Scope::Machine,
        origin: Origin::Shell,
        mode: Mode::Auto,
        conversation_policy: ConversationPolicy::new(setup.scratch("root shell")),
    });
    let refused = approvals::denial("shell", &decision);
    assert!(refused.contains("nested_shell is not available in auto"), "{refused}");
    h_records.push(expect_request(request(vec![
        first,
        tool_message("call_1", "shell", &input),
        result_message("call_1", &refused, true),
    ])));
    h_records.push(answer(&text_answer("No nested shell.")));
    let mut h = setup.start(h_records).await;

    let sent = h.prompt("root shell").await;
    h.wait_end(sent.turn_id).await;

    assert!(h.toolbox.invoked().is_empty());
    assert!(judgements(&h).await.is_empty());
    h.finish();
}

#[tokio::test]
async fn auto_falls_back_to_cautious_with_reason() {
    let (mut setup, _) = auto("hello");
    setup.sandbox = SandboxStatus::unavailable("bubblewrap is not installed");
    let mut state = setup.live_state(&setup.cwd, "hello");
    let fallback =
        ModeFallback { asked: Mode::Auto, reason: "bubblewrap is not installed".to_owned() };
    state.fallback = Some(fallback.clone());
    let input = json!({ "command": "rm build.log" });
    let records = one_call(&setup, &state, "hello", &input, "done", false, "Removed.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("hello").await;
    let call_id = h.wait_approval().await;
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(sent.turn_id).await;

    let answered = sent.settings.expect("the prompt's settings");
    assert_eq!(
        (answered.mode, answered.fallback.clone()),
        (Mode::Cautious, Some(fallback.clone()))
    );
    let events = h.events().await;
    let started = find(&events, |e| matches!(e, Event::TurnStarted { .. }));
    let Event::TurnStarted { settings: Some(settings), .. } = started else {
        panic!("the turn records its settings: {started:?}");
    };
    assert_eq!((settings.mode, settings.fallback), (Mode::Cautious, Some(fallback)));
    // As cautious, the writer asks, and the call runs typed into the hidden shell.
    assert_eq!(h.toolbox.ran()[0].launch, Launch::Direct);
    assert!(!h.toolbox.judged()[0].auto, "a turn that fell back collects no auto facts");
    h.finish();
}

#[tokio::test]
async fn a_project_at_home_falls_back_to_cautious() {
    let (mut setup, _) = auto("hello");
    let id: ProjectId = "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap();
    let home = setup.home().to_path_buf();
    setup.project = Some((id, home));
    let derivation = Derivation { scope: Scope::Project(id), basis: Basis::Registered, repo: None };
    setup.scope = std::mem::take(&mut setup.scope).with(setup.cwd.clone(), derivation);
    let mut state = setup.live_state(&setup.cwd, "hello");
    let fallback =
        ModeFallback { asked: Mode::Auto, reason: ModeFallback::HOME_PROJECT_REASON.to_owned() };
    state.fallback = Some(fallback.clone());
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "hello")])),
        answer(&text_answer("Hi.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("hello").await;
    h.wait_end(sent.turn_id).await;

    // The prompt does not know its project yet; the turn does.
    assert_eq!(sent.settings.map(|settings| settings.mode), Some(Mode::Auto));
    let events = h.events().await;
    let started = find(&events, |e| matches!(e, Event::TurnStarted { .. }));
    let Event::TurnStarted { settings: Some(settings), .. } = started else {
        panic!("the turn records its settings: {started:?}");
    };
    assert_eq!((settings.mode, settings.fallback), (Mode::Cautious, Some(fallback)));
    h.finish();
}

/// The note that the model reads after `plant-hook` when the user `keep`s the change.
fn kept_note() -> String {
    format!(
        "done\n[The user kept the git change; it moved back: {} (core.fsmonitor).]",
        planted().path.display()
    )
}

#[tokio::test]
async fn surface_question_before_next_call() {
    let (setup, state) = auto("plant");
    let plant = json!({ "command": "plant-hook" });
    let test = json!({ "command": "cargo test" });
    let first = setup.prompt(&state, "plant");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&tool_answer("call_1", "shell", &plant)),
        expect_request(request(vec![
            first.clone(),
            tool_message("call_1", "shell", &plant),
            result_message("call_1", &kept_note(), false),
        ])),
        answer(&tool_answer("call_2", "shell", &test)),
        expect_request(request(vec![
            first,
            tool_message("call_1", "shell", &plant),
            result_message("call_1", &kept_note(), false),
            tool_message("call_2", "shell", &test),
            result_message("call_2", "done", false),
        ])),
        answer(&text_answer("Tested.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("plant").await;
    let question_id = h.wait_question().await;
    assert_eq!(h.toolbox.invoked().len(), 1, "the next call waits for the answer");
    assert_eq!(h.handle.state().await.unwrap().pending_questions, vec![question_id]);
    h.keep(question_id, true).await;
    h.wait_end(sent.turn_id).await;

    assert_eq!(h.toolbox.restored(), vec![planted()]);
    let kinds = h.kinds().await;
    let at = |kind: &str| kinds.iter().position(|k| k == kind).unwrap();
    assert!(at("tool_call_completed") < at("sandbox_surface_changed"));
    assert!(at("sandbox_surface_changed") < at("surface_question_requested"));
    assert!(
        at("surface_question_answered")
            < kinds.iter().rposition(|k| k == "tool_call_started").unwrap()
    );
    let events = h.events().await;
    assert!(matches!(
        find(&events, |e| matches!(e, Event::SurfaceQuestionAnswered { .. })),
        Event::SurfaceQuestionAnswered { keep: true, origin: Some(Origin::Shell), .. }
    ));
    assert!(matches!(
        find(&events, |e| matches!(e, Event::SandboxSurfaceChanged { .. })),
        Event::SandboxSurfaceChanged { quarantined: true, .. }
    ));
    h.finish();
}

#[tokio::test]
async fn surface_question_expiry_keeps_quarantine() {
    let (mut setup, state) = auto("plant");
    setup.config.approval_timeout = Some(Duration::from_secs(60));
    let plant = json!({ "command": "plant-hook" });
    let note = format!(
        "done\n[Nobody answered; the git change stayed in quarantine: {} (core.fsmonitor).]",
        planted().path.display()
    );
    let records = one_call(&setup, &state, "plant", &plant, &note, false, "Left it.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("plant").await;
    let question_id = h.wait_question().await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(60));
    h.wait_end(sent.turn_id).await;

    assert!(h.toolbox.restored().is_empty());
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::SurfaceQuestionAnswered { .. })),
        Event::SurfaceQuestionAnswered {
            turn_id: sent.turn_id,
            question_id,
            keep: false,
            origin: None,
        }
    );
    let late = SandboxSurfaceRespond {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        question_id,
        keep: true,
    };
    let refused = h.handle.respond_surface(late, Origin::Shell).await;
    assert!(
        matches!(refused, Err(ConversationError::QuestionNotPending { question_id: id }) if id == question_id),
        "{refused:?}"
    );
    h.finish();
}

#[tokio::test]
async fn a_no_keeps_the_change_in_quarantine_and_a_phone_cannot_answer() {
    let (setup, state) = auto("plant");
    let plant = json!({ "command": "plant-hook" });
    let note = format!(
        "done\n[The user did not keep the git change; it stays in quarantine: {} \
         (core.fsmonitor).]",
        planted().path.display()
    );
    let records = one_call(&setup, &state, "plant", &plant, &note, false, "Left it.");
    let mut h = setup.start(records).await;

    let sent = h.prompt("plant").await;
    let question_id = h.wait_question().await;
    let phone = SandboxSurfaceRespond {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        question_id,
        keep: true,
    };
    let refused = h.handle.respond_surface(phone, Origin::Phone).await;
    assert!(matches!(
        refused,
        Err(ConversationError::RemoteSurfaceAnswer { origin: Origin::Phone })
    ));
    h.keep(question_id, false).await;
    h.wait_end(sent.turn_id).await;

    assert!(h.toolbox.restored().is_empty());
    h.finish();
}

#[tokio::test]
async fn an_interrupt_during_the_surface_question_keeps_the_quarantine() {
    let (setup, state) = auto("plant");
    let plant = json!({ "command": "plant-hook" });
    let first = setup.prompt(&state, "plant");
    let records =
        vec![expect_request(request(vec![first])), answer(&tool_answer("call_1", "shell", &plant))];
    let mut h = setup.start(records).await;

    let sent = h.prompt("plant").await;
    let question_id = h.wait_question().await;
    let interrupt = efr_protocol::TurnInterrupt {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        turn_id: Some(sent.turn_id),
    };
    h.handle.interrupt(interrupt, Origin::Shell).await.unwrap();
    let end = h.wait_end(sent.turn_id).await;

    assert!(matches!(end, Event::TurnInterrupted { .. }), "{end:?}");
    assert!(h.toolbox.restored().is_empty());
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::SurfaceQuestionAnswered { .. })),
        Event::SurfaceQuestionAnswered {
            turn_id: sent.turn_id,
            question_id,
            keep: false,
            origin: None,
        }
    );
    h.finish();
}

#[tokio::test]
async fn the_turn_report_comes_before_the_end_of_the_turn() {
    let (setup, state) = auto("build it");
    let input = json!({ "command": "cargo build" });
    let records = one_call(&setup, &state, "build it", &input, "done", false, "Built.");
    let mut h = setup.start(records).await;
    let file = efr_protocol::ReportedFile { path: "build.rs".into(), detail: None };
    h.toolbox.report.lock().unwrap().push(file.clone());

    let sent = h.prompt("build it").await;
    h.wait_end(sent.turn_id).await;

    let events = h.events().await;
    let at = |wanted: fn(&Event) -> bool| events.iter().position(wanted).unwrap();
    let report = at(|e| matches!(e, Event::TurnSurfaceReport { .. }));
    let completed = at(|e| matches!(e, Event::TurnCompleted { .. }));
    assert_eq!(report + 1, completed);
    assert!(matches!(
        &events[report],
        Event::TurnSurfaceReport { turn_id, files } if *turn_id == sent.turn_id && files == &[file]
    ));
    h.finish();
}
