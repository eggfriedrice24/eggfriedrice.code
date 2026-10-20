//! Job notices in the actor (phase C): a notice never enters a running turn and is never
//! the user's input; it rides in the next turn, or starts a wake turn while the
//! conversation is idle, within the wake limits and the chain rule, in the mode of the
//! newest prompt.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    ApprovalDecision, EffectiveSettings, Event, EventEnvelope, FullModeStatus, HeldReason,
    JobState, JobStopBy, Mode, ModeFallback, NoticeKind, SandboxStatus, Seq, TurnId, TurnSettings,
};
use efr_provider::{ContentBlock, Message, Request, Role};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::testing::{DONE, Harness, Script, Setup, job_info};
use crate::turn::WAKE_QUESTION;
use crate::wake::CALL_CAP;
use crate::{JobNoticeInput, NOT_USER_INPUT, NoticeBody, notice_text};

/// One shell call of `line` that starts a background job.
fn job(line: &str) -> (&'static str, Value) {
    ("shell", json!({ "command": line, "background": true }))
}

/// One shell call of `line`.
fn shell(line: &str) -> (&'static str, Value) {
    ("shell", json!({ "command": line }))
}

/// A setup whose turns run in `auto`.
fn auto() -> Setup {
    let mut setup = Setup::new();
    setup.config.mode = Mode::Auto;
    setup
}

/// Starts `setup` with a model that first starts the job `j1` (`npm run dev`) and ends
/// its turn, then makes `later` in order. Waits for the end of the job's turn.
async fn with_job(setup: Setup, later: Vec<(&str, Value)>) -> (Harness, Arc<Script>, TurnId) {
    let mut calls = vec![job("npm run dev"), (DONE, Value::Null)];
    calls.extend(later);
    let script = Script::new(calls);
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let sent = h.prompt("run the dev server and tell me when it crashes").await;
    h.wait_end(sent.turn_id).await;
    (h, script, sent.turn_id)
}

/// The notice that the job `j<number>` ended with exit 1, from the event `seq`.
fn ended(h: &Harness, number: u32, seq: u64, wakes: bool) -> JobNoticeInput {
    let turn: TurnId = "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap();
    let call = "0192f0c1-7a00-7000-8000-000000000002".parse().unwrap();
    let job = job_info(h.conversation_id, turn, call, number, "npm run dev");
    let body = NoticeBody::Ended {
        state: JobState::Exited,
        exit_code: Some(1),
        stopped_by: None,
        tail: "error: write ~/notes.txt failed\n",
    };
    let text = notice_text(&job, &format!("$SCRATCH/jobs/j{number}.out"), body);
    JobNoticeInput::new(job, NoticeKind::Exited, Seq::new(seq), text, wakes)
}

/// The notice that a line of the job `j<number>` matched its watch pattern.
fn matched(h: &Harness, number: u32, seq: u64) -> JobNoticeInput {
    let turn: TurnId = "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap();
    let call = "0192f0c1-7a00-7000-8000-000000000002".parse().unwrap();
    let job = job_info(h.conversation_id, turn, call, number, "npm run dev");
    let body = NoticeBody::Watch { line: "panic: out of memory", wakes_left: 2 };
    let text = notice_text(&job, &format!("$SCRATCH/jobs/j{number}.out"), body);
    JobNoticeInput::new(job, NoticeKind::Watch, Seq::new(seq), text, true)
}

/// Hands the conversation `notice`.
async fn notify(h: &Harness, notice: JobNoticeInput) {
    h.handle.notice(notice).await.expect("the actor takes the notice");
}

/// Waits for the wake turn that a notice starts, and for its end.
async fn wake(h: &mut Harness) -> TurnId {
    let event = h.wait_for(|e| matches!(e, Event::JobNotice { woke: true, .. })).await;
    let Event::JobNotice { turn_id, .. } = event else {
        unreachable!("wait_for returns an event it accepted");
    };
    h.wait_end(turn_id).await;
    turn_id
}

/// Waits for the `job_notice_held` of the event `seq`, and returns its reason.
async fn held(h: &mut Harness, seq: u64) -> HeldReason {
    let event = h
        .wait_for(
            |e| matches!(e, Event::JobNoticeHeld { source_seq, .. } if source_seq.get() == seq),
        )
        .await;
    let Event::JobNoticeHeld { reason, .. } = event else {
        unreachable!("wait_for returns an event it accepted");
    };
    reason
}

