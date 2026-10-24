//! Background jobs end to end through a `TestDaemon`: the shared fake launcher of
//! `efr-test-daemon` (a `/bin/sh` script, no bubblewrap, no zsh), the `job_env` seam in
//! place of the hidden shell's `env` file, a scripted model and the `TestClock`. A job
//! that must outlive its turn blocks on a FIFO that the test writes after the turn's
//! end; no test waits on a fixed time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::{
    ApprovalDecision, ConversationId, Event, EventEnvelope, JobId, JobInfo, JobState, JobStopBy,
    JobsList, JobsListResult, JobsStop, JobsStopResult, Launch, Method, Mode, PromptSendResult,
    TurnInterrupt, TurnSettings,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_stdx::time::Clock as _;
use efr_test_daemon::{
    FakeLauncher, PeerSide, TTY, TestDaemon, TestDaemonBuilder, command_id, events_until,
};
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::sandbox::{REVIEWER_ALLOWS, ready};

/// A gate before a step of the model: the step waits until it says yes.
type Gate = Arc<dyn Fn() -> bool + Send + Sync>;

/// A model that calls a tool for each step, after the step's gate, then says `done`.
/// The reviewer allows every exit.
struct JobModel {
    id: ProviderId,
    steps: Vec<(String, Value, Option<Gate>)>,
    asked: AtomicUsize,
}

impl std::fmt::Debug for JobModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobModel").field("steps", &self.steps.len()).finish()
    }
}

impl JobModel {
    fn new(steps: Vec<(&str, Value, Option<Gate>)>) -> Arc<Self> {
        Arc::new(JobModel {
            id: ProviderId::new("test").unwrap(),
            steps: steps
                .into_iter()
                .map(|(name, input, gate)| (name.to_owned(), input, gate))
                .collect(),
            asked: AtomicUsize::new(0),
        })
    }

    /// Calls `shell` with each input in turn.
    fn shell(inputs: Vec<Value>) -> Arc<Self> {
        JobModel::new(inputs.into_iter().map(|input| ("shell", input, None)).collect())
    }
}

#[async_trait]
impl Provider for JobModel {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        if request.one_shot {
            let events = vec![
                ProviderEvent::TextDelta { text: REVIEWER_ALLOWS.to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ];
            return Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))));
        }
        let n = self.asked.fetch_add(1, Ordering::SeqCst);
        let events = match self.steps.get(n) {
            Some((name, _, _)) if name == "done" => vec![
                ProviderEvent::TextDelta { text: "done".to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ],
            Some((name, input, gate)) => {
                if let Some(gate) = gate {
                    // NOTE: a real-time poll: the provider's future must be Send, and
                    // the gate is a file or an event of a process that the test started.
                    let mut waited = 0;
                    while !gate() {
                        assert!(waited < 1000, "the model's gate stayed shut for 10 s");
                        efr_stdx::time::SystemClock.sleep(Duration::from_millis(10)).await;
                        waited += 1;
                    }
                }
                let call_id = format!("call_{n}");
                vec![
                    ProviderEvent::ToolCallStart {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        freeform: false,
                    },
                    ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() },
                    ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None },
                ]
            }
            None => vec![
                ProviderEvent::TextDelta { text: "done".to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ],
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// The environment of every job in these tests.
fn job_env(dirs: &TestDirs) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ("HOME".to_owned(), dirs.home().display().to_string()),
    ])
}

/// A test's daemon with the fake launcher, a ready sandbox, a fixed job environment
/// and `model`; `change` sets the config.
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

    fn builder(&self, model: Arc<dyn Provider>) -> TestDaemonBuilder {
        TestDaemon::builder()
            .dirs(Arc::clone(&self.dirs))
            .sandbox_launcher(self.fake.path())
            .probe_override(ready())
            .job_env(job_env(&self.dirs))
            .custom_provider(model)
    }

    async fn start(&self, model: Arc<dyn Provider>) -> TestDaemon {
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

/// A background `shell` input that runs `command`.
fn background(command: &str) -> Value {
    json!({ "command": command, "background": true })
}

/// Sends `text` in `mode` from the shell of `tty` and returns the turn's events up to
/// its end; every approval gets `decision`.
async fn turn_on(
    daemon: &TestDaemon,
    tty: &str,
    text: &str,
    mode: Mode,
    decision: ApprovalDecision,
) -> (PromptSendResult, Vec<EventEnvelope>) {
    crate::sandbox::run_turn_on(daemon, tty, text, mode, decision).await
}

/// [`turn_on`] with the command id `n`, so a second terminal's prompt is its own.
async fn turn_on_n(
    daemon: &TestDaemon,
    tty: &str,
    n: u128,
    text: &str,
    mode: Mode,
) -> (PromptSendResult, Vec<EventEnvelope>) {
    crate::sandbox::run_turn_with(daemon, tty, n, text, mode, ApprovalDecision::Deny).await
}

/// The events of the second turn of the test terminal's conversation, a prompt with the
/// command id `n` in `auto`.
async fn next_turn(daemon: &TestDaemon, n: u128, text: &str) -> Vec<EventEnvelope> {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(n, text, TTY) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let mut ends = 0;
    let events = until(daemon, sent.conversation_id, |event| {
        if matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. }) {
            ends += 1;
        }
        ends == 2
    })
    .await;
    let first_end = events
        .iter()
        .position(|envelope| matches!(envelope.event, Event::TurnCompleted { .. }))
        .unwrap();
    events[first_end + 1..].to_vec()
}

