//! Turns that job notices start (efr's auto spec, section 3.18, phase C), end to end
//! from a job's end to the provider's request: the shared fake launcher of
//! `efr-test-daemon` (a `/bin/sh` script, no bubblewrap, no zsh), the `job_env` seam, a
//! scripted model that keeps the first message of each turn, and the `TestClock`. A job
//! that must outlive its turn blocks on a FIFO that the test writes after the turn's
//! end, so no test waits on a fixed time.
//!
//! A notice wakes an idle conversation only in `auto`, within the wake limits, and
//! never for a job that a wake turn started or that ran when efrd restarted; a notice
//! that does not wake rides in the first message of the next turn that a prompt starts.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use efr_conversation::NOT_USER_INPUT;
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, ConversationId, Event, EventEnvelope,
    HeldReason, JobState, Method, Mode, NoticeKind, PromptSendResult, TurnId, TurnSettings,
};
use efr_provider::{
    Message, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, Role,
    StopReason,
};
use efr_stdx::time::Clock as _;
use efr_test_daemon::{FakeLauncher, TTY, TestDaemon, TestDaemonBuilder, command_id, events_until};
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::sandbox::{REVIEWER_ALLOWS, ready};

/// The reviewer's answer that hands an exit to the user.
pub(crate) const REVIEWER_ASKS: &str = r#"{"verdict":"ask_user","risk":"medium","user_authorization":"low","category":"unsure","rationale":"the user decides"}"#;

/// One answer of the model: a tool call, or `None` for the end of the turn.
pub(crate) type Answer = Option<(&'static str, Value)>;

/// A model that answers each request of every turn with the next answer of its list,
/// and keeps the text blocks of the first message of each turn: the preamble, the
/// prompt of a prompt turn, then the notices. The reviewer answers `verdict`.
pub(crate) struct Recorder {
    id: ProviderId,
    answers: Mutex<VecDeque<Answer>>,
    firsts: Mutex<Vec<Vec<String>>>,
    calls: AtomicUsize,
    reviews: AtomicUsize,
    verdict: &'static str,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder").finish_non_exhaustive()
    }
}

impl Recorder {
    pub(crate) fn new(answers: Vec<Answer>) -> Arc<Self> {
        Recorder::reviewed(answers, REVIEWER_ALLOWS)
    }

    pub(crate) fn reviewed(answers: Vec<Answer>, verdict: &'static str) -> Arc<Self> {
        Arc::new(Recorder {
            id: ProviderId::new("test").unwrap(),
            answers: Mutex::new(answers.into()),
            firsts: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            reviews: AtomicUsize::new(0),
            verdict,
        })
    }

    /// The text blocks of the first message of each turn, in order.
    pub(crate) fn firsts(&self) -> Vec<Vec<String>> {
        self.firsts.lock().unwrap().clone()
    }

    /// How many reviews the reviewer answered.
    pub(crate) fn reviews(&self) -> usize {
        self.reviews.load(Ordering::SeqCst)
    }
}

/// The text blocks of `message`.
fn texts(message: &Message) -> Vec<String> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            efr_provider::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn stream(events: Vec<ProviderEvent>) -> ProviderStream {
    Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
}

#[async_trait]
impl Provider for Recorder {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        if request.one_shot {
            self.reviews.fetch_add(1, Ordering::SeqCst);
            return Ok(stream(vec![
                ProviderEvent::TextDelta { text: self.verdict.to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ]));
        }
        // NOTE: the first request of a turn ends with the turn's first message; a later
        // one ends with the tool results, which have no text block.
        if let Some(last) = request.messages.last()
            && last.role == Role::User
        {
            let blocks = texts(last);
            if !blocks.is_empty() {
                self.firsts.lock().unwrap().push(blocks);
            }
        }
        let next = self.answers.lock().unwrap().pop_front().flatten();
        let Some((name, input)) = next else {
            return Ok(stream(vec![
                ProviderEvent::TextDelta { text: "done".to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ]));
        };
        let call_id = format!("call_{}", self.calls.fetch_add(1, Ordering::SeqCst));
        Ok(stream(vec![
            ProviderEvent::ToolCallStart {
                call_id: call_id.clone(),
                name: name.to_owned(),
                freeform: false,
            },
            ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() },
            ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None },
        ]))
    }
}