/// The texts of the first message of `request`'s newest turn: the user message after
/// the last assistant message before it, or its first user message.
fn first_texts(request: &Request) -> Vec<String> {
    let start = request
        .messages
        .iter()
        .rposition(|message| message.role == Role::User && is_first(message))
        .expect("a first message");
    texts(&request.messages[start])
}

/// True for a first message of a turn: it starts with the live-state preamble.
fn is_first(message: &Message) -> bool {
    matches!(message.content.first(), Some(ContentBlock::Text { text }) if text.contains("<live_state>"))
}

/// The texts of `message`.
fn texts(message: &Message) -> Vec<String> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The events of the log with their envelopes, from `turn_id` on.
async fn turn_events(h: &Harness, turn_id: TurnId) -> Vec<EventEnvelope> {
    h.envelopes().await.into_iter().filter(|e| e.event.turn_id() == Some(turn_id)).collect()
}

#[tokio::test]
async fn job_notice_wakes_idle_conversation() {
    let (mut h, script, _) = with_job(auto(), Vec::new()).await;
    let notice = ended(&h, 1, 900, true);
    notify(&h, notice.clone()).await;
    let woke = wake(&mut h).await;

    let events = turn_events(&h, woke).await;
    assert!(matches!(events[0].event, Event::JobNotice { woke: true, .. }), "{:?}", events[0]);
    assert!(matches!(events[1].event, Event::TurnStarted { .. }), "{:?}", events[1]);
    assert!(matches!(events.last().unwrap().event, Event::TurnCompleted { .. }));
    let turns = h
        .store
        .readers()
        .with(move |conn| efr_store::conversations::turn(conn, woke))
        .await
        .unwrap()
        .unwrap();
    assert!(turns.wake && turns.command_id.is_none(), "{turns:?}");
    // The wake turn has no prompt: its first message is the preamble and the notice.
    let request = script.requests().last().cloned().unwrap();
    let first = first_texts(&request);
    assert_eq!(first.len(), 2, "{first:?}");
    assert_eq!(first[1], notice.text);
}

#[tokio::test]
async fn notices_recorded_before_turn_started() {
    let (mut h, _, _) = with_job(auto(), Vec::new()).await;
    notify(&h, ended(&h, 1, 900, true)).await;
    let woke = wake(&mut h).await;
    let events = turn_events(&h, woke).await;
    let (notice, started) = (&events[0], &events[1]);
    assert!(matches!(notice.event, Event::JobNotice { .. }));
    assert!(matches!(started.event, Event::TurnStarted { .. }));
    assert_eq!(notice.seq.get() + 1, started.seq.get(), "one batch, the notice first");
}

#[tokio::test]
async fn job_notice_marked_not_user_input() {
    let (mut h, script, _) = with_job(auto(), Vec::new()).await;
    notify(&h, ended(&h, 1, 900, true)).await;
    let woke = wake(&mut h).await;

    let request = script.requests().last().cloned().unwrap();
    let notice = first_texts(&request).pop().unwrap();
    assert!(notice.lines().nth(1) == Some(NOT_USER_INPUT), "{notice}");
    let events = turn_events(&h, woke).await;
    assert!(matches!(
        &events[0].event,
        Event::JobNotice { not_user_input: true, notice_kind: NoticeKind::Exited, .. }
    ));
}

#[tokio::test]
async fn watch_match_wakes_model() {
    let (mut h, script, _) = with_job(auto(), Vec::new()).await;
    let notice = matched(&h, 1, 901);
    notify(&h, notice.clone()).await;
    let woke = wake(&mut h).await;

    let events = turn_events(&h, woke).await;
    assert!(matches!(&events[0].event, Event::JobNotice { notice_kind: NoticeKind::Watch, .. }));
    let request = script.requests().last().cloned().unwrap();
    assert!(first_texts(&request).pop().unwrap().contains("matched: panic: out of memory"));
}