/// [`turn_on`] from the test's terminal in `auto`.
async fn auto_turn(daemon: &TestDaemon, text: &str) -> (PromptSendResult, Vec<EventEnvelope>) {
    turn_on(daemon, TTY, text, Mode::Auto, ApprovalDecision::Deny).await
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

/// The `job_ended` of `id` in `conversation`, once it comes.
async fn ended(daemon: &TestDaemon, conversation: ConversationId, id: &str) -> Event {
    let events = until(
        daemon,
        conversation,
        |event| matches!(event, Event::JobEnded { job_id, .. } if job_id.as_str() == id),
    )
    .await;
    events.last().unwrap().event.clone()
}

/// Each job that a `job_started` of `events` names.
fn started(events: &[EventEnvelope]) -> Vec<JobInfo> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::JobStarted { job, .. } => Some(job.clone()),
            _ => None,
        })
        .collect()
}

/// The output of each finished tool call of `events`, with whether it failed.
fn outputs(events: &[EventEnvelope]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { output, is_error, .. } => Some((output.clone(), *is_error)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn shell_background_returns_at_once_with_id_and_file() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("echo first-line; read answer < {}; echo got-$answer", fifo.display());
    let daemon = setup.start(JobModel::shell(vec![background(&line)])).await;
    let (sent, events) = auto_turn(&daemon, "start the server").await;
    let [(output, is_error)] = outputs(&events).try_into().unwrap();
    assert!(!is_error, "{output}");
    assert!(output.starts_with("job j1 started: echo first-line;"), "{output}");
    let file = setup.dirs.dirs().data().join("scratch");
    assert!(output.contains("/jobs/j1.out"), "{output}");
    assert!(output.contains("time limit: 60 minutes"), "{output}");
    let [job] = started(&events).try_into().unwrap();
    assert_eq!(job.id, JobId::nth(1));
    assert_eq!(job.state, JobState::Running);
    assert_eq!(job.launch, Launch::contained());
    assert!(job.output.starts_with(&file), "{}", job.output.display());
    // NOTE: the turn ended while the job runs: the job blocks on the FIFO.
    let running: JobsListResult = daemon
        .client()
        .await
        .unwrap()
        .call(Method::JobsList(JobsList { conversation_id: Some(sent.conversation_id) }))
        .await
        .unwrap();
    assert_eq!(running.jobs[0].state, JobState::Running);
    release(&fifo).await;
    let end = ended(&daemon, sent.conversation_id, "j1").await;
    let Event::JobEnded { state, exit_code, stopped_by, tail, launcher_lost, .. } = end else {
        unreachable!()
    };
    assert_eq!((state, exit_code, stopped_by), (JobState::Exited, Some(0), None));
    assert_eq!(tail, "first-line\ngot-go");
    assert!(!launcher_lost);
    let text = std::fs::read_to_string(&job.output).unwrap();
    assert_eq!(text, "first-line\ngot-go\n");
    let [spec] = specs(setup.fake.dir()).try_into().unwrap();
    assert!(spec.runtime.call_dir.parent().unwrap().ends_with("jobs"));
    assert!(spec.job.is_some());
    Wait::new("the job's dirs go").until(|| !spec.runtime.call_dir.exists()).await.unwrap();
    daemon.stop().await.unwrap();
}

/// The specs that the fake launcher kept, one per call or job.
fn specs(fake: &Path) -> Vec<efr_sandbox::SandboxSpec> {
    let mut specs: Vec<efr_sandbox::SandboxSpec> = std::fs::read_dir(fake)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("spec-"))
        .map(|entry| {
            efr_sandbox::SandboxSpec::from_json(&std::fs::read(entry.path()).unwrap()).unwrap()
        })
        .collect();
    specs.sort_by_key(|spec| spec.call);
    specs
}

/// The seq of the first event of `events` that `find` accepts.
fn seq_of(events: &[EventEnvelope], find: impl Fn(&Event) -> bool) -> u64 {
    events.iter().find(|envelope| find(&envelope.event)).unwrap().seq.get()
}

fn kinds(events: &[EventEnvelope]) -> Vec<&str> {
    events.iter().map(|envelope| envelope.event.kind()).collect()
}

/// Every file named `name` below `root`.
fn files_named(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if entry.file_name() == name {
                found.push(path);
            }
        }
    }
    found
}

/// True once the output file `name` below `root` holds `text`.
fn output_holds(root: PathBuf, name: &'static str, text: &'static str) -> Gate {
    Arc::new(move || {
        files_named(&root, name)
            .iter()
            .any(|file| std::fs::read_to_string(file).is_ok_and(|held| held.contains(text)))
    })
}