/// A test's tree with the fake launcher.
struct Setup {
    dirs: Arc<TestDirs>,
    fake: FakeLauncher,
}

impl Setup {
    fn new() -> Setup {
        let dirs = Arc::new(TestDirs::new().unwrap());
        let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
        Setup { dirs, fake }
    }

    /// A daemon over `model` with the fake launcher, a ready sandbox, a fixed job
    /// environment and `auto` as the config's mode. A wake turn runs in the lowest of
    /// the job turn's mode, the newest prompt's mode and the config's mode, so a config
    /// in `cautious` never wakes.
    fn builder(&self, model: Arc<Recorder>) -> TestDaemonBuilder {
        let env = BTreeMap::from([
            ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
            ("HOME".to_owned(), self.dirs.home().display().to_string()),
        ]);
        TestDaemon::builder()
            .dirs(Arc::clone(&self.dirs))
            .sandbox_launcher(self.fake.path())
            .probe_override(ready())
            .job_env(env)
            .config(|config| config.permissions.mode = Mode::Auto)
            .custom_provider(model)
    }

    async fn start(&self, model: Arc<Recorder>) -> TestDaemon {
        self.builder(model).start().await.unwrap()
    }

    /// A FIFO below the test's root, which a job reads and the test writes.
    fn fifo(&self, name: &str) -> PathBuf {
        let path = self.dirs.root().join(name);
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        path
    }
}

/// Writes one line to `fifo`, which a job reads; it waits for the reader.
async fn release(fifo: &Path) {
    let fifo = fifo.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::write(&fifo, b"go\n")).await.unwrap().unwrap();
}

/// A background `shell` call that runs `command`.
fn job(command: &str) -> Answer {
    Some(("shell", json!({ "command": command, "background": true })))
}

/// A job line that blocks on `fifo`, then runs `then`.
fn blocked(fifo: &Path, then: &str) -> String {
    format!("read answer < {}; {then}", fifo.display())
}

/// The turn of the prompt `text` with the command id `n` in `mode` from the test's
/// terminal, up to its own end, with each of its questions answered with `decision`.
/// The events start at the prompt's `prompt_queued`; an earlier turn's are left out.
pub(crate) async fn turn(
    daemon: &TestDaemon,
    n: u128,
    text: &str,
    mode: Mode,
    decision: ApprovalDecision,
) -> (PromptSendResult, Vec<EventEnvelope>) {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(n, text, TTY) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(mode), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let own = sent.turn_id;
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let mut seen = Vec::new();
    let mut answers = n * 100;
    loop {
        let events = events_until(&mut follow, |event| {
            event.turn_id() == Some(own)
                && matches!(
                    event,
                    Event::ApprovalRequested { .. }
                        | Event::TurnCompleted { .. }
                        | Event::TurnFailed { .. }
                        | Event::TurnInterrupted { .. }
                )
        })
        .await
        .unwrap();
        let last = events.last().unwrap().event.clone();
        seen.extend(events);
        let Event::ApprovalRequested { call_id, .. } = last else { break };
        answers += 1;
        let answer = Method::ApprovalRespond(ApprovalRespond {
            command_id: command_id(answers),
            conversation_id: sent.conversation_id,
            call_id,
            decision,
        });
        let _: ApprovalRespondResult = client.call(answer).await.unwrap();
    }
    seen.retain(|envelope| envelope.seq >= sent.seq);
    (sent, seen)
}

/// [`turn`] with a no to each question.
async fn prompt(daemon: &TestDaemon, n: u128, text: &str, mode: Mode) -> PromptSendResult {
    turn(daemon, n, text, mode, ApprovalDecision::Deny).await.0
}

/// The events of `conversation` from its start up to the first that `stop` accepts.
async fn until(
    daemon: &TestDaemon,
    conversation: ConversationId,
    stop: impl FnMut(&Event) -> bool,
) -> Vec<EventEnvelope> {
    let client = daemon.client().await.unwrap();
    let mut follow = daemon.follow(&client, conversation).await.unwrap();
    events_until(&mut follow, stop).await.unwrap()
}