#[tokio::test]
async fn notice_during_turn_is_not_steering() {
    // The model of the second turn asks a question, and the notice comes while it waits.
    let mut setup = auto();
    setup.config.mode = Mode::Auto;
    let (mut h, script, _) = with_job(setup, vec![shell("sudo true"), (DONE, Value::Null)]).await;
    let sent = h.prompt("check sudo").await;
    let call_id = h.wait_approval().await;
    notify(&h, ended(&h, 1, 900, true)).await;
    assert_eq!(h.handle.state().await.unwrap().pending_approvals, vec![call_id]);
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(sent.turn_id).await;

    let events = turn_events(&h, sent.turn_id).await;
    assert!(!events.iter().any(|e| matches!(e.event, Event::SteeringDelivered { .. })));
    assert!(!events.iter().any(|e| matches!(e.event, Event::JobNotice { .. })));
    // The turn's own requests never held the notice; it starts the next turn.
    let requests = script.requests();
    let turn_requests = &requests[2..4];
    for request in turn_requests {
        let all: Vec<String> = request.messages.iter().flat_map(texts).collect();
        assert!(!all.iter().any(|text| text.contains(NOT_USER_INPUT)));
    }
    wake(&mut h).await;
}

#[tokio::test]
async fn job_notice_never_answers_pending_question() {
    let (mut h, _, _) = with_job(auto(), vec![shell("sudo true")]).await;
    let sent = h.prompt("check sudo").await;
    let call_id = h.wait_approval().await;
    notify(&h, ended(&h, 1, 900, true)).await;
    notify(&h, matched(&h, 1, 901)).await;
    let state = h.handle.state().await.unwrap();
    assert_eq!(state.pending_approvals, vec![call_id], "the question still waits");
    assert_eq!(state.running, Some(sent.turn_id));
    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalResolved { .. })));
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(sent.turn_id).await;
}

#[tokio::test]
async fn restart_notice_never_wakes() {
    let (mut h, script, _) = with_job(auto(), Vec::new()).await;
    let notice = ended(&h, 1, 900, false);
    notify(&h, notice.clone()).await;
    assert_eq!(held(&mut h, 900).await, HeldReason::DaemonRestart);
    assert_eq!(h.handle.state().await.unwrap().running, None, "no wake turn");

    // The next prompt's turn takes it after its prompt.
    let sent = h.prompt("what happened?").await;
    h.wait_end(sent.turn_id).await;
    let events = turn_events(&h, sent.turn_id).await;
    assert!(matches!(&events[1].event, Event::JobNotice { woke: false, .. }), "{:?}", events[1]);
    assert!(matches!(&events[2].event, Event::TurnStarted { .. }), "{:?}", events[2]);
    let first = first_texts(&script.requests().last().cloned().unwrap());
    assert_eq!(first[1..], ["what happened?".to_owned(), notice.text]);
}

/// A notice that efrd hands the actor again at each start is held once: the store
/// keeps the first `job_notice_held`.
#[tokio::test]
async fn restart_records_held_notice_once() {
    let (mut h, _, _) = with_job(auto(), Vec::new()).await;
    let end = Event::JobEnded {
        job_id: efr_protocol::JobId::nth(1),
        state: JobState::Exited,
        exit_code: Some(1),
        stopped_by: None,
        output_bytes: 10,
        ran_secs: 3,
        tail: "error\n".to_owned(),
        surface_changes: Vec::new(),
        quarantined: false,
        survivors: Vec::new(),
        launcher_lost: false,
        output_truncated: false,
        blocked: Vec::new(),
        blocked_more: 0,
        bus_refused: Vec::new(),
        bus_refused_more: 0,
    };
    let committed =
        h.store.writer().append(efr_store::Batch::new().event(h.conversation_id, end)).await;
    let seq = committed.unwrap().events()[0].seq.get();
    notify(&h, ended(&h, 1, seq, false)).await;
    assert_eq!(held(&mut h, seq).await, HeldReason::DaemonRestart);
    let script = Script::new(Vec::new());
    for _ in 0..2 {
        h = h.restart_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
        notify(&h, ended(&h, 1, seq, false)).await;
        // NOTE: the actor answers in order, so the notice was handled before this.
        h.handle.state().await.unwrap();
    }
    let events = h.store.events().await.unwrap();
    let held_count = events
        .iter()
        .filter(|envelope| matches!(envelope.event, Event::JobNoticeHeld { .. }))
        .count();
    assert_eq!(held_count, 1, "a restart records no second job_notice_held");
}

