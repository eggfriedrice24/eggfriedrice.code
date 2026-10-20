//! Full mode in a turn: calls run as the user with no sandbox and no question, the exits
//! that full mode lets through are recorded with the judge `full`, nobody reviews them,
//! the gate holds before each call and at the start of a queued prompt or steer.
//!
//! NOTE: the fake toolbox runs no command: each line here is only read by the engine.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use efr_permissions::{CallFacts, Requirements, TargetKind};
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, CallId, ErrorCode, Event, ExitKind, FullModeStatus,
    FullRefusal, JudgeKind, Launch, Mode, Origin, SandboxStatus, TurnId, TurnSettings, Verdict,
};
use pretty_assertions::assert_eq;
use serde_json::Value;

use super::review::{count, judgements, reviewed, shell};
use crate::ConversationError;
use crate::exit;
use crate::presence::PRESENCE_CHECK;
use crate::testing::{DONE, FakeAllowWriter, FakeJudge, Harness, Script, Setup, allows};
use crate::turn::full::FULL_TURNED_OFF;

/// The reviewed setup (the registered project at the working directory, `~/lib`, the
/// caches) whose turns run in full mode, which the daemon allows, with `judge` before
/// the user, which full mode must never call.
fn full(judge: Option<Arc<FakeJudge>>) -> Setup {
    let mut setup = reviewed(judge);
    setup.config.mode = Mode::Full;
    setup.config.full_mode = FullModeStatus { allowed: true, refused: None };
    setup
}

/// A copy to the new file `~/notes/<name>` in the existing dir `~/notes`, with the facts
/// that efrd gives: a `write` exit outside the project.
fn notes_copy(home: &Path, name: &str) -> (String, Requirements) {
    let notes = home.join("notes");
    let file = notes.join(name);
    let line = format!("cp a.txt {}", file.display());
    let facts = CallFacts {
        targets: vec![(file.clone(), None), (notes.clone(), Some(TargetKind::Dir))],
        real_paths: vec![(file.clone(), file.clone()), (notes.clone(), notes)],
        ..CallFacts::default()
    };
    let requirements =
        Requirements::none().with_command(line.clone()).with_write(file).with_facts(facts);
    (line, requirements)
}

/// Starts `setup` with a model that makes `calls`, with `declared` as the requirements
/// of each line, and sends `said`.
async fn start(
    setup: Setup,
    calls: Vec<(&str, Value)>,
    declared: Vec<(String, Requirements)>,
    said: &str,
) -> (Harness, Arc<Script>, TurnId) {
    let script = Script::new(calls);
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    for (line, requirements) in declared {
        h.toolbox.declared.lock().unwrap().insert(line, requirements);
    }
    let sent = h.prompt(said).await;
    (h, script, sent.turn_id)
}

/// The settings that the turn `turn_id` started with, as (mode, fallback).
async fn started(h: &Harness, turn_id: TurnId) -> (Mode, Option<efr_protocol::ModeFallback>) {
    h.events()
        .await
        .into_iter()
        .find_map(|event| match event {
            Event::TurnStarted { turn_id: id, settings: Some(settings), .. } if id == turn_id => {
                Some((settings.mode, settings.fallback))
            }
            _ => None,
        })
        .expect("the turn started with its settings")
}

/// Sends new settings with `full_mode`, as efrd does after a reload.
fn set_full_mode(h: &Harness, full_mode: FullModeStatus) {
    let mut config = (**h.settings.borrow()).clone();
    config.full_mode = full_mode;
    h.settings.send_replace(Arc::new(config));
}

#[tokio::test]
async fn full_needs_no_sandbox() {
    let mut setup = full(None);
    setup.sandbox = SandboxStatus::unavailable("bubblewrap is not installed");
    let (mut h, _, turn_id) = start(setup, vec![shell("cargo build")], Vec::new(), "build").await;
    let end = h.wait_end(turn_id).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert_eq!(started(&h, turn_id).await, (Mode::Full, None), "no fallback");
    assert_eq!(h.toolbox.ran().len(), 1);
    assert_eq!(h.toolbox.ran()[0].launch, Launch::Direct);
}

