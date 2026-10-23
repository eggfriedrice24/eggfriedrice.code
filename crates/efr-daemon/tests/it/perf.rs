//! The `bench_perf_*` benches of efrd end to end (`research/perf-plan.md`, section 2):
//! the time to the first token, `prompt.send`, the end of a turn, one shell call in
//! `auto`, the start of a subscription, and the memory of the daemon. `just bench-perf`
//! runs them in release; each is an ignored test that prints its lines through
//! `efr_test_support::bench::Report` and never fails on a time.
//!
//! Each bench runs the real daemon in this process (`TestDaemon`) on a tree under
//! cargo's target temp dir, with a file store as efrd has, never on the user's dirs.
//! The daemon runs on a `TestClock`, which a [`Follower`] moves with real time every
//! millisecond, so the draft interval of 16 ms, the update interval of 200 ms and the
//! timeouts are real, as in efrd on `SystemClock`. A `TestDaemon` gives the seams that
//! a bench needs (the fake model, the fake launcher, the isolated git) on a clock that
//! a bench can also move ahead, for the idle period of the memory bench. The model is
//! [`PerfModel`], a provider of this test, or the real providers against the fake
//! servers of `efr-test-daemon`.
//!
//! A bench that counts git processes runs again in a child process of this test binary
//! with a `PATH` whose first `git` is a counting wrapper ([`GitCount`]): efrd runs git
//! from the `PATH` of its process, which is this process.

mod memory;
mod prompt_send;
mod shell_call;
mod subscribe;
mod ttft;
mod turn_end;

use std::collections::BTreeMap;
use std::fmt;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, ConversationSubscribe,
    ConversationSubscribeItem, DraftPart, Event, EventEnvelope, Method, Mode, PromptSend,
    PromptSendResult, ShellContext, TurnSettings,
};
use efr_provider::{
    ContentBlock, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request,
    Role, StopReason,
};
use efr_stdx::time::{Clock as _, SystemClock};
use efr_test_daemon::{Client, TestDaemon, TestDaemonBuilder, command_id};
use efr_test_support::{TestClock, TestDirs};
use futures::StreamExt as _;
use serde_json::Value;

/// The reviewer's answer: it allows every exit.
const REVIEWER_ALLOWS: &str = crate::sandbox::REVIEWER_ALLOWS;

/// The model's summary for a compaction.
const SUMMARY: &str = "The user asked many times to read notes.txt in the project. The model \
                       read it each time and said that it lists the steps of the build and \
                       the tests. Nothing changed.";

/// The variable that names the log of the counting git in the child process.
const GIT_LOG: &str = "EFR_PERF_GIT_LOG";

/// A rung of a ladder that takes longer than this ends the ladder (the brief's rule).
pub(crate) const RUNG_LIMIT: Duration = Duration::from_secs(10);

/// A fresh tree for one bench under cargo's target temp dir, not on the tmpfs of
/// `/tmp`.
pub(crate) fn tree() -> Arc<TestDirs> {
    Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap())
}