#[tokio::test]
async fn wake_turn_without_start_turn_never_wakes() {
    let (mut h, _, _) = with_job(auto(), Vec::new()).await;
    notify(&h, ended(&h, 7, 900, true)).await;
    assert_eq!(held(&mut h, 900).await, HeldReason::DaemonRestart);
    assert_eq!(h.handle.state().await.unwrap().running, None);
}

#[tokio::test]
async fn held_notice_rides_in_next_prompt_turn() {
    let mut setup = auto();
    setup.config.max_wakes_per_hour = 0;
    let (mut h, script, _) = with_job(setup, Vec::new()).await;
    let notice = ended(&h, 1, 900, true);
    notify(&h, notice.clone()).await;
    assert_eq!(held(&mut h, 900).await, HeldReason::WakesOff);

    let sent = h.prompt("go on").await;
    h.wait_end(sent.turn_id).await;
    let first = first_texts(&script.requests().last().cloned().unwrap());
    assert_eq!(first[1..], ["go on".to_owned(), notice.text]);
    let events = turn_events(&h, sent.turn_id).await;
    assert!(matches!(&events[0].event, Event::PromptQueued { .. }));
    assert!(matches!(&events[1].event, Event::JobNotice { woke: false, .. }));
}

/// The mode that the turn `turn_id` started with.
async fn started_mode(h: &Harness, turn_id: TurnId) -> Mode {
    turn_events(h, turn_id)
        .await
        .iter()
        .find_map(|e| match &e.event {
            Event::TurnStarted { settings: Some(settings), .. } => Some(settings.mode),
            _ => None,
        })
        .expect("the turn started with its settings")
}

#[tokio::test]
async fn wake_turn_runs_in_cautious_and_asks() {
    // The job started in a `cautious` turn, after the user said yes. Its end wakes the
    // model in `cautious`, and a call of the wake turn asks.
    let script = Script::new(vec![job("npm run dev"), (DONE, Value::Null), shell("sudo true")]);
    let mut h =
        Setup::new().start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let sent = h.prompt("run the dev server").await;
    let call_id = h.wait_approval().await;
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(sent.turn_id).await;
    h.presence.set_followed(true);

    notify(&h, ended(&h, 1, 900, true)).await;
    let event = h.wait_for(|e| matches!(e, Event::JobNotice { woke: true, .. })).await;
    let woke = event.turn_id().unwrap();
    let call_id = h.wait_approval().await;
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(woke).await;
    assert_eq!(started_mode(&h, woke).await, Mode::Cautious);
}

#[tokio::test]
async fn wake_turn_follows_newest_prompt_mode() {
    let (mut h, _, _) = with_job(auto(), vec![(DONE, Value::Null), shell("sudo true")]).await;
    let cwd = h.cwd.clone();
    let mut params = h.prompt_params(&cwd, "thanks");
    params.settings = TurnSettings { mode: Some(Mode::Manual), ..TurnSettings::default() };
    let sent = h.handle.send_prompt(params, efr_protocol::Origin::Shell).await.unwrap();
    h.wait_end(sent.turn_id).await;
    h.presence.set_followed(true);

    notify(&h, ended(&h, 1, 900, true)).await;
    let event = h.wait_for(|e| matches!(e, Event::JobNotice { woke: true, .. })).await;
    let woke = event.turn_id().unwrap();
    let call_id = h.wait_approval().await;
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(woke).await;
    assert_eq!(started_mode(&h, woke).await, Mode::Manual);
}

/// A setup whose turns run in full mode, which the daemon allows.
fn full() -> Setup {
    let mut setup = Setup::new();
    setup.config.mode = Mode::Full;
    setup.config.full_mode = FullModeStatus { allowed: true, refused: None };
    setup
}

/// The settings that the turn `turn_id` started with.
async fn started_settings(h: &Harness, turn_id: TurnId) -> EffectiveSettings {
    turn_events(h, turn_id)
        .await
        .iter()
        .find_map(|e| match &e.event {
            Event::TurnStarted { settings: Some(settings), .. } => Some(settings.clone()),
            _ => None,
        })
        .expect("the turn started with its settings")
}