#[tokio::test]
async fn full_turn_at_home_project_stays_full() {
    let mut setup = full(None);
    let home = setup.home().to_path_buf();
    let project: efr_protocol::ProjectId = "0192f0c1-7a00-7000-8000-000000000009".parse().unwrap();
    let derivation = efr_scope::Derivation {
        scope: efr_protocol::Scope::Project(project),
        basis: efr_scope::Basis::Registered,
        repo: None,
    };
    setup.cwd = home.clone();
    setup.scope = std::mem::take(&mut setup.scope).with(home.clone(), derivation);
    let locations = efr_permissions::Locations::new(&home).unwrap().with_project(project, &home);
    setup.engine = Some(efr_permissions::Engine::with_defaults(locations.unwrap()));
    let (mut h, _, turn_id) = start(setup, vec![shell("ls")], Vec::new(), "look").await;
    h.wait_end(turn_id).await;

    // `auto` would fall back here: a project at home is never a write root.
    assert_eq!(started(&h, turn_id).await, (Mode::Full, None));
    assert_eq!(h.toolbox.ran().len(), 1);
}

#[tokio::test]
async fn full_shell_call_runs_direct() {
    let (mut h, _, turn_id) =
        start(full(None), vec![shell("cargo test")], Vec::new(), "run the tests").await;
    h.wait_end(turn_id).await;

    assert_eq!(count(&h, "approval_requested").await, 0);
    let ran = h.toolbox.ran();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].launch, Launch::Direct);
    assert_eq!(ran[0].mode, Mode::Full);
    assert!(ran[0].is_full() && ran[0].predicts() && !ran[0].sandboxed());
    let started = h.events().await.into_iter().find_map(|event| match event {
        Event::ToolCallStarted { launch, .. } => Some(launch),
        _ => None,
    });
    assert_eq!(started, Some(None), "a direct call names no launch");
}

#[tokio::test]
async fn full_records_exit_judged_full_and_asks_no_reviewer() {
    let judge = FakeJudge::answering(vec![allows()]);
    let setup = full(Some(Arc::clone(&judge)));
    let declared = notes_copy(setup.home(), "today.txt");
    let calls = vec![shell(&declared.0)];
    let (mut h, _, turn_id) = start(setup, calls, vec![declared], "copy it to my notes").await;
    h.wait_end(turn_id).await;

    assert_eq!(judge.calls(), 0, "nobody reviews a full call");
    assert_eq!(count(&h, "approval_requested").await, 0);
    assert_eq!(judgements(&h).await, vec![(JudgeKind::Full, Verdict::Allow)]);
    let kinds: Vec<String> = h.kinds().await;
    let at = |kind: &str| kinds.iter().position(|known| known == kind).unwrap();
    assert!(at("exit_requested") < at("exit_judged"));
    assert!(at("exit_judged") < at("tool_call_completed"), "recorded before the call ran");
    let Event::ExitRequested { kinds, grants, record, .. } =
        h.events().await.into_iter().find(|e| matches!(e, Event::ExitRequested { .. })).unwrap()
    else {
        unreachable!("found an exit_requested");
    };
    assert_eq!(kinds, vec![ExitKind::Write]);
    assert!(grants.is_empty(), "no sandbox, no grants");
    assert_eq!(record.user_messages, vec!["copy it to my notes"]);
    let ran = h.toolbox.ran();
    assert_eq!(ran[0].launch, Launch::Direct);
    assert_eq!(
        ran[0].exits.iter().map(|need| need.kind).collect::<Vec<_>>(),
        vec![ExitKind::Write],
        "the toolbox gets the exits, so efrd copies the file first"
    );
}