/// The events of `conversation` up to the end of its `n`th turn.
async fn until_turn_end(
    daemon: &TestDaemon,
    conversation: ConversationId,
    n: usize,
) -> Vec<EventEnvelope> {
    let mut ends = 0;
    until(daemon, conversation, |event| {
        if matches!(
            event,
            Event::TurnCompleted { .. } | Event::TurnFailed { .. } | Event::TurnInterrupted { .. }
        ) {
            ends += 1;
        }
        ends == n
    })
    .await
}

/// The events of `conversation` up to the `job_notice_held` of `job`.
async fn until_held(
    daemon: &TestDaemon,
    conversation: ConversationId,
    job: &str,
) -> Vec<EventEnvelope> {
    until(
        daemon,
        conversation,
        |event| matches!(event, Event::JobNoticeHeld { job_id, .. } if job_id.as_str() == job),
    )
    .await
}

/// The turns that started in `events`, in order.
fn turn_starts(events: &[EventEnvelope]) -> Vec<TurnId> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::TurnStarted { turn_id, .. } => Some(*turn_id),
            _ => None,
        })
        .collect()
}

/// Each `job_notice` of `events`: its turn, job, kind, whether it woke the turn and its
/// text.
fn notices(events: &[EventEnvelope]) -> Vec<(TurnId, String, NoticeKind, bool, String)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::JobNotice {
                turn_id, job_id, notice_kind, woke, not_user_input, text, ..
            } => {
                assert!(*not_user_input, "a notice is never the user's input");
                Some((*turn_id, job_id.to_string(), *notice_kind, *woke, text.clone()))
            }
            _ => None,
        })
        .collect()
}

/// Each `job_notice_held` of `events`: the job and the reason.
fn held(events: &[EventEnvelope]) -> Vec<(String, HeldReason)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::JobNoticeHeld { job_id, reason, .. } => Some((job_id.to_string(), *reason)),
            _ => None,
        })
        .collect()
}

/// The seq of the first event of `events` that `find` accepts.
fn seq_of(events: &[EventEnvelope], find: impl Fn(&Event) -> bool) -> u64 {
    events.iter().find(|envelope| find(&envelope.event)).unwrap().seq.get()
}

/// True when `events` hold a `prompt_queued` of `turn`.
fn queued(events: &[EventEnvelope], turn: TurnId) -> bool {
    events.iter().any(|envelope| {
        matches!(&envelope.event, Event::PromptQueued { turn_id, .. } if *turn_id == turn)
    })
}