#[tokio::test]
async fn wake_turn_of_full_runs_as_auto() {
    let (mut h, _, job_turn) = with_job(full(), Vec::new()).await;
    assert_eq!(started_mode(&h, job_turn).await, Mode::Full, "the job started in full mode");
    assert_eq!(h.toolbox.invoked().len(), 1, "a full job starts with no question");

    notify(&h, ended(&h, 1, 900, true)).await;
    let woke = wake(&mut h).await;
    let settings = started_settings(&h, woke).await;
    assert_eq!(settings.mode, Mode::Auto);
    assert_eq!(
        settings.fallback,
        Some(ModeFallback { asked: Mode::Full, reason: ModeFallback::WAKE_REASON.to_owned() })
    );
}

#[tokio::test]
async fn wake_turn_of_full_without_sandbox_runs_as_cautious() {
    let mut setup = full();
    setup.sandbox = SandboxStatus::unavailable("bubblewrap is not installed");
    let (mut h, _, _) = with_job(setup, Vec::new()).await;

    notify(&h, ended(&h, 1, 900, true)).await;
    let woke = wake(&mut h).await;
    let settings = started_settings(&h, woke).await;
    assert_eq!(settings.mode, Mode::Cautious);
    assert_eq!(
        settings.fallback.map(|fallback| fallback.reason),
        Some(format!("{}; bubblewrap is not installed", ModeFallback::WAKE_REASON))
    );
}

#[tokio::test]
async fn wake_turn_of_full_with_the_gate_off_runs_as_auto() {
    let (mut h, _, _) = with_job(full(), Vec::new()).await;
    // The user turns full mode off after the job started.
    let mut config = (**h.settings.borrow()).clone();
    config.full_mode = FullModeStatus { allowed: false, refused: None };
    h.settings.send_replace(Arc::new(config));

    notify(&h, ended(&h, 1, 900, true)).await;
    let woke = wake(&mut h).await;
    let end = turn_events(&h, woke).await.pop().map(|e| e.event);
    assert!(matches!(end, Some(Event::TurnCompleted { .. })), "no gate error: {end:?}");
    let settings = started_settings(&h, woke).await;
    assert_eq!(settings.mode, Mode::Auto);
    assert_eq!(settings.fallback.map(|fallback| fallback.asked), Some(Mode::Full));
}

#[tokio::test]
async fn model_stopped_job_rides_in_next_prompt_without_held() {
    let (mut h, script, _) = with_job(auto(), Vec::new()).await;
    let mut notice = ended(&h, 1, 900, false);
    notice.job.stopped_by = Some(JobStopBy::Model);
    notify(&h, notice.clone()).await;
    // NOTE: the actor answers in order, so the notice was handled before this.
    assert_eq!(h.handle.state().await.unwrap().running, None, "no wake turn");

    let sent = h.prompt("status?").await;
    h.wait_end(sent.turn_id).await;
    let first = first_texts(&script.requests().last().cloned().unwrap());
    assert_eq!(first[1..], ["status?".to_owned(), notice.text]);
    let events = h.store.events().await.unwrap();
    assert!(
        !events.iter().any(|envelope| matches!(envelope.event, Event::JobNoticeHeld { .. })),
        "a job that the model stopped records no job_notice_held"
    );
}

#[tokio::test]
async fn job_started_in_wake_turn_never_wakes() {
    let (mut h, _, _) = with_job(auto(), vec![job("cargo build"), (DONE, Value::Null)]).await;
    notify(&h, ended(&h, 1, 900, true)).await;
    wake(&mut h).await;
    assert_eq!(h.toolbox.invoked().len(), 2, "the wake turn started j2");

    notify(&h, ended(&h, 2, 901, true)).await;
    assert_eq!(held(&mut h, 901).await, HeldReason::WakeTurnJob);
    assert_eq!(h.handle.state().await.unwrap().running, None);
}