#[tokio::test]
async fn auto_turn_still_reviews_and_offers_always() {
    // The guard of the split of `CallContext`: `auto` keeps its reviewer and its offer.
    let judge = FakeJudge::answering(vec![crate::testing::unanswered()]);
    let mut setup = reviewed(Some(Arc::clone(&judge)));
    setup.allow_writer = Some(FakeAllowWriter::saving());
    let declared = notes_copy(setup.home(), "today.txt");
    let calls = vec![shell(&declared.0)];
    let (mut h, _, turn_id) = start(setup, calls, vec![declared], "copy it to my notes").await;
    let asked = h.wait_for(|e| matches!(e, Event::ApprovalRequested { .. })).await;
    let Event::ApprovalRequested { call_id, exit: Some(exit), .. } = asked else {
        panic!("the question shows the exit: {asked:?}");
    };
    assert!(exit.always.is_some(), "auto offers a rule: {exit:?}");
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(turn_id).await;

    assert_eq!(judge.calls(), 1, "auto asks its reviewer first");
    assert!(h.toolbox.judged()[0].sandboxed());
}

#[tokio::test]
async fn a_full_prompt_fails_at_start_when_the_gate_went_off() {
    let calls = vec![shell("sudo true"), (DONE, Value::Null)];
    let (mut h, _, first) = start(full(None), calls, Vec::new(), "update").await;
    let call_id = h.wait_approval().await;
    // A second prompt queues while full mode is on.
    let queued = h.prompt("and then clean up").await;
    set_full_mode(&h, FullModeStatus { allowed: false, refused: None });
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(first).await;
    let end = h.wait_end(queued.turn_id).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the queued full prompt fails at its start: {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Invalid);
    assert_eq!(error.message, ConversationError::FullModeOff.to_string());
    assert!(h.toolbox.invoked().is_empty());
}

/// Runs a full turn whose first call (`sudo true`) asks; while it waits, efrd sends
/// `full_mode`; the user says yes. Returns the harness, the model and the terminal
/// event.
async fn gate_changes_mid_turn(full_mode: FullModeStatus) -> (Harness, Arc<Script>, Event) {
    let calls = vec![shell("sudo true"), shell("cargo build")];
    let (mut h, script, turn_id) = start(full(None), calls, Vec::new(), "update").await;
    let call_id = h.wait_approval().await;
    set_full_mode(&h, full_mode);
    h.answer(call_id, ApprovalDecision::Allow).await;
    let end = h.wait_end(turn_id).await;
    (h, script, end)
}

#[tokio::test]
async fn a_running_full_turn_stops_when_the_gate_goes_off() {
    let (h, _, end) = gate_changes_mid_turn(FullModeStatus { allowed: false, refused: None }).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn stops: {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(error.message, FULL_TURNED_OFF);
    assert_eq!(
        FULL_TURNED_OFF,
        "full mode was turned off while this turn ran; send the prompt again"
    );
    let invoked: Vec<String> =
        h.toolbox.invoked().iter().map(|(_, i)| i["command"].to_string()).collect();
    assert_eq!(invoked, vec!["\"sudo true\""], "the answered call ran; the next did not");
    assert_eq!(h.toolbox.judged().len(), 1, "the next call was never judged");
    let outputs: Vec<(String, Option<String>)> = h
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::ToolCallCompleted { output, refusal, .. } => Some((output, refusal)),
            _ => None,
        })
        .collect();
    assert_eq!(
        outputs.last(),
        Some(&(
            FULL_TURNED_OFF.to_owned(),
            Some("full mode was turned off while this turn ran".to_owned())
        )),
        "the call that did not run says why"
    );
}