/// The next command id of this process: unique across the daemons of one bench, which
/// share a store and its receipts.
pub(crate) fn next_command() -> u128 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    u128::from(NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The seed of the next daemon of this process, so a daemon on the tree of an earlier
/// one never draws its ids again.
fn next_seed() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(100);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A builder for a daemon on `dirs` and `clock` with a file store, as efrd has.
pub(crate) fn builder(dirs: &Arc<TestDirs>, clock: &TestClock) -> TestDaemonBuilder {
    TestDaemon::builder().dirs(Arc::clone(dirs)).clock(clock.clone()).seed(next_seed()).persistent()
}

/// Moves a `TestClock` with real time, every millisecond, while it lives.
#[derive(Debug)]
pub(crate) struct Follower(tokio::task::JoinHandle<()>);

impl Follower {
    pub(crate) fn start(clock: &TestClock) -> Follower {
        let clock = clock.clone();
        Follower(tokio::spawn(async move {
            let mut last = Instant::now();
            loop {
                SystemClock.sleep(Duration::from_millis(1)).await;
                let now = Instant::now();
                clock.advance(now - last);
                last = now;
            }
        }))
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What [`PerfModel`] does in each turn: it calls each tool of `tools` in turn, one per
/// model call, then answers `answer`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Plan {
    pub(crate) tools: Vec<(String, Value)>,
    pub(crate) answer: String,
}

impl Plan {
    /// A plan that answers `text` at once.
    pub(crate) fn answer(text: impl Into<String>) -> Plan {
        Plan { tools: Vec::new(), answer: text.into() }
    }

    /// A plan that calls `name` with `input`, then answers `text`.
    pub(crate) fn call(name: &str, input: Value, text: impl Into<String>) -> Plan {
        Plan { tools: vec![(name.to_owned(), input)], answer: text.into() }
    }
}

/// A hook that the model runs when it ends an answer, before it hands the end over.
type OnDone = Arc<dyn Fn() + Send + Sync>;

/// A model of the benches: it follows its [`Plan`], answers the reviewer with an allow,
/// and notes when each turn's call came and when it handed over the end of each answer.
pub(crate) struct PerfModel {
    id: ProviderId,
    plan: Mutex<Plan>,
    called: Mutex<Vec<Instant>>,
    ended: Arc<Mutex<Vec<Instant>>>,
    on_done: Mutex<Option<OnDone>>,
    issued: Mutex<Issued>,
}

/// The tool calls that the model made in the turn of the mark `current`, and the marks
/// of every turn so far.
#[derive(Debug, Default)]
struct Issued {
    current: String,
    count: usize,
    seen: std::collections::HashSet<String>,
}

impl fmt::Debug for PerfModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PerfModel").field("plan", &*lock(&self.plan)).finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PerfModel {
    pub(crate) fn new(plan: Plan) -> Arc<PerfModel> {
        Arc::new(PerfModel {
            id: ProviderId::new("test").unwrap(),
            plan: Mutex::new(plan),
            called: Mutex::default(),
            ended: Arc::default(),
            on_done: Mutex::default(),
            issued: Mutex::default(),
        })
    }

    /// Follows `plan` from the next model call on.
    pub(crate) fn set_plan(&self, plan: Plan) {
        *lock(&self.plan) = plan;
    }

    /// Runs `hook` each time the model ends an answer.
    pub(crate) fn on_done(&self, hook: impl Fn() + Send + Sync + 'static) {
        *lock(&self.on_done) = Some(Arc::new(hook));
    }

    /// When the model handed over the end of its latest answer.
    pub(crate) fn last_end(&self) -> Option<Instant> {
        lock(&self.ended).last().copied()
    }

    /// When the model handed over the end of each answer of a turn's call from `from`
    /// to `to`, in order.
    pub(crate) fn ends_between(&self, from: Instant, to: Instant) -> Vec<Instant> {
        lock(&self.ended).iter().copied().filter(|at| *at >= from && *at <= to).collect()
    }
}

/// The mark of the current turn: the newest `[perf-N]` of a user's text, which
/// [`prompt`] puts at the end of each prompt. A compaction can take the prompt out of
/// the history; the model then finds an older mark or none.
fn turn_mark(request: &Request) -> Option<String> {
    request
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == Role::User)
        .flat_map(|message| message.content.iter().rev())
        .find_map(|block| match block {
            ContentBlock::Text { text } => {
                let start = text.rfind(MARK)?;
                let end = text[start..].find(']')?;
                Some(text[start..start + end].to_owned())
            }
            _ => None,
        })
}

/// The start of the mark of a prompt.
const MARK: &str = "[perf-";