#[tokio::test]
async fn job_notice_wakes_idle_conversation() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let model = Recorder::new(vec![
        job(&blocked(&fifo, "echo 'panic: out of cheese'; exit 3")),
        None,
        // The wake turn ends at once.
        None,
    ]);
    let daemon = setup.start(Arc::clone(&model)).await;
    let sent = prompt(&daemon, 1, "run the server and tell me when it crashes", Mode::Auto).await;
    release(&fifo).await;
    let events = until_turn_end(&daemon, sent.conversation_id, 2).await;
    let starts = turn_starts(&events);
    let [first, wake] = starts.try_into().unwrap();
    assert_ne!(first, wake);
    assert!(!queued(&events, wake), "no prompt started the wake turn");
    let [(turn, job_id, kind, woke, text)] = notices(&events).try_into().unwrap();
    assert_eq!((turn, job_id.as_str(), kind, woke), (wake, "j1", NoticeKind::Exited, true));
    assert!(text.starts_with("<job id=\"j1\" state=\"exited\" exit=\"3\""), "{text}");
    assert!(text.contains(NOT_USER_INPUT), "{text}");
    assert!(text.contains("\npanic: out of cheese\n"), "{text}");
    let ended = seq_of(&events, |event| matches!(event, Event::JobEnded { .. }));
    let noticed = seq_of(&events, |event| matches!(event, Event::JobNotice { .. }));
    let started = seq_of(
        &events,
        |event| matches!(event, Event::TurnStarted { turn_id, .. } if *turn_id == wake),
    );
    assert!(ended < noticed && noticed < started, "{ended} < {noticed} < {started}");
    // The provider saw the wake turn's first message: the preamble and the notice, and
    // no prompt.
    let firsts = model.firsts();
    let [prompt_turn, wake_turn] = firsts.try_into().unwrap();
    assert_eq!(prompt_turn[1], "run the server and tell me when it crashes");
    assert_eq!(wake_turn.len(), 2, "{wake_turn:?}");
    assert_eq!(wake_turn[1], text);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn wake_turn_row_in_history() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let model = Recorder::new(vec![job(&blocked(&fifo, "echo ended")), None, None]);
    // NOTE: a persistent daemon keeps its database on disk, where the test reads it.
    let daemon = setup.builder(model).persistent().start().await.unwrap();
    let sent = prompt(&daemon, 1, "start it", Mode::Auto).await;
    release(&fifo).await;
    let events = until_turn_end(&daemon, sent.conversation_id, 2).await;
    let [first, wake] = turn_starts(&events).try_into().unwrap();
    let database = daemon.dirs().dirs().data().join(efr_store::DATABASE_FILE);
    let readers = efr_store::Readers::open(database, 1);
    let conversation = sent.conversation_id;
    let turns = readers
        .with(move |conn| efr_store::conversations::turns(conn, conversation))
        .await
        .unwrap();
    let rows: Vec<(TurnId, bool, bool, &str, bool)> = turns
        .iter()
        .map(|turn| {
            let done = turn.status == efr_store::conversations::TurnStatus::Completed;
            (turn.id, turn.wake, turn.command_id.is_some(), turn.prompt.as_str(), done)
        })
        .collect();
    assert_eq!(
        rows,
        [(first, false, true, "start it", true), (wake, true, false, "job notice: j1", true)]
    );
    let wake_row = turns.iter().find(|turn| turn.wake).unwrap();
    assert!(wake_row.started_at.is_some(), "the wake turn is an undo candidate");
    // The history reads the wake turn from its notice, which comes first.
    let client = daemon.client().await.unwrap();
    let history = daemon.events(&client, conversation).await.unwrap();
    let first_of_wake = history
        .iter()
        .find(|envelope| envelope.event.turn_id() == Some(wake))
        .map(|envelope| envelope.event.kind());
    assert_eq!(first_of_wake, Some("job_notice"));
    assert!(!queued(&history, wake));
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn watch_match_wakes_model() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = blocked(&fifo, "true");
    let line = format!("echo 'listening on port 3000'; {line}");
    let model = Recorder::new(vec![
        Some((
            "shell",
            json!({ "command": line, "background": true, "watch": { "pattern": "port [0-9]+" } }),
        )),
        None,
        None,
    ]);
    let daemon = setup.start(Arc::clone(&model)).await;
    let sent = prompt(&daemon, 1, "start the server and wake me when it listens", Mode::Auto).await;
    let events = until_turn_end(&daemon, sent.conversation_id, 2).await;
    let [_, wake] = turn_starts(&events).try_into().unwrap();
    let [(turn, job_id, kind, woke, text)] = notices(&events).try_into().unwrap();
    assert_eq!((turn, job_id.as_str(), kind, woke), (wake, "j1", NoticeKind::Watch, true));
    assert!(text.contains("state=\"running\" watch=\"matched\""), "{text}");
    assert!(text.contains("matched: listening on port 3000\nwakes left: 0\n"), "{text}");
    let firsts = model.firsts();
    assert_eq!(firsts[1][1..], [text], "the wake turn's first message holds the match");
    // NOTE: the job still runs; it ends at efrd's stop, which wakes nobody.
    let client = daemon.client().await.unwrap();
    let list: efr_protocol::JobsListResult = client
        .call(Method::JobsList(efr_protocol::JobsList {
            conversation_id: Some(sent.conversation_id),
        }))
        .await
        .unwrap();
    assert_eq!(list.jobs[0].state, JobState::Running);
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn wake_limit_six_per_hour() {
    let setup = Setup::new();
    let (one, two) = (setup.fifo("one"), setup.fifo("two"));
    let model = Recorder::new(vec![
        job(&blocked(&one, "echo first")),
        job(&blocked(&two, "echo second")),
        None,
        // The one wake turn of the hour.
        None,
        // The next prompt's turn.
        None,
    ]);
    let daemon = setup
        .builder(Arc::clone(&model))
        .config(|config| {
            assert_eq!(config.jobs.max_wakes_per_hour, 6, "the default limit");
            config.jobs.max_wakes_per_hour = 1;
        })
        .start()
        .await
        .unwrap();
    let sent = prompt(&daemon, 1, "start two jobs", Mode::Auto).await;
    let conversation = sent.conversation_id;
    release(&one).await;
    until_turn_end(&daemon, conversation, 2).await;
    // NOTE: a minute later the window still holds the first wake turn.
    daemon.clock().advance(Duration::from_secs(60));
    release(&two).await;
    let events = until_held(&daemon, conversation, "j2").await;
    assert_eq!(held(&events), [("j2".to_owned(), HeldReason::WakeLimit)]);
    assert_eq!(turn_starts(&events).len(), 2, "no second wake turn");
    let sent = prompt(&daemon, 2, "what happened?", Mode::Auto).await;
    let events = until_turn_end(&daemon, sent.conversation_id, 3).await;
    let starts = turn_starts(&events);
    let last = *starts.last().unwrap();
    assert!(queued(&events, last), "a prompt started the third turn");
    let taken: Vec<(TurnId, String, bool)> =
        notices(&events).into_iter().map(|(turn, job, _, woke, _)| (turn, job, woke)).collect();
    assert_eq!(taken, [(starts[1], "j1".to_owned(), true), (last, "j2".to_owned(), false)]);
    let firsts = model.firsts();
    let third = firsts.last().unwrap();
    assert_eq!(third[1], "what happened?");
    assert!(third[2].starts_with("<job id=\"j2\""), "{third:?}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn wake_chain_stops_after_one_turn() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let model = Recorder::new(vec![
        job(&blocked(&fifo, "echo first")),
        None,
        // The wake turn starts a job of its own.
        job("echo from-the-wake-turn"),
        None,
        // The next prompt's turn.
        None,
    ]);
    let daemon = setup.start(Arc::clone(&model)).await;
    let sent = prompt(&daemon, 1, "start it", Mode::Auto).await;
    let conversation = sent.conversation_id;
    release(&fifo).await;
    let events = until_held(&daemon, conversation, "j2").await;
    assert_eq!(held(&events), [("j2".to_owned(), HeldReason::WakeTurnJob)]);
    let starts = turn_starts(&events);
    assert_eq!(starts.len(), 2, "the job of the wake turn woke nobody");
    let started_in = events.iter().find_map(|envelope| match &envelope.event {
        Event::JobStarted { job, turn_id, .. } if job.id.as_str() == "j2" => Some(*turn_id),
        _ => None,
    });
    assert_eq!(started_in, Some(starts[1]));
    let sent = prompt(&daemon, 2, "and now?", Mode::Auto).await;
    let events = until_turn_end(&daemon, sent.conversation_id, 3).await;
    let last = *turn_starts(&events).last().unwrap();
    let taken: Vec<(TurnId, String, bool)> =
        notices(&events).into_iter().map(|(turn, job, _, woke, _)| (turn, job, woke)).collect();
    assert_eq!(taken[1..], [(last, "j2".to_owned(), false)]);
    assert!(model.firsts()[2][2].contains("from-the-wake-turn"));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn wake_turn_follows_lowered_mode() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let model = Recorder::new(vec![
        job(&blocked(&fifo, "echo done")),
        None,
        // The `manual` turn: no call.
        None,
        // The next `auto` prompt.
        None,
    ]);
    let daemon = setup.start(Arc::clone(&model)).await;
    let sent = prompt(&daemon, 1, "start it", Mode::Auto).await;
    let conversation = sent.conversation_id;
    prompt(&daemon, 2, "now be careful", Mode::Manual).await;
    release(&fifo).await;
    let events = until_held(&daemon, conversation, "j1").await;
    assert_eq!(held(&events), [("j1".to_owned(), HeldReason::NotAuto)]);
    assert_eq!(turn_starts(&events).len(), 2, "no wake turn after the lowered mode");
    prompt(&daemon, 3, "back to auto", Mode::Auto).await;
    let events = until_turn_end(&daemon, conversation, 3).await;
    let last = *turn_starts(&events).last().unwrap();
    let taken: Vec<(TurnId, bool)> =
        notices(&events).into_iter().map(|(turn, _, _, woke, _)| (turn, woke)).collect();
    assert_eq!(taken, [(last, false)]);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn restart_notice_does_not_wake() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let model = Recorder::new(vec![
        job(&blocked(&fifo, "true")),
        None,
        // The next prompt's turn after the restart.
        None,
    ]);
    let mut daemon = setup.builder(Arc::clone(&model)).persistent().start().await.unwrap();
    let sent = prompt(&daemon, 1, "start it", Mode::Auto).await;
    let conversation = sent.conversation_id;
    daemon.restart().await.unwrap();
    let events = until_held(&daemon, conversation, "j1").await;
    assert_eq!(held(&events), [("j1".to_owned(), HeldReason::DaemonRestart)]);
    let ended = events.iter().find_map(|envelope| match &envelope.event {
        Event::JobEnded { state, stopped_by, .. } => Some((*state, *stopped_by)),
        _ => None,
    });
    assert_eq!(ended, Some((JobState::Stopped, Some(efr_protocol::JobStopBy::DaemonStop))));
    assert_eq!(turn_starts(&events).len(), 1, "the notice of a stopped job woke nobody");
    assert_eq!(model.firsts().len(), 1, "no request after the restart");
    prompt(&daemon, 2, "what happened to the job?", Mode::Auto).await;
    let events = until_turn_end(&daemon, conversation, 2).await;
    let last = *turn_starts(&events).last().unwrap();
    let taken: Vec<(TurnId, bool)> =
        notices(&events).into_iter().map(|(turn, _, _, woke, _)| (turn, woke)).collect();
    assert_eq!(taken, [(last, false)]);
    let firsts = model.firsts();
    let [_, after] = firsts.try_into().unwrap();
    assert_eq!(after[1], "what happened to the job?");
    assert!(after[2].contains("stopped_by=\"daemon_stop\""), "{after:?}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn away_wake_turn_question_expires() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let outside = json!({
        "command": "echo hi",
        "needs": { "outside": true, "reason": "to look at the result" }
    });
    let model = Recorder::reviewed(
        vec![
            job(&blocked(&fifo, "echo ended")),
            None,
            // The wake turn asks: the reviewer hands the exit to the user.
            Some(("shell", outside)),
            None,
        ],
        REVIEWER_ASKS,
    );
    let daemon = setup
        .builder(Arc::clone(&model))
        .config(|config| config.presence.away_wait_secs = 30)
        .start()
        .await
        .unwrap();
    let sent = prompt(&daemon, 1, "start it", Mode::Auto).await;
    let conversation = sent.conversation_id;
    release(&fifo).await;
    let events =
        until(&daemon, conversation, |event| matches!(event, Event::ApprovalRequested { .. }))
            .await;
    let Some(Event::ApprovalRequested { summary, .. }) = events.last().map(|e| e.event.clone())
    else {
        unreachable!()
    };
    assert!(summary.starts_with("(job notice turn) "), "{summary}");
    // NOTE: nobody that can answer follows the conversation, so the question gets the
    // away wait on the test clock; the clock moves until it expires.
    let client = daemon.client().await.unwrap();
    let mut follow = daemon.follow(&client, conversation).await.unwrap();
    let expired = events_until(&mut follow, |event| matches!(event, Event::ApprovalExpired { .. }));
    tokio::pin!(expired);
    let events = loop {
        tokio::select! {
            events = &mut expired => break events.unwrap(),
            () = efr_stdx::time::SystemClock.sleep(Duration::from_millis(20)) => {
                daemon.clock().advance(Duration::from_secs(5));
            }
        }
    };
    let away = events.iter().find_map(|envelope| match &envelope.event {
        Event::ApprovalExpired { away, .. } => Some(*away),
        _ => None,
    });
    assert_eq!(away, Some(true));
    let events = until_turn_end(&daemon, conversation, 2).await;
    let outputs: Vec<&str> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { output, .. } => Some(output.as_str()),
            _ => None,
        })
        .collect();
    assert!(outputs[1].starts_with("Nobody could approve this now"), "{outputs:?}");
    assert!(!events.iter().any(|e| matches!(e.event, Event::ApprovalResolved { .. })));
    drop(client);
    daemon.stop().await.unwrap();
}