#[tokio::test]
async fn a_running_full_turn_stops_when_undo_goes_off() {
    let refused = FullModeStatus { allowed: true, refused: Some(FullRefusal::UndoOff) };
    let (h, _, end) = gate_changes_mid_turn(refused).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn stops: {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(error.message, format!("full mode cannot run: {}", FullRefusal::UndoOff.describe()));
    assert_eq!(h.toolbox.invoked().len(), 1);
}

/// A late steer of `text` that queues as a prompt with `mode`, from `origin`.
async fn steer_as_prompt(
    h: &mut Harness,
    mode: Mode,
    origin: Origin,
) -> Result<efr_protocol::TurnSteerResult, ConversationError> {
    let mut params = h.steer_params(None, "clean up the build", true);
    if let Some(efr_protocol::LateSteer::Queue { settings, .. }) = params.if_late.as_mut() {
        *settings = TurnSettings { mode: Some(mode), ..TurnSettings::default() };
    }
    h.handle.steer(params, origin).await
}

#[tokio::test]
async fn a_phone_steer_with_mode_full_runs_as_cautious() {
    let mut setup = full(None);
    setup.config.mode = Mode::Cautious;
    let script = Script::new(Vec::new());
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let queued = steer_as_prompt(&mut h, Mode::Full, Origin::Phone).await.unwrap();
    assert!(queued.queued, "no turn runs, so the steer queues as a prompt");
    h.wait_end(queued.turn_id).await;
    assert_eq!(started(&h, queued.turn_id).await, (Mode::Cautious, None));
}

#[tokio::test]
async fn a_local_steer_with_mode_full_obeys_the_gate() {
    let mut setup = Setup::new();
    assert!(!setup.config.full_mode.allowed, "full mode is off by default");
    setup.config.mode = Mode::Cautious;
    let script = Script::new(Vec::new());
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let refused = steer_as_prompt(&mut h, Mode::Full, Origin::Shell).await;
    assert!(matches!(refused, Err(ConversationError::FullModeOff)), "{refused:?}");
    assert_eq!(h.handle.state().await.unwrap().running, None, "nothing queued");

    set_full_mode(&h, FullModeStatus { allowed: true, refused: None });
    let queued = steer_as_prompt(&mut h, Mode::Full, Origin::Shell).await.unwrap();
    h.wait_end(queued.turn_id).await;
    assert_eq!(started(&h, queued.turn_id).await, (Mode::Full, None));
}

/// A guarded delete of the home directory. It is a dead branch, as every delete of a
/// test is; the engine still reads it, and the fake toolbox runs no line.
const DELETE_HOME: &str = "false && rm -rf ~";

/// What a question about the delete of the home directory names.
const HOME_TARGET: &str = "~ (your home directory)";

/// The next question of the log: its call, its summary and its fixed limit.
async fn next_question(h: &mut Harness) -> (CallId, String, Option<u32>) {
    let asked = h.wait_for(|e| matches!(e, Event::ApprovalRequested { .. })).await;
    let Event::ApprovalRequested { call_id, summary, expires_in_secs, .. } = asked else {
        unreachable!("wait_for returns an event it accepted");
    };
    (call_id, summary, expires_in_secs)
}

/// Lets the question `call_id`, which waits on the clock, expire after `CRITICAL_WAIT`.
async fn let_expire(h: &mut Harness, call_id: CallId) {
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(exit::CRITICAL_WAIT);
    h.wait_for(|e| matches!(e, Event::ApprovalExpired { call_id: id, .. } if *id == call_id)).await;
}

#[tokio::test]
async fn full_critical_question_expires_after_120s() {
    let calls = vec![shell(DELETE_HOME)];
    let (mut h, script, turn_id) = start(full(None), calls, Vec::new(), "clean up").await;
    let (call_id, _, expires) = next_question(&mut h).await;
    assert_eq!(expires, Some(120));
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(119));
    tokio::task::yield_now().await;
    assert!(!h.has_ended(turn_id), "it still waits at 119 seconds");
    h.clock.advance(Duration::from_secs(1));
    let end = h.wait_end(turn_id).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "the model goes on: {end:?}");
    assert!(h.toolbox.invoked().is_empty(), "the delete did not run");
    let events = h.events().await;
    assert!(events.contains(&Event::ApprovalExpired { turn_id, call_id, away: false }));
    let expected = exit::critical_expired(HOME_TARGET, Some(Duration::from_secs(120)));
    assert_eq!(script.results(), vec![(expected.clone(), true)]);
    assert_eq!(
        expected,
        "efr did not run this delete of ~ (your home directory): the user did not answer in \
         120 seconds. Do not try it another way. Delete narrower paths that you can name, or \
         ask the user to run it."
    );
}