#[async_trait]
impl Provider for PerfModel {
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
        if request.side_call {
            // A compaction's summary: the history of a long conversation reaches the
            // trigger, as a real one does.
            let events = vec![
                ProviderEvent::TextDelta { text: SUMMARY.to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ];
            return Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))));
        }
        lock(&self.called).push(Instant::now());
        let plan = lock(&self.plan).clone();
        // NOTE: the calls of a turn count by its mark, not by the results in the
        // history, which a compaction in the turn can take out. A request whose mark is
        // gone gets the answer, so a turn always ends.
        let done = match turn_mark(&request) {
            Some(mark) => {
                let mut issued = lock(&self.issued);
                if issued.current != mark {
                    if issued.seen.contains(&mark) {
                        // An older turn's mark: this turn's prompt is gone.
                        usize::MAX
                    } else {
                        issued.seen.insert(mark.clone());
                        issued.current = mark;
                        issued.count = 1;
                        0
                    }
                } else {
                    issued.count += 1;
                    issued.count - 1
                }
            }
            None => usize::MAX,
        };
        let events = match plan.tools.get(done) {
            Some((name, input)) => {
                let call_id = format!("perf_{}", next_command());
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
                ProviderEvent::TextDelta { text: plan.answer },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ],
        };
        let hook = lock(&self.on_done).clone();
        let ended = Arc::clone(&self.ended);
        let stream = futures::stream::iter(events).map(move |event| {
            if matches!(event, ProviderEvent::Done { .. }) {
                if let Some(hook) = &hook {
                    hook();
                }
                lock(&ended).push(Instant::now());
            }
            Ok(event)
        });
        Ok(Box::pin(stream))
    }
}

/// A `prompt.send` of `text` from the shell of `tty` in `cwd`, in `mode` when it is
/// set, with a command id of its own and the mark of its turn for [`PerfModel`].
pub(crate) fn prompt(tty: &str, cwd: &Path, text: &str, mode: Option<Mode>) -> PromptSend {
    let mut context = ShellContext::new(cwd);
    context.tty = Some(tty.to_owned());
    let n = next_command();
    PromptSend {
        command_id: command_id(n),
        conversation_id: None,
        new_conversation: false,
        text: format!("{text} {MARK}{n}]"),
        context: Some(context),
        last_command: None,
        recent_commands: Vec::new(),
        settings: TurnSettings { mode, ..TurnSettings::default() },
    }
}

/// One turn as a client saw it, the way `efr send` follows it: the prompt, its ack,
/// then a subscription from the prompt's event with drafts.
#[derive(Debug)]
pub(crate) struct TurnRun {
    pub(crate) sent: PromptSendResult,
    /// When the client sent `prompt.send`.
    pub(crate) sent_at: Instant,
    /// When the client got its ack.
    pub(crate) acked_at: Instant,
    /// When the client first held text of the answer, from a draft or an event.
    pub(crate) first_text_at: Option<Instant>,
    /// Each event of the turn with the time the client got it.
    pub(crate) events: Vec<(Instant, EventEnvelope)>,
    /// The approvals that the client answered with yes.
    pub(crate) approvals: usize,
}

impl TurnRun {
    /// When the client got the turn's terminal event.
    pub(crate) fn ended_at(&self) -> Instant {
        self.events.last().map_or(self.acked_at, |(at, _)| *at)
    }

    /// The kind of the turn's terminal event.
    pub(crate) fn end_kind(&self) -> &str {
        self.events.last().map_or("none", |(_, envelope)| envelope.event.kind())
    }

    /// The events of kind `kind`.
    pub(crate) fn count(&self, kind: &str) -> usize {
        self.events.iter().filter(|(_, envelope)| envelope.event.kind() == kind).count()
    }

    /// When the client got each `tool_call_started`, in order.
    pub(crate) fn started_at(&self) -> Vec<Instant> {
        self.events
            .iter()
            .filter(|(_, envelope)| matches!(envelope.event, Event::ToolCallStarted { .. }))
            .map(|(at, _)| *at)
            .collect()
    }