#[tokio::test]
async fn job_started_recorded_before_tool_returns() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("read answer < {}", fifo.display());
    let daemon = setup.start(JobModel::shell(vec![background(&line)])).await;
    let (sent, events) = auto_turn(&daemon, "start it").await;
    let started_at = seq_of(&events, |event| matches!(event, Event::JobStarted { .. }));
    let completed = seq_of(&events, |event| matches!(event, Event::ToolCallCompleted { .. }));
    assert!(
        started_at < completed,
        "job_started {started_at} comes before the call's end {completed}"
    );
    let start = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::JobStarted { launcher_pid, launcher_start, call_id, .. } => {
                Some((*launcher_pid, *launcher_start, *call_id))
            }
            _ => None,
        })
        .unwrap();
    assert!(start.0 > 0 && start.1 > 0, "{start:?}");
    let call = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { call_id, .. } => Some(*call_id),
        _ => None,
    });
    assert_eq!(call, Some(start.2));
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_ends_at_time_limit() {
    let setup = Setup::new();
    let input = json!({ "command": "sleep 30", "background": true, "max_minutes": 1 });
    let daemon = setup.start(JobModel::shell(vec![input])).await;
    let (sent, events) = auto_turn(&daemon, "wait a bit").await;
    let [job] = started(&events).try_into().unwrap();
    assert_eq!(job.ends_by.as_second() - job.started_at.as_second(), 60);
    daemon.clock().set(job.ends_by);
    let Event::JobEnded { state, stopped_by, exit_code, .. } =
        ended(&daemon, sent.conversation_id, "j1").await
    else {
        unreachable!()
    };
    assert_eq!(
        (state, stopped_by, exit_code),
        (JobState::TimedOut, Some(JobStopBy::TimeLimit), None)
    );
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_default_limit_60_minutes() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("read answer < {}", fifo.display());
    let daemon = setup.start(JobModel::shell(vec![background(&line)])).await;
    let (sent, events) = auto_turn(&daemon, "start it").await;
    let [job] = started(&events).try_into().unwrap();
    assert_eq!(job.ends_by.as_second() - job.started_at.as_second(), 60 * 60);
    let [spec] = specs(setup.fake.dir()).try_into().unwrap();
    let plan = spec.job.unwrap();
    assert_eq!((plan.max_seconds, plan.backstop_secs), (3600, efr_sandbox::JOB_BACKSTOP_SECS));
    assert_eq!(plan.nice, 19);
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_max_minutes_capped_at_480() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("read answer < {}", fifo.display());
    let input = json!({ "command": line, "background": true, "max_minutes": 1000 });
    let daemon = setup.start(JobModel::shell(vec![input])).await;
    let (sent, events) = auto_turn(&daemon, "start it").await;
    let [job] = started(&events).try_into().unwrap();
    assert_eq!(job.ends_by.as_second() - job.started_at.as_second(), 480 * 60);
    let [(output, _)] = outputs(&events).try_into().unwrap();
    assert!(output.contains("time limit: 480 minutes (the most that efr allows)"), "{output}");
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_stop_returns_tail() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("echo line-one; echo line-two; read answer < {}", fifo.display());
    let scratch = setup.dirs.dirs().data().join("scratch");
    let model = JobModel::new(vec![
        ("shell", background(&line), None),
        ("job_stop", json!({ "id": "j1" }), Some(output_holds(scratch, "j1.out", "line-two"))),
    ]);
    let daemon = setup.start(model).await;
    let (sent, events) = auto_turn(&daemon, "start it, then stop it").await;
    let [_, (stopped, is_error)] = outputs(&events).try_into().unwrap();
    assert!(!is_error, "{stopped}");
    assert!(stopped.starts_with("job j1 stopped\n"), "{stopped}");
    assert!(stopped.ends_with("line-one\nline-two"), "{stopped}");
    let Event::JobEnded { state, stopped_by, tail, .. } =
        ended(&daemon, sent.conversation_id, "j1").await
    else {
        unreachable!()
    };
    assert_eq!((state, stopped_by), (JobState::Stopped, Some(JobStopBy::Model)));
    assert_eq!(tail, "line-one\nline-two");
    daemon.stop().await.unwrap();
}

/// Starts a job that blocks on a FIFO, stops efrd while it runs, and starts efrd again
/// on the same tree; returns the conversation's events after the restart.
async fn stopped_by_efrd(setup: &Setup) -> Vec<EventEnvelope> {
    let fifo = setup.fifo("go");
    let line = format!("echo before-the-stop; read answer < {}", fifo.display());
    let model = JobModel::shell(vec![background(&line)]);
    let mut daemon = setup.builder(model).persistent().start().await.unwrap();
    let (sent, _) = auto_turn(&daemon, "start it").await;
    let scratch = setup.dirs.dirs().data().join("scratch");
    let printed = output_holds(scratch, "j1.out", "before-the-stop");
    Wait::new("the job's first line").until(|| printed()).await.unwrap();
    daemon.restart().await.unwrap();
    let client = daemon.client().await.unwrap();
    let events = daemon.events(&client, sent.conversation_id).await.unwrap();
    drop(client);
    daemon.stop().await.unwrap();
    events
}