#[tokio::test]
async fn full_critical_question_denied_at_once_when_away() {
    let setup = full(None);
    setup.presence.set_away(true);
    let (mut h, script, turn_id) =
        start(setup, vec![shell(DELETE_HOME)], Vec::new(), "clean up").await;
    let (call_id, _, expires) = next_question(&mut h).await;
    // No clock moves: nobody is there, so the question does not wait.
    h.wait_end(turn_id).await;

    assert_eq!(expires, None, "a question that does not wait names no limit");
    assert!(h.toolbox.invoked().is_empty());
    assert!(h.events().await.contains(&Event::ApprovalExpired { turn_id, call_id, away: true }));
    assert!(h.presence.pushed().is_empty(), "no phone is woken for a question that is gone");
    assert_eq!(script.results(), vec![(exit::critical_expired(HOME_TARGET, None), true)]);
}

#[tokio::test]
async fn full_three_expired_deletes_deny_at_once() {
    let setup = full(None);
    setup.presence.set_away(true);
    let calls = vec![shell(DELETE_HOME); 4];
    let (mut h, script, turn_id) = start(setup, calls, Vec::new(), "clean up").await;
    let end = h.wait_end(turn_id).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert_eq!(count(&h, "approval_requested").await, 3, "the fourth delete asks no more");
    let results = script.results();
    assert_eq!(results.len(), 4);
    let refused = exit::critical_refused(HOME_TARGET);
    assert_eq!(results[3], (refused.clone(), true));
    assert_eq!(
        refused,
        "efr refuses deletes of ~ (your home directory) for the rest of this turn. Do not try \
         it another way. Delete narrower paths that you can name, or ask the user to run it."
    );
    assert!(h.toolbox.invoked().is_empty());
}

#[tokio::test]
async fn full_answer_does_not_reset_the_expiry_count() {
    let calls = vec![shell(DELETE_HOME); 5];
    let (mut h, script, turn_id) = start(full(None), calls, Vec::new(), "clean up").await;
    let (first, _, _) = next_question(&mut h).await;
    let_expire(&mut h, first).await;
    let (second, _, _) = next_question(&mut h).await;
    h.answer(second, ApprovalDecision::Deny).await;
    for _ in 0..2 {
        let (call_id, _, _) = next_question(&mut h).await;
        let_expire(&mut h, call_id).await;
    }
    h.wait_end(turn_id).await;

    assert_eq!(count(&h, "approval_requested").await, 4, "the fifth delete asks no more");
    assert_eq!(script.results().last().unwrap().0, exit::critical_refused(HOME_TARGET));
}