    /// The time from each `tool_call_started` to the `tool_call_completed` of the same
    /// call, in the order of the calls.
    pub(crate) fn call_times(&self) -> Vec<Duration> {
        let mut started = BTreeMap::new();
        let mut times = Vec::new();
        for (at, envelope) in &self.events {
            match &envelope.event {
                Event::ToolCallStarted { call_id, .. } => {
                    started.insert(*call_id, *at);
                }
                Event::ToolCallCompleted { call_id, .. } => {
                    if let Some(start) = started.remove(call_id) {
                        times.push(at.saturating_duration_since(start));
                    }
                }
                _ => {}
            }
        }
        times
    }
}

/// Sends `params` over `client` and follows its turn to the end as `efr send` does,
/// answering every approval with yes.
pub(crate) async fn run_turn(client: &Client, params: PromptSend) -> TurnRun {
    let sent_at = Instant::now();
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let acked_at = Instant::now();
    follow_turn(client, sent, sent_at, acked_at).await
}

/// Follows the turn of `sent`, a prompt that the client sent at `sent_at` and that the
/// daemon acked at `acked_at`, as [`run_turn`] does.
pub(crate) async fn follow_turn(
    client: &Client,
    sent: PromptSendResult,
    sent_at: Instant,
    acked_at: Instant,
) -> TurnRun {
    let subscribe = ConversationSubscribe {
        conversation_id: sent.conversation_id,
        after_seq: Some(sent.seq),
        answers_input: false,
        drafts: true,
    };
    let mut stream = client.stream(Method::ConversationSubscribe(subscribe)).await.unwrap();
    let mut run =
        TurnRun { sent, sent_at, acked_at, first_text_at: None, events: Vec::new(), approvals: 0 };
    let turn = run.sent.turn_id;
    while let Some(item) = stream.next().await {
        let now = Instant::now();
        let envelopes = match item.unwrap() {
            ConversationSubscribeItem::Event(envelope) => vec![envelope],
            ConversationSubscribeItem::Snapshot(snapshot) => snapshot.events,
            ConversationSubscribeItem::Draft(draft) => {
                if matches!(draft.draft, DraftPart::Text { .. }) && draft.turn_id == turn {
                    run.first_text_at.get_or_insert(now);
                }
                Vec::new()
            }
            _ => Vec::new(),
        };
        for envelope in envelopes {
            let mut end = false;
            match &envelope.event {
                Event::AssistantMessageUpdated { turn_id, .. }
                | Event::AssistantMessageCompleted { turn_id, .. }
                    if *turn_id == turn =>
                {
                    run.first_text_at.get_or_insert(now);
                }
                Event::ApprovalRequested { turn_id, call_id, .. } if *turn_id == turn => {
                    run.approvals += 1;
                    let answer = Method::ApprovalRespond(ApprovalRespond {
                        command_id: command_id(next_command()),
                        conversation_id: run.sent.conversation_id,
                        call_id: *call_id,
                        decision: ApprovalDecision::Allow,
                    });
                    let _: ApprovalRespondResult = client.call(answer).await.unwrap();
                }
                Event::TurnCompleted { turn_id, .. }
                | Event::TurnFailed { turn_id, .. }
                | Event::TurnInterrupted { turn_id, .. }
                    if *turn_id == turn =>
                {
                    end = true;
                }
                _ => {}
            }
            run.events.push((now, envelope));
            if end {
                return run;
            }
        }
    }
    panic!("the subscription ended before the end of turn {turn}")
}

/// The counting `git` of a bench that runs in a child process: a wrapper first on the
/// `PATH` that logs the git command of each run, one line each, then runs the real git.
/// A bench counts the runs between two points of its own, so the git runs of its
/// fixtures do not count.
#[derive(Debug)]
pub(crate) struct GitCount {
    log: PathBuf,
}