#[tokio::test]
async fn wake_limit_six_per_hour() {
    let (mut h, _, _) = with_job(auto(), Vec::new()).await;
    for seq in 900..906 {
        notify(&h, matched(&h, 1, seq)).await;
        wake(&mut h).await;
    }
    notify(&h, matched(&h, 1, 906)).await;
    assert_eq!(held(&mut h, 906).await, HeldReason::WakeLimit);
    assert_eq!(h.handle.state().await.unwrap().running, None);

    // An hour later one more may start, and it takes the notice that waited.
    h.clock.advance(Duration::from_secs(3600));
    notify(&h, matched(&h, 1, 907)).await;
    let woke = wake(&mut h).await;
    let taken: Vec<u64> = turn_events(&h, woke)
        .await
        .iter()
        .filter_map(|e| match &e.event {
            Event::JobNotice { source_seq, .. } => Some(source_seq.get()),
            _ => None,
        })
        .collect();
    assert_eq!(taken, [906, 907]);
}

#[tokio::test]
async fn notice_over_wake_limit_waits_for_next_turn() {
    let mut setup = auto();
    setup.config.max_wakes_per_hour = 1;
    let (mut h, script, _) = with_job(setup, Vec::new()).await;
    notify(&h, matched(&h, 1, 900)).await;
    wake(&mut h).await;
    let over = matched(&h, 1, 901);
    notify(&h, over.clone()).await;
    assert_eq!(held(&mut h, 901).await, HeldReason::WakeLimit);

    let sent = h.prompt("status?").await;
    h.wait_end(sent.turn_id).await;
    let first = first_texts(&script.requests().last().cloned().unwrap());
    assert_eq!(first[1..], ["status?".to_owned(), over.text]);
}

#[tokio::test]
async fn the_daemon_wide_limit_holds_a_notice() {
    let (mut h, _, _) = with_job(auto(), Vec::new()).await;
    h.toolbox.wakes_denied.store(true, std::sync::atomic::Ordering::SeqCst);
    notify(&h, ended(&h, 1, 900, true)).await;
    assert_eq!(held(&mut h, 900).await, HeldReason::WakeLimit);
}

#[tokio::test]
async fn prompt_turn_takes_at_most_ten_notices() {
    let (mut h, script, _) = with_job(auto(), Vec::new()).await;
    for seq in 900..912 {
        notify(&h, ended(&h, 1, seq, false)).await;
    }
    held(&mut h, 911).await;
    let sent = h.prompt("what ended?").await;
    h.wait_end(sent.turn_id).await;

    let first = first_texts(&script.requests().last().cloned().unwrap());
    assert_eq!(first.len(), 1 + 1 + 10 + 1, "preamble, prompt, ten notices, one line");
    assert_eq!(first.last().unwrap(), "2 more job notices; call jobs to read them");
    let taken = turn_events(&h, sent.turn_id)
        .await
        .iter()
        .filter(|e| matches!(e.event, Event::JobNotice { .. }))
        .count();
    assert_eq!(taken, 10);
}

#[tokio::test]
async fn wake_turn_started_has_start_turn_cwd() {
    let (mut h, _, _) = with_job(auto(), vec![(DONE, Value::Null)]).await;
    let start_cwd = h.cwd.clone();
    let elsewhere = h.dirs.create_dir("home/other").unwrap();
    let sent = h.prompt_in(&elsewhere, "look around").await;
    h.wait_end(sent.turn_id).await;

    notify(&h, ended(&h, 1, 900, true)).await;
    let woke = wake(&mut h).await;
    let events = turn_events(&h, woke).await;
    let cwd = events.iter().find_map(|e| match &e.event {
        Event::TurnStarted { cwd, .. } => Some(cwd.clone()),
        _ => None,
    });
    assert_eq!(cwd.as_deref(), Some(Path::new(&start_cwd)));
    assert_eq!(h.toolbox.moved.lock().unwrap().len(), 1, "only the user's own move");
}

#[tokio::test]
async fn user_prompt_queues_behind_wake_turn() {
    let (mut h, _, _) = with_job(auto(), vec![shell("sudo true")]).await;
    notify(&h, ended(&h, 1, 900, true)).await;
    let event = h.wait_for(|e| matches!(e, Event::JobNotice { woke: true, .. })).await;
    let woke = event.turn_id().unwrap();
    let call_id = h.wait_approval().await;
    let queued = h.prompt("stop that").await;
    assert!(queued.queued, "the prompt waits for the wake turn");
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(woke).await;
    h.wait_end(queued.turn_id).await;
}