#[tokio::test]
async fn full_fourth_refusal_stops_turn_names_mode() {
    // Three questions expire; each later guarded delete is refused with no question, and
    // three such refusals in a row stop the turn, with a text that names full mode.
    let setup = full(None);
    setup.presence.set_away(true);
    let calls = vec![shell(DELETE_HOME); 7];
    let (mut h, script, turn_id) = start(setup, calls, Vec::new(), "clean up").await;
    let end = h.wait_end(turn_id).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("three refusals in a row stop the turn: {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(error.message, exit::refusals_stopped(Mode::Full));
    assert!(error.message.starts_with("full mode stopped this turn"), "{}", error.message);
    assert_eq!(count(&h, "approval_requested").await, 3);
    assert_eq!(count(&h, "tool_call_started").await, 6, "the seventh call never came");
    assert_eq!(script.requests().len(), 6);
}

/// Answers `call_id` with `decision` from `origin`.
async fn answer_from(
    h: &mut Harness,
    call_id: CallId,
    decision: ApprovalDecision,
    origin: Origin,
) -> Result<efr_protocol::ApprovalRespondResult, ConversationError> {
    let params = ApprovalRespond {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        call_id,
        decision,
    };
    h.handle.respond_approval(params, origin).await
}

#[tokio::test]
async fn full_privilege_answer_from_phone_refused() {
    let (mut h, _, turn_id) =
        start(full(None), vec![shell("sudo true")], Vec::new(), "update").await;
    let (call_id, summary, expires) = next_question(&mut h).await;
    assert_eq!(expires, None, "only a guarded delete has a fixed limit");
    assert_eq!(
        summary.lines().next(),
        Some(
            "full mode still asks for this: more rights, such as sudo; a yes runs every \
             command of this line"
        )
    );

    let refused = answer_from(&mut h, call_id, ApprovalDecision::Allow, Origin::Phone).await;
    assert!(
        matches!(
            refused,
            Err(ConversationError::RemotePrivilegeAnswer { call_id: id, .. }) if id == call_id
        ),
        "{refused:?}"
    );
    answer_from(&mut h, call_id, ApprovalDecision::Deny, Origin::Shell).await.unwrap();
    h.wait_end(turn_id).await;
    assert!(h.toolbox.invoked().is_empty());
}

#[tokio::test]
async fn full_guard_answer_from_phone_refused() {
    let (mut h, _, turn_id) =
        start(full(None), vec![shell(DELETE_HOME)], Vec::new(), "clean up").await;
    let (call_id, _, _) = next_question(&mut h).await;

    let refused = answer_from(&mut h, call_id, ApprovalDecision::Allow, Origin::Phone).await;
    let Err(error) = refused else {
        panic!("a phone never answers a guarded delete: {refused:?}");
    };
    assert!(matches!(error, ConversationError::RemotePrivilegeAnswer { .. }), "{error:?}");
    assert!(
        error.to_string().ends_with(
            "only the user's own machine answers a privilege exit, a delete that full mode \
             guards, or a background job"
        ),
        "{error}"
    );
    assert_eq!(h.handle.state().await.unwrap().pending_approvals, vec![call_id], "still open");
    answer_from(&mut h, call_id, ApprovalDecision::Deny, Origin::Shell).await.unwrap();
    h.wait_end(turn_id).await;
    assert!(h.toolbox.invoked().is_empty());
}

/// A line that leaves a process behind the turn: a `persistence` exit, which full mode
/// still asks about. A dead branch, as every such line of a test is.
const PERSIST: &str = "false && systemd-run --user true";

#[tokio::test]
async fn full_floor_question_offers_no_always() {
    let judge = FakeJudge::answering(vec![allows()]);
    let mut setup = full(Some(Arc::clone(&judge)));
    setup.allow_writer = Some(FakeAllowWriter::saving());
    let (mut h, _, turn_id) = start(setup, vec![shell(PERSIST)], Vec::new(), "start it").await;
    let asked = h.wait_for(|e| matches!(e, Event::ApprovalRequested { .. })).await;
    let Event::ApprovalRequested { call_id, summary, exit: Some(exit), .. } = asked else {
        panic!("the question shows the exit: {asked:?}");
    };
    assert_eq!(exit.always, None, "full mode offers no always-allow rule");
    assert!(exit.user_only, "no reviewer in full mode");
    assert!(exit.kinds.contains(&ExitKind::Persistence), "{:?}", exit.kinds);
    assert_eq!(
        summary.lines().next(),
        Some("full mode still asks for this: a process that outlives the turn")
    );
    // A phone may answer it: it needs no password and no own machine.
    answer_from(&mut h, call_id, ApprovalDecision::Deny, Origin::Phone).await.unwrap();
    h.wait_end(turn_id).await;
    assert_eq!(judge.calls(), 0);
    assert_eq!(judgements(&h).await, vec![(JudgeKind::User, Verdict::Deny)]);
}

#[tokio::test]
async fn full_tree_read_with_a_secret_below_asks_and_names_it() {
    // No sandbox masks the secret in full mode, so the read of the whole dir asks.
    let setup = full(None);
    let aws = setup.home().join(".aws");
    let line = "rg TOKEN ~/.aws".to_owned();
    let declared =
        (line.clone(), Requirements::none().with_command(line.clone()).with_read_tree(aws));
    let (mut h, _, turn_id) = start(setup, vec![shell(&line)], vec![declared], "find it").await;
    let (call_id, summary, _) = next_question(&mut h).await;
    assert_eq!(
        summary.lines().next(),
        Some("full mode still asks for this: a whole dir with a secret below it")
    );
    answer_from(&mut h, call_id, ApprovalDecision::Deny, Origin::Shell).await.unwrap();
    h.wait_end(turn_id).await;
    assert!(h.toolbox.invoked().is_empty());
}

#[tokio::test]
async fn full_question_with_exits_that_expires_records_judge_full() {
    let mut setup = full(None);
    setup.config.approval_timeout = Some(Duration::from_secs(60));
    let (mut h, _, turn_id) = start(setup, vec![shell(PERSIST)], Vec::new(), "start it").await;
    let (call_id, _, expires) = next_question(&mut h).await;
    assert_eq!(expires, None);
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(60));
    h.wait_end(turn_id).await;

    assert!(h.events().await.contains(&Event::ApprovalExpired { turn_id, call_id, away: false }));
    let judged: Vec<(JudgeKind, Verdict, Option<String>)> = h
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::ExitJudged { judge, verdict, category, .. } => Some((judge, verdict, category)),
            _ => None,
        })
        .collect();
    assert_eq!(judged, vec![(JudgeKind::Full, Verdict::Deny, Some("expired".to_owned()))]);
}