impl GitCount {
    /// In the child process: the counter. In the test's own process: runs the test
    /// `name` of the module `module` (`module_path!()`) again in a child process with
    /// the counting git first on its `PATH`, checks that it passed, and gives `None`.
    pub(crate) async fn child_or_run(module: &str, name: &str) -> Option<GitCount> {
        if let Some(log) = std::env::var_os(GIT_LOG) {
            return Some(GitCount { log: PathBuf::from(log) });
        }
        let dir = tree();
        let bin = dir.create_dir("git-bin").unwrap();
        let log = dir.root().join("git.log");
        let path = std::env::var_os("PATH").unwrap_or_default();
        let real = std::env::split_paths(&path)
            .map(|dir| dir.join("git"))
            .find(|git| git.is_file())
            .expect("a git on the PATH");
        let wrapper = WRAPPER
            .replace("@LOG@", &log.to_string_lossy())
            .replace("@GIT@", &real.to_string_lossy());
        let git = bin.join("git");
        std::fs::write(&git, wrapper).unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&path));
        let test = match module.split_once("::") {
            Some((_, inner)) => format!("{inner}::{name}"),
            None => name.to_owned(),
        };
        let exe = std::env::current_exe().unwrap();
        let status = efr_stdx::process::command(&exe, dir.root())
            .args(["--exact", &test, "--ignored", "--nocapture"])
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env(GIT_LOG, &log)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "the bench in the child process failed: {status}");
        None
    }

    /// The git runs so far.
    pub(crate) fn runs(&self) -> u64 {
        std::fs::read_to_string(&self.log).map_or(0, |text| text.lines().count() as u64)
    }

    /// The git commands of the runs after the first `from`, with their counts.
    pub(crate) fn commands_after(&self, from: u64) -> BTreeMap<String, u64> {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let mut counts = BTreeMap::new();
        for line in text.lines().skip(usize::try_from(from).unwrap_or(usize::MAX)) {
            *counts.entry(line.to_owned()).or_insert(0) += 1;
        }
        counts
    }
}

/// The counting git: it logs the first word of its arguments that is no option (the
/// git command), then runs the real git with the same arguments.
const WRAPPER: &str = r#"#!/bin/sh
sub=''
skip=0
for a in "$@"; do
    if [ "$skip" = 1 ]; then skip=0; continue; fi
    case "$a" in
    -c|-C) skip=1 ;;
    -*) ;;
    *) sub=$a; break ;;
    esac
done
echo "${sub:-none}" >> '@LOG@'
exec '@GIT@' "$@"
"#;

/// `git <args>` in `dir` without the user's git config, for the fixture of a bench; it
/// panics when git fails.
pub(crate) async fn fixture_git(dir: &Path, args: &[&str]) {
    let output = efr_stdx::process::command("git", dir)
        .args(["-c", "user.name=efr", "-c", "user.email=efr@test", "-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

/// A fresh git repository at `dir` with `files` files of one line in directories of
/// 100, committed.
pub(crate) async fn git_fixture(dir: &Path, files: usize) {
    for n in 0..files {
        let sub = dir.join(format!("d{:03}", n / 100));
        if n % 100 == 0 {
            std::fs::create_dir_all(&sub).unwrap();
        }
        std::fs::write(sub.join(format!("f{n:05}.txt")), format!("line {n}\n")).unwrap();
    }
    fixture_git(dir, &["init", "-q", "-b", "main"]).await;
    fixture_git(dir, &["add", "-A"]).await;
    fixture_git(dir, &["commit", "-q", "--allow-empty", "-m", "fixture"]).await;
}

/// Registers `path` as a project of `daemon`.
pub(crate) async fn register(daemon: &TestDaemon, path: &Path) {
    let client = daemon.client().await.unwrap();
    let add = Method::AdminProjectAdd(efr_protocol::AdminProjectAdd {
        path: path.to_path_buf(),
        name: None,
        git_root: false,
    });
    let _: efr_protocol::AdminProjectAddResult = client.call(add).await.unwrap();
}

/// The median of `times`; zero for none.
pub(crate) fn median_time(times: &[Duration]) -> Duration {
    efr_test_support::bench::Stats::of(times).p50
}

/// The median of `values`; 0 for none.
pub(crate) fn median(values: &[u64]) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or(0)
}