#[tokio::test]
async fn wake_turn_question_marked() {
    let (mut h, _, _) = with_job(auto(), vec![shell("sudo true")]).await;
    h.presence.set_followed(true);
    notify(&h, ended(&h, 1, 900, true)).await;
    let asked = h.wait_for(|e| matches!(e, Event::ApprovalRequested { .. })).await;
    let Event::ApprovalRequested { summary, call_id, turn_id, .. } = asked else {
        unreachable!("wait_for returns an event it accepted");
    };
    assert!(summary.starts_with(WAKE_QUESTION), "{summary}");
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(turn_id).await;
}

#[tokio::test]
async fn wake_turn_question_away_without_follower() {
    // The user typed a minute ago, but no client that can answer follows the
    // conversation: the question of a wake turn waits only the away wait.
    let (mut h, _, _) = with_job(auto(), vec![shell("sudo true")]).await;
    notify(&h, ended(&h, 1, 900, true)).await;
    let event = h.wait_for(|e| matches!(e, Event::JobNotice { woke: true, .. })).await;
    let woke = event.turn_id().unwrap();
    let call_id = h.wait_approval().await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(300));
    h.wait_end(woke).await;
    let events = h.events().await;
    assert!(events.contains(&Event::ApprovalExpired { turn_id: woke, call_id, away: true }));
    assert!(h.toolbox.invoked().len() == 1, "only the job ran");
}

#[tokio::test]
async fn away_wake_turn_question_expires() {
    let (mut h, script, _) = with_job(auto(), vec![shell("sudo true")]).await;
    h.presence.set_away(true);
    h.presence.set_followed(true);
    notify(&h, ended(&h, 1, 900, true)).await;
    let event = h.wait_for(|e| matches!(e, Event::JobNotice { woke: true, .. })).await;
    let woke = event.turn_id().unwrap();
    h.wait_approval().await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(300));
    h.wait_end(woke).await;
    assert_eq!(script.results().last().unwrap().0, crate::exit::AWAY_EXPIRED);
}

#[tokio::test]
async fn wake_turn_does_not_set_present() {
    let (mut h, _, _) = with_job(auto(), Vec::new()).await;
    h.presence.set_away(true);
    let reads = h.presence.reads();
    notify(&h, ended(&h, 1, 900, true)).await;
    wake(&mut h).await;
    assert_eq!(h.presence.reads(), reads, "a wake turn without a question reads no presence");
    assert!(h.presence.pushed().is_empty());
    use crate::PresenceReader as _;
    assert_eq!(h.presence.presence(h.conversation_id), efr_protocol::Presence::Away);
}

#[tokio::test]
async fn wake_turn_stops_at_call_cap() {
    let calls: Vec<(&str, Value)> = (0..25).map(|n| shell(&format!("echo {n}"))).collect();
    let (mut h, script, _) = with_job(auto(), calls).await;
    notify(&h, ended(&h, 1, 900, true)).await;
    let woke = wake(&mut h).await;

    assert_eq!(h.toolbox.invoked().len(), 1 + 20, "the job, then 20 calls");
    let events = turn_events(&h, woke).await;
    assert!(matches!(events.last().unwrap().event, Event::TurnCompleted { .. }));
    // The next request says why the turn stopped.
    let sent = h.prompt("what now?").await;
    h.wait_end(sent.turn_id).await;
    let request = script.requests().last().cloned().unwrap();
    let all: Vec<String> = request.messages.iter().flat_map(texts).collect();
    assert!(all.iter().any(|text| text == CALL_CAP), "{all:?}");
}

#[tokio::test]
async fn esc_stops_foreground_not_jobs() {
    let (mut h, _, _) = with_job(auto(), vec![("hang", json!({}))]).await;
    let sent = h.prompt("wait").await;
    h.toolbox.hang_started.notified().await;
    let params = h.interrupt_params(Some(sent.turn_id), Vec::new(), Vec::new());
    h.handle.interrupt(params, efr_protocol::Origin::Shell).await.unwrap();
    h.wait_end(sent.turn_id).await;

    let ran = h.toolbox.ran();
    let job_call = ran[0].call_id;
    let cancelled = h.toolbox.cancelled();
    assert_eq!(cancelled, vec![ran[1].call_id], "only the foreground call");
    assert!(!cancelled.contains(&job_call));
}