#[tokio::test]
async fn full_away_wait_vs_guard_wait() {
    // The user leaves after the question parked: the guard's 120 seconds end before the
    // away wait of 300 seconds.
    let (mut h, _, turn_id) =
        start(full(None), vec![shell(DELETE_HOME)], Vec::new(), "clean up").await;
    let (call_id, _, _) = next_question(&mut h).await;
    // NOTE: a guarded delete reads the presence twice when it parks: once to know
    // whether to wait at all, once in its wait.
    h.presence.wait_for_reads(2).await;
    h.presence.set_away(true);
    h.clock.advance(PRESENCE_CHECK);
    h.presence.wait_for_reads(3).await;
    h.clock.advance(exit::CRITICAL_WAIT - PRESENCE_CHECK - Duration::from_secs(1));
    tokio::task::yield_now().await;
    assert!(!h.has_ended(turn_id), "the guard waits its 120 seconds");
    h.clock.advance(Duration::from_secs(1));
    h.wait_end(turn_id).await;
    assert!(h.events().await.contains(&Event::ApprovalExpired { turn_id, call_id, away: true }));

    // A short away wait ends before the guard's 120 seconds.
    let mut setup = full(None);
    setup.config.away_wait = Duration::from_secs(30);
    let (mut h, script, turn_id) =
        start(setup, vec![shell(DELETE_HOME)], Vec::new(), "clean up").await;
    next_question(&mut h).await;
    // NOTE: a guarded delete reads the presence twice when it parks: once to know
    // whether to wait at all, once in its wait.
    h.presence.wait_for_reads(2).await;
    h.presence.set_away(true);
    h.clock.advance(PRESENCE_CHECK);
    h.presence.wait_for_reads(3).await;
    h.clock.advance(Duration::from_secs(30));
    h.wait_end(turn_id).await;
    let waited = PRESENCE_CHECK + Duration::from_secs(30);
    assert_eq!(script.results(), vec![(exit::critical_expired(HOME_TARGET, Some(waited)), true)]);
}

#[tokio::test]
async fn full_guard_summary_names_the_target_first() {
    let (mut h, _, turn_id) =
        start(full(None), vec![shell(DELETE_HOME)], Vec::new(), "clean up").await;
    let (call_id, summary, _) = next_question(&mut h).await;
    let mut lines = summary.lines();
    assert_eq!(
        lines.next(),
        Some("full mode still asks for this: delete of ~ (your home directory)")
    );
    let second = lines.next().unwrap_or_default();
    assert!(second.starts_with("shell: run \"false && rm -rf ~\""), "the call next: {summary}");
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(turn_id).await;
}
