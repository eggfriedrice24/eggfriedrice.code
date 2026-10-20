//! Full mode in a turn: calls run as the user with no sandbox and no question, the exits
//! that full mode lets through are recorded with the judge `full`, nobody reviews them,
//! the gate holds before each call and at the start of a queued prompt or steer.
//!
//! NOTE: the fake toolbox runs no command: each line here is only read by the engine.

use std::path::Path;
use std::sync::Arc;

use efr_permissions::{CallFacts, Requirements, TargetKind};
use efr_protocol::{
    ApprovalDecision, ErrorCode, Event, ExitKind, FullModeStatus, FullRefusal, JudgeKind, Launch,
    Mode, Origin, SandboxStatus, TurnId, TurnSettings, Verdict,
};
use pretty_assertions::assert_eq;
use serde_json::Value;

use super::review::{count, judgements, reviewed, shell};
use crate::ConversationError;
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