#[tokio::test]
async fn wake_turn_history_rebuilds_notice() {
    let (mut h, script, _) = with_job(auto(), Vec::new()).await;
    notify(&h, ended(&h, 1, 900, true)).await;
    wake(&mut h).await;
    let sent_by_wake = script.requests().last().cloned().unwrap();
    let wake_first = sent_by_wake.messages.iter().rev().find(|m| is_first(m)).cloned().unwrap();

    // The next request carries the wake turn's first message as it was sent, also after
    // a restart, which reads it back from the store.
    let h = h.restart_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let mut h = h;
    let sent = h.prompt("and now?").await;
    h.wait_end(sent.turn_id).await;
    let request = script.requests().last().cloned().unwrap();
    assert!(request.messages.contains(&wake_first));
}

/// A shell call that writes `~/notes.txt`, an exit that the classifier judges.
fn write_notes(h: &Setup) -> (&'static str, Value) {
    let path = h.home().join("notes.txt");
    ("shell", json!({ "command": "echo x > ~/notes.txt", "writes": [path] }))
}

#[tokio::test]
async fn wake_turn_fact_in_record() {
    let mut setup = auto();
    let judge = crate::testing::FakeJudge::answering(vec![crate::testing::denies()]);
    setup.judge = Some(Arc::clone(&judge));
    let write = write_notes(&setup);
    let (mut h, _, _) = with_job(setup, vec![write]).await;
    notify(&h, ended(&h, 1, 900, true)).await;
    wake(&mut h).await;

    assert_eq!(judge.calls(), 1);
    let record = &judge.seen()[0].record;
    assert!(record.action.wake_turn);
    assert!(!record.action.background, "the call itself is no job");
    assert_eq!(record.user_messages, vec!["run the dev server and tell me when it crashes"]);
}

#[tokio::test]
async fn wake_turn_keeps_classifier_pause() {
    let mut setup = auto();
    let denials = vec![crate::testing::denies(); 3];
    let judge = crate::testing::FakeJudge::answering(denials);
    setup.judge = Some(Arc::clone(&judge));
    let write = write_notes(&setup);
    let (mut h, _, _) = with_job(
        setup,
        vec![write.clone(), write.clone(), write.clone(), (DONE, Value::Null), write],
    )
    .await;
    notify(&h, ended(&h, 1, 900, true)).await;
    wake(&mut h).await;
    assert_eq!(judge.calls(), 3, "three denials pause the classifier");

    // The next wake turn goes on paused: its exit asks the user, not the classifier.
    h.presence.set_followed(true);
    notify(&h, ended(&h, 1, 901, true)).await;
    let event = h.wait_for(|e| matches!(e, Event::JobNotice { woke: true, .. })).await;
    let woke = event.turn_id().unwrap();
    let call_id = h.wait_approval().await;
    assert_eq!(judge.calls(), 3);
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(woke).await;
}

#[tokio::test]
async fn a_paused_wake_turn_stops_when_nobody_follows() {
    let mut setup = auto();
    let judge = crate::testing::FakeJudge::answering(vec![crate::testing::denies(); 3]);
    setup.judge = Some(Arc::clone(&judge));
    let write = write_notes(&setup);
    let (mut h, _, _) = with_job(
        setup,
        vec![write.clone(), write.clone(), write.clone(), (DONE, Value::Null), write],
    )
    .await;
    notify(&h, ended(&h, 1, 900, true)).await;
    wake(&mut h).await;

    // Nobody follows the conversation: the paused classifier cannot hand the exit to a
    // person, so the turn stops at once, with no question.
    notify(&h, ended(&h, 1, 901, true)).await;
    let woke = wake(&mut h).await;
    let events = turn_events(&h, woke).await;
    assert!(matches!(events.last().unwrap().event, Event::TurnFailed { .. }));
    assert!(!events.iter().any(|e| matches!(e.event, Event::ApprovalRequested { .. })));
    assert_eq!(judge.calls(), 3);
}