#[tokio::test]
async fn job_ends_at_efrd_stop() {
    let setup = Setup::new();
    let events = stopped_by_efrd(&setup).await;
    let ends: Vec<(JobState, Option<JobStopBy>)> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::JobEnded { state, stopped_by, .. } => Some((*state, *stopped_by)),
            _ => None,
        })
        .collect();
    // NOTE: one end, at the stop; the restart finds no job that still runs.
    assert_eq!(ends, [(JobState::Stopped, Some(JobStopBy::DaemonStop))]);
    assert!(!kinds(&events).contains(&"job_notice"), "no turn took a notice");
}

#[tokio::test]
async fn job_tasks_not_cancelled_before_stop_all() {
    let setup = Setup::new();
    let events = stopped_by_efrd(&setup).await;
    let tail = events.iter().find_map(|envelope| match &envelope.event {
        Event::JobEnded { tail, .. } => Some(tail.clone()),
        _ => None,
    });
    assert_eq!(tail.as_deref(), Some("before-the-stop"));
}

#[tokio::test]
async fn job_ids_continue_after_restart() {
    let setup = Setup::new();
    let model = JobModel::new(vec![
        ("shell", background("true"), None),
        ("done", Value::Null, None),
        ("shell", background("true"), None),
    ]);
    // NOTE: the end of `j1` finds the conversation idle. With wakes on, it starts a
    // wake turn that races the restart: the wake turn can take the model's next
    // step, or the restart can cancel it before its job starts.
    let mut daemon = setup
        .builder(model)
        .config(|config| config.jobs.max_wakes_per_hour = 0)
        .persistent()
        .start()
        .await
        .unwrap();
    let (sent, events) = auto_turn(&daemon, "a quick job").await;
    assert_eq!(started(&events)[0].id, JobId::nth(1));
    ended(&daemon, sent.conversation_id, "j1").await;
    daemon.restart().await.unwrap();
    let events = next_turn(&daemon, 2, "one more").await;
    let ids: Vec<String> = started(&events).iter().map(|job| job.id.to_string()).collect();
    assert_eq!(ids, ["j2"], "an id never repeats after a restart");
    ended(&daemon, sent.conversation_id, "j2").await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn two_starts_in_one_turn_get_two_ids() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("read answer < {}", fifo.display());
    let model = JobModel::shell(vec![background(&line), background("echo second")]);
    let daemon = setup.start(model).await;
    let (sent, events) = auto_turn(&daemon, "two jobs").await;
    let ids: Vec<String> = started(&events).iter().map(|job| job.id.to_string()).collect();
    assert_eq!(ids, ["j1", "j2"]);
    let outputs = outputs(&events);
    assert!(outputs[0].0.starts_with("job j1 started"), "{}", outputs[0].0);
    assert!(outputs[1].0.starts_with("job j2 started"), "{}", outputs[1].0);
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_limit_per_conversation() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("read answer < {}", fifo.display());
    let model = JobModel::shell(vec![background(&line), background("echo second")]);
    let daemon =
        setup.builder(model).config(|config| config.jobs.max_running = 1).start().await.unwrap();
    let (sent, events) = auto_turn(&daemon, "two jobs").await;
    let [(first, _), (second, is_error)] = outputs(&events).try_into().unwrap();
    assert!(first.starts_with("job j1 started"), "{first}");
    assert!(is_error, "{second}");
    assert!(second.contains("1 job runs in this conversation"), "{second}");
    assert!(second.contains("j1 running: read answer"), "{second}");
    assert!(second.contains("60 minutes left"), "{second}");
    assert_eq!(started(&events).len(), 1);
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_limit_per_daemon() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("read answer < {}", fifo.display());
    let model = JobModel::new(vec![
        ("shell", background(&line), None),
        ("done", Value::Null, None),
        ("shell", background("echo second"), None),
    ]);
    let daemon = setup
        .builder(model)
        .config(|config| config.jobs.max_running_daemon = 1)
        .start()
        .await
        .unwrap();
    let (first, events) =
        turn_on(&daemon, TTY, "a job here", Mode::Auto, ApprovalDecision::Deny).await;
    assert_eq!(started(&events).len(), 1);
    let other = "/dev/pts/efr-other";
    let (second, events) = turn_on_n(&daemon, other, 2, "a job there", Mode::Auto).await;
    assert_ne!(first.conversation_id, second.conversation_id);
    let [(output, is_error)] = outputs(&events).try_into().unwrap();
    assert!(is_error, "{output}");
    assert!(output.contains("1 job runs in efr"), "{output}");
    assert!(started(&events).is_empty());
    release(&fifo).await;
    ended(&daemon, first.conversation_id, "j1").await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn jobs_list_shows_time_left() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("echo listed; read answer < {}", fifo.display());
    let input = json!({ "command": line, "background": true, "max_minutes": 30 });
    let model = JobModel::new(vec![("shell", input, None), ("jobs", json!({}), None)]);
    let daemon = setup.start(model).await;
    let (sent, events) = auto_turn(&daemon, "start and list").await;
    let [_, (listed, _)] = outputs(&events).try_into().unwrap();
    assert!(listed.starts_with("j1 running: echo listed;"), "{listed}");
    assert!(listed.contains("30 minutes left"), "{listed}");
    let client = daemon.client().await.unwrap();
    let list: JobsListResult = client
        .call(Method::JobsList(JobsList { conversation_id: Some(sent.conversation_id) }))
        .await
        .unwrap();
    let [job] = list.jobs.try_into().unwrap();
    assert_eq!(job.state, JobState::Running);
    assert_eq!(job.ends_by.as_second() - job.started_at.as_second(), 30 * 60);
    let all: JobsListResult =
        client.call(Method::JobsList(JobsList { conversation_id: None })).await.unwrap();
    assert_eq!(all.jobs.len(), 1);
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn jobs_read_output_offset_and_tail() {
    let setup = Setup::new();
    let line = "i=1; while [ $i -le 300 ]; do echo line-$i; i=$((i+1)); done";
    let scratch = setup.dirs.dirs().data().join("scratch");
    let model = JobModel::new(vec![
        ("shell", background(line), None),
        (
            "jobs",
            json!({ "id": "j1", "tail": 3 }),
            Some(output_holds(scratch, "j1.out", "line-300\n")),
        ),
        ("jobs", json!({ "id": "j1", "offset": 7 }), None),
        ("jobs", json!({ "id": "j1" }), None),
    ]);
    let daemon = setup.start(model).await;
    let (_, events) = auto_turn(&daemon, "count").await;
    let [_, (tail, _), (offset, _), (default, _)] = outputs(&events).try_into().unwrap();
    assert!(tail.ends_with("\nline-298\nline-299\nline-300"), "{tail}");
    let offset_text = offset.split_once('\n').unwrap().1;
    assert!(offset_text.starts_with("line-2\nline-3\n"), "{offset}");
    let shown = default.split_once('\n').unwrap().1;
    assert_eq!(shown.lines().count(), 50);
    assert!(shown.starts_with("line-251\n"), "{shown}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn watch_max_wakes_respected() {
    let setup = Setup::new();
    let input = json!({
        "command": "echo error one; echo fine; echo error two; echo error three",
        "background": true,
        "watch": { "pattern": "^error", "max_wakes": 2 }
    });
    let daemon = setup.start(JobModel::shell(vec![input])).await;
    let (sent, events) = auto_turn(&daemon, "watch it").await;
    let [job] = started(&events).try_into().unwrap();
    assert_eq!(job.watch.unwrap().max_wakes, 2);
    // NOTE: every match of a job is recorded before its end.
    let events =
        until(&daemon, sent.conversation_id, |event| matches!(event, Event::JobEnded { .. })).await;
    let matched: Vec<(String, u8)> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::JobWatchMatched { line, wakes_left, .. } => Some((line.clone(), *wakes_left)),
            _ => None,
        })
        .collect();
    assert_eq!(matched, [("error one".to_owned(), 1), ("error two".to_owned(), 0)]);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn watch_match_wakes_model() {
    // NOTE: the daemon half: a match is recorded and its notice goes to the actor; the
    // actor's wake turn is the conversation crate's.
    let setup = Setup::new();
    let input = json!({
        "command": "echo ready on port 3000",
        "background": true,
        "watch": { "pattern": "port [0-9]+" }
    });
    let daemon = setup.start(JobModel::shell(vec![input])).await;
    let (sent, _) = auto_turn(&daemon, "watch it").await;
    let events = until(&daemon, sent.conversation_id, |event| {
        matches!(event, Event::JobWatchMatched { .. })
    })
    .await;
    let Event::JobWatchMatched { job_id, line, wakes_left } = events.last().unwrap().event.clone()
    else {
        unreachable!()
    };
    assert_eq!((job_id.as_str(), line.as_str(), wakes_left), ("j1", "ready on port 3000", 0));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn jobs_disabled_refuses_background() {
    let setup = Setup::new();
    let daemon = setup
        .builder(JobModel::shell(vec![background("echo hi")]))
        .config(|config| config.jobs.enabled = false)
        .start()
        .await
        .unwrap();
    let (_, events) = auto_turn(&daemon, "start it").await;
    let [(output, is_error)] = outputs(&events).try_into().unwrap();
    assert!(is_error);
    assert!(output.contains("jobs.enabled = false"), "{output}");
    assert!(started(&events).is_empty());
    assert!(specs(setup.fake.dir()).is_empty(), "no launcher ran");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_in_manual_starts_after_approval_in_exit_child() {
    // NOTE: outside `auto` the user answers before every job, and an approved job runs
    // in the exit child, with no sandbox, as an approved line in the hidden shell would.
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("read answer < {}", fifo.display());
    let daemon = setup.start(JobModel::shell(vec![background(&line)])).await;
    let (sent, events) =
        turn_on(&daemon, TTY, "start it", Mode::Manual, ApprovalDecision::Allow).await;
    assert!(kinds(&events).contains(&"approval_requested"), "{:?}", kinds(&events));
    let [job] = started(&events).try_into().unwrap();
    assert_eq!(job.launch, Launch::Unsandboxed);
    let [spec] = specs(setup.fake.dir()).try_into().unwrap();
    assert_eq!(spec.launch, efr_sandbox::SpecLaunch::Unsandboxed);
    release(&fifo).await;
    let Event::JobEnded { state, .. } = ended(&daemon, sent.conversation_id, "j1").await else {
        unreachable!()
    };
    assert_eq!(state, JobState::Exited);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_in_manual_denied_starts_no_job() {
    let setup = Setup::new();
    let daemon = setup.start(JobModel::shell(vec![background("echo hi")])).await;
    let (_, events) = turn_on(&daemon, TTY, "start it", Mode::Manual, ApprovalDecision::Deny).await;
    assert!(kinds(&events).contains(&"approval_requested"));
    assert!(started(&events).is_empty());
    assert!(specs(setup.fake.dir()).is_empty(), "no launcher ran");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn esc_stops_foreground_not_jobs() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("read answer < {}", fifo.display());
    let shut = setup.dirs.root().join("never");
    let never: Gate = Arc::new(move || shut.exists());
    let model = JobModel::new(vec![
        ("shell", background(&line), None),
        ("shell", json!({ "command": "echo later" }), Some(never)),
    ]);
    let daemon = setup.start(model).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(1, "start it", TTY) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    until(&daemon, sent.conversation_id, |event| matches!(event, Event::ToolCallCompleted { .. }))
        .await;
    let interrupt = Method::TurnInterrupt(TurnInterrupt {
        command_id: command_id(50),
        conversation_id: sent.conversation_id,
        turn_id: None,
        resend_steers: Vec::new(),
        resend_as: None,
        withdraw_steers: Vec::new(),
        withdraw: Vec::new(),
    });
    let _: Value = client.call(interrupt).await.unwrap();
    until(&daemon, sent.conversation_id, |event| matches!(event, Event::TurnInterrupted { .. }))
        .await;
    let list: JobsListResult = client
        .call(Method::JobsList(JobsList { conversation_id: Some(sent.conversation_id) }))
        .await
        .unwrap();
    assert_eq!(list.jobs[0].state, JobState::Running, "Esc stops the turn, never a job");
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn esc_while_job_starts_leaves_whole_job() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("read answer < {}", fifo.display());
    setup.fake.set("hold-start").unwrap();
    let daemon = setup.start(JobModel::shell(vec![background(&line)])).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(1, "start it", TTY) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let held = setup.fake.dir().join("held");
    Wait::new("the launcher between its start byte and its start")
        .until(|| held.exists())
        .await
        .unwrap();
    let interrupt = Method::TurnInterrupt(TurnInterrupt {
        command_id: command_id(50),
        conversation_id: sent.conversation_id,
        turn_id: None,
        resend_steers: Vec::new(),
        resend_as: None,
        withdraw_steers: Vec::new(),
        withdraw: Vec::new(),
    });
    let _: Value = client.call(interrupt).await.unwrap();
    until(&daemon, sent.conversation_id, |event| matches!(event, Event::TurnInterrupted { .. }))
        .await;
    std::fs::remove_file(setup.fake.dir().join("hold-start")).unwrap();
    // NOTE: the start goes on without the call: the job is recorded and runs, as a job
    // does after Esc, and its launcher is not left behind with no job.
    until(&daemon, sent.conversation_id, |event| matches!(event, Event::JobStarted { .. })).await;
    let list: JobsListResult = client
        .call(Method::JobsList(JobsList { conversation_id: Some(sent.conversation_id) }))
        .await
        .unwrap();
    assert_eq!(list.jobs[0].state, JobState::Running);
    release(&fifo).await;
    let Event::JobEnded { state, .. } = ended(&daemon, sent.conversation_id, "j1").await else {
        unreachable!()
    };
    assert_eq!(state, JobState::Exited);
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn jobs_stop_ends_a_job_for_the_user() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("echo stopping; read answer < {}", fifo.display());
    let daemon = setup.start(JobModel::shell(vec![background(&line)])).await;
    let (sent, _) = auto_turn(&daemon, "start it").await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let stop = |n, job| {
        Method::JobsStop(JobsStop {
            command_id: command_id(n),
            conversation_id: None,
            job_id: JobId::nth(job),
        })
    };
    let stopped: JobsStopResult = client.call(stop(60, 1)).await.unwrap();
    assert_eq!(
        (stopped.job.state, stopped.job.stopped_by),
        (JobState::Stopped, Some(JobStopBy::User))
    );
    let again: JobsStopResult = client.call(stop(61, 1)).await.unwrap();
    assert_eq!(again.job.state, JobState::Stopped, "a job that ended keeps its state");
    let unknown = client.call::<JobsStopResult>(stop(62, 9)).await;
    assert!(unknown.is_err(), "{unknown:?}");
    let _ = sent;
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn delete_stops_a_running_job_first() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("echo deleting; read answer < {}", fifo.display());
    let daemon = setup.start(JobModel::shell(vec![background(&line)])).await;
    let (sent, _) = auto_turn(&daemon, "start it").await;
    let data = setup.dirs.dirs().data().to_path_buf();
    assert_eq!(files_named(&data, "j1.out").len(), 1);
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();

    let delete = Method::ConversationDelete(efr_protocol::ConversationDelete {
        command_id: command_id(70),
        conversation_id: sent.conversation_id,
        force: false,
        stop: false,
    });
    let answer: efr_protocol::ConversationDeleteResult = client.call(delete).await.unwrap();

    let efr_protocol::ConversationDeleteResult::Deleted { jobs_stopped, not_removed, .. } = answer
    else {
        panic!("not deleted: {answer:?}")
    };
    assert_eq!((jobs_stopped, not_removed), (1, Vec::new()));
    // The job ended by the user's stop before the tombstone, and the follow ends with
    // the tombstone.
    let events =
        events_until(&mut follow, |event| matches!(event, Event::ConversationDeleted { .. }))
            .await
            .unwrap();
    let ended_at = events
        .iter()
        .position(|envelope| {
            matches!(
                envelope.event,
                Event::JobEnded { stopped_by: Some(JobStopBy::User), state: JobState::Stopped, .. }
            )
        })
        .expect("the job's end");
    assert!(ended_at < events.len() - 1);
    let all: JobsListResult =
        client.call(Method::JobsList(JobsList { conversation_id: None })).await.unwrap();
    assert_eq!(all.jobs, Vec::new(), "no job of it runs or is listed");
    assert_eq!(files_named(&data, "j1.out"), Vec::<PathBuf>::new(), "its output went");
    drop((client, follow));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_stop_escalates_when_launcher_is_stopped() {
    // NOTE: the launcher stops itself with SIGSTOP and its child ignores SIGTERM: the
    // stop sends SIGCONT and SIGTERM, and the sweep by session ends the child.
    let setup = Setup::new();
    setup.fake.set("stop-self").unwrap();
    let line = "trap '' TERM; echo pid-$$; while :; do sleep 1; done";
    let daemon = setup.start(JobModel::shell(vec![background(line)])).await;
    let (sent, _) = auto_turn(&daemon, "start it").await;
    let scratch = setup.dirs.dirs().data().join("scratch");
    let child: u32 = Wait::new("the job's pid")
        .until_some(|| {
            let file = files_named(&scratch, "j1.out").pop()?;
            std::fs::read_to_string(file).ok()?.trim().strip_prefix("pid-")?.parse().ok()
        })
        .await
        .unwrap();
    assert_eq!(daemon.peer_side(child), PeerSide::ModelSide, "a process of a job is model-side");
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let stop = client.call::<JobsStopResult>(Method::JobsStop(JobsStop {
        command_id: command_id(70),
        conversation_id: Some(sent.conversation_id),
        job_id: JobId::nth(1),
    }));
    tokio::pin!(stop);
    // NOTE: the launcher's trap does not end the child, so efrd waits its grace on its
    // clock, then kills the launcher; the test moves the clock until the stop answers.
    let stopped = loop {
        tokio::select! {
            stopped = &mut stop => break stopped.unwrap(),
            () = efr_stdx::time::SystemClock.sleep(Duration::from_millis(20)) => {
                daemon.clock().advance(Duration::from_secs(1));
            }
        }
    };
    assert_eq!(
        (stopped.job.state, stopped.job.stopped_by),
        (JobState::Stopped, Some(JobStopBy::User))
    );
    Wait::new("the swept child is gone")
        .until(|| !Path::new(&format!("/proc/{child}")).exists())
        .await
        .unwrap();
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_survivors_named_in_ended_event() {
    let setup = Setup::new();
    setup.fake.set("survivors").unwrap();
    let daemon = setup.start(JobModel::shell(vec![background("echo done")])).await;
    let (sent, _) = auto_turn(&daemon, "start it").await;
    let Event::JobEnded { survivors, launcher_lost, .. } =
        ended(&daemon, sent.conversation_id, "j1").await
    else {
        unreachable!()
    };
    assert_eq!(survivors, ["fake-survivor"]);
    assert!(!launcher_lost);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn lost_launcher_marked_in_ended_event() {
    let setup = Setup::new();
    setup.fake.set("no-result").unwrap();
    let daemon = setup.start(JobModel::shell(vec![background("echo done")])).await;
    let (sent, _) = auto_turn(&daemon, "start it").await;
    let Event::JobEnded { state, launcher_lost, .. } =
        ended(&daemon, sent.conversation_id, "j1").await
    else {
        unreachable!()
    };
    assert!(launcher_lost);
    assert_eq!(state, JobState::Failed);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn turn_end_names_running_job() {
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let line = format!("read answer < {}", fifo.display());
    let daemon = setup.start(JobModel::shell(vec![background(&line)])).await;
    let (sent, events) = auto_turn(&daemon, "start it").await;
    let undo = events.iter().find_map(|envelope| match &envelope.event {
        Event::TurnCompleted { undo, .. } => undo.clone(),
        _ => None,
    });
    let undo = undo.expect("the turn's roots are not complete while its job runs");
    assert!(
        undo.not_covered.iter().any(|line| line.ends_with("(job j1 runs)")),
        "{:?}",
        undo.not_covered
    );
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_home_copy_has_no_after() {
    let setup = Setup::new();
    let notes = setup.dirs.home().join("Documents/notes.md");
    std::fs::create_dir_all(notes.parent().unwrap()).unwrap();
    std::fs::write(&notes, "old notes\n").unwrap();
    let line = format!("echo new notes > {}", notes.display());
    let daemon = setup.start(JobModel::shell(vec![background(&line)])).await;
    let (sent, events) = turn_on(
        &daemon,
        TTY,
        "put new notes in ~/Documents/notes.md",
        Mode::Auto,
        ApprovalDecision::Allow,
    )
    .await;
    assert_eq!(started(&events).len(), 1, "{:?}", kinds(&events));
    ended(&daemon, sent.conversation_id, "j1").await;
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "new notes\n");
    assert_eq!(
        crate::home::home_copies(&setup.dirs).await,
        [(notes.clone(), "old notes\n".to_owned())]
    );
    let undo = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::TurnCompleted { undo, .. } => undo.clone(),
            _ => None,
        })
        .unwrap();
    assert!(
        undo.not_covered.iter().any(|line| line == "~/Documents/notes.md (a job wrote it)"),
        "{:?}",
        undo.not_covered
    );
    assert!(!kinds(&events).contains(&"home_file_changed"), "a job's write has no after copy");
    daemon.stop().await.unwrap();
}

/// A daemon with a real hidden zsh and the fake launcher, with no `job_env` seam: a job
/// gets the environment that the zsh integration writes.
async fn zsh_daemon(setup: &Setup, model: Arc<dyn Provider>) -> TestDaemon {
    TestDaemon::builder()
        .local_pty()
        .dirs(Arc::clone(&setup.dirs))
        .sandbox_launcher(setup.fake.path())
        .probe_override(ready())
        .custom_provider(model)
        .start()
        .await
        .unwrap()
}

/// The `env` files that the fake launcher kept, one per job, as text.
fn kept_envs(fake: &Path) -> Vec<String> {
    let mut envs: Vec<String> = std::fs::read_dir(fake)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("env-"))
        .map(|entry| String::from_utf8_lossy(&std::fs::read(entry.path()).unwrap()).into_owned())
        .collect();
    envs.sort();
    envs
}

#[tokio::test]
async fn shell_job_waits_for_first_prompt_env() {
    if !crate::support::zsh_enabled("shell_job_waits_for_first_prompt_env") {
        return;
    }
    let setup = Setup::new();
    let daemon = zsh_daemon(&setup, JobModel::shell(vec![background("echo from-the-job")])).await;
    let (sent, events) = auto_turn(&daemon, "start it").await;
    let [(output, is_error)] = outputs(&events).try_into().unwrap();
    assert!(!is_error, "{output}");
    let Event::JobEnded { tail, .. } = ended(&daemon, sent.conversation_id, "j1").await else {
        unreachable!()
    };
    assert_eq!(tail, "from-the-job");
    let [env] = kept_envs(setup.fake.dir()).try_into().unwrap();
    let home = format!("HOME={}\0", setup.dirs.home().display());
    assert!(env.contains(&home), "the job got the shell's own environment");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_job_env_after_empty_line() {
    if !crate::support::zsh_enabled("shell_job_env_after_empty_line") {
        return;
    }
    let setup = Setup::new();
    let shell_dir = setup.dirs.dirs().runtime().join("sbx");
    let gone = shell_dir.clone();
    // NOTE: before the second job, the shell's env file goes, as it would for a shell
    // that started before efr wrote one.
    let removed: Gate = Arc::new(move || {
        let Ok(entries) = std::fs::read_dir(&gone) else { return false };
        let mut found = false;
        for entry in entries.flatten() {
            found |= std::fs::remove_file(entry.path().join("env")).is_ok();
        }
        found
    });
    let model = JobModel::new(vec![
        ("shell", background("echo first"), None),
        ("shell", background("echo second"), Some(removed)),
    ]);
    let daemon = zsh_daemon(&setup, model).await;
    let (sent, events) = auto_turn(&daemon, "two jobs").await;
    let outputs = outputs(&events);
    assert!(outputs.iter().all(|(_, is_error)| !is_error), "{outputs:?}");
    assert_eq!(started(&events).len(), 2);
    ended(&daemon, sent.conversation_id, "j2").await;
    assert_eq!(kept_envs(setup.fake.dir()).len(), 2);
    daemon.stop().await.unwrap();
}
