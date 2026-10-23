//! The bench of one shell call in `auto` on the real sandbox: calls of `true` in a git
//! project of 100, 2,000 and 20,000 files, with 0, 1 and 4 contained jobs that run in
//! the same project. Each `phase` line of efrd (docs/sandbox.md) is a measure of its
//! own, with the time per call of each turn. A bench, ignored: `just bench-perf` runs
//! it in release. It needs a real zsh (`EFR_TEST_ZSH=1`) and the real launcher
//! (`EFR_TEST_SBX_BIN`), and reports why it skips without them.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ApprovalDecision,
    ApprovalRespond, ApprovalRespondResult, ConversationSubscribe, Event, EventEnvelope, Method,
    Mode, PromptSendResult, ShellContext, TurnSettings,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_daemon::{TTY, TestDaemon, command_id, events_until};
use efr_test_support::TestDirs;
use efr_test_support::bench::Report;
use serde_json::{Value, json};

use super::{
    REVIEWER_ALLOWS, debug_log, git, phase_lines, real_launcher, take_log, turn_cost, with_zsh,
};
use crate::support::zsh_enabled;

/// The calls of `true` in each measured turn.
const CALLS: u32 = 10;

/// The project sizes of the ladder, in files.
const PROJECTS: [usize; 3] = [100, 2_000, 20_000];

/// The running jobs of each step.
const JOBS: [usize; 3] = [0, 1, 4];

/// A model that answers each request with the next queued shell call, and with `done`
/// when the queue is empty. The reviewer's requests get an allow.
#[derive(Debug)]
struct Queue {
    id: ProviderId,
    calls: Mutex<VecDeque<Value>>,
    next: Mutex<u64>,
}

impl Queue {
    fn new() -> Arc<Self> {
        Arc::new(Queue {
            id: ProviderId::new("test").unwrap(),
            calls: Mutex::new(VecDeque::new()),
            next: Mutex::new(0),
        })
    }

    /// Queues `count` calls of the shell tool with `input`.
    fn push(&self, input: &Value, count: usize) {
        let mut calls = self.calls.lock().unwrap();
        calls.extend(std::iter::repeat_n(input.clone(), count));
    }
}

fn stream(events: Vec<ProviderEvent>) -> ProviderStream {
    Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
}

#[async_trait]
impl Provider for Queue {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        if request.one_shot {
            return Ok(stream(vec![
                ProviderEvent::TextDelta { text: REVIEWER_ALLOWS.to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ]));
        }
        let Some(input) = self.calls.lock().unwrap().pop_front() else {
            return Ok(stream(vec![
                ProviderEvent::TextDelta { text: "done".to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ]));
        };
        let call_id = {
            let mut next = self.next.lock().unwrap();
            *next += 1;
            format!("call_{next}")
        };
        Ok(stream(vec![
            ProviderEvent::ToolCallStart {
                call_id: call_id.clone(),
                name: "shell".to_owned(),
                freeform: false,
            },
            ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() },
            ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None },
        ]))
    }
}

/// A git project of `files` files in dirs of 100, with one commit.
async fn project(dirs: &TestDirs, files: usize) -> std::path::PathBuf {
    let project = dirs.create_dir(format!("home/p{files}")).unwrap();
    git(&project, &["init", "-q", "-b", "main"]).await;
    for n in 0..files {
        let dir = project.join(format!("src/d{}", n / 100));
        if n.is_multiple_of(100) {
            std::fs::create_dir_all(&dir).unwrap();
        }
        std::fs::write(dir.join(format!("f{n}.rs")), format!("pub fn f{n}() {{}}\n")).unwrap();
    }
    git(&project, &["add", "."]).await;
    git(&project, &["commit", "-qm", "files"]).await;
    project
}

/// The prompt numbers, one per prompt of the bench.
struct Prompts(u128);

impl Prompts {
    fn next(&mut self) -> u128 {
        self.0 += 1;
        self.0
    }
}

/// One turn in `auto` from `cwd` (prompt `n`), with each approval answered with no:
/// the events of the turn from its prompt on.
async fn turn_events(daemon: &TestDaemon, n: u128, cwd: &Path) -> Vec<EventEnvelope> {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(n, "go", TTY) else { unreachable!() };
    let mut context = ShellContext::new(cwd);
    context.tty = Some(TTY.to_owned());
    params.context = Some(context);
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    // NOTE: from this turn's prompt on, so the end of an earlier turn does not count.
    let params = ConversationSubscribe {
        conversation_id: sent.conversation_id,
        after_seq: Some(sent.seq),
        answers_input: false,
        drafts: false,
    };
    let mut follow = client.stream(Method::ConversationSubscribe(params)).await.unwrap();
    let mut seen = Vec::new();
    let mut answers = n * 100;
    loop {
        let events = events_until(&mut follow, |event| {
            matches!(
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
            decision: ApprovalDecision::Deny,
        });
        let _: ApprovalRespondResult = client.call(answer).await.unwrap();
    }
    seen
}

/// Starts `count` contained jobs of `sleep` in `project` in one turn, and checks that
/// each started.
async fn start_jobs(
    daemon: &TestDaemon,
    model: &Queue,
    prompts: &mut Prompts,
    project: &Path,
    count: usize,
) {
    let job = json!({ "command": "sleep 300", "background": true });
    model.push(&job, count);
    let events = turn_events(daemon, prompts.next(), project).await;
    let started =
        events.iter().filter(|envelope| matches!(envelope.event, Event::JobStarted { .. })).count();
    assert_eq!(started, count, "{:?}", super::kinds(&events));
}

/// The measures of one size of project.
async fn project_steps(report: &mut Report, launcher: &Path, files: usize) -> Option<()> {
    let dirs = Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap());
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let project = project(&dirs, files).await;
    let home = dirs.home().to_string_lossy().into_owned();
    let env = BTreeMap::from([
        ("HOME".to_owned(), home.clone()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.to_string_lossy().into_owned()),
    ]);
    let job_env = BTreeMap::from([
        ("HOME".to_owned(), home),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
    ]);
    let model = Queue::new();
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .shell_env(env)
        .job_env(job_env)
        .sandbox_launcher(launcher)
        .custom_provider(Arc::clone(&model) as Arc<dyn Provider>)
        .config(|settings| settings.jobs.max_running = 8)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    if !status.sandbox.is_some_and(|sandbox| sandbox.available) {
        report.count("skipped: the probe says the sandbox is unavailable", 0);
        daemon.stop().await.unwrap();
        return None;
    }
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: project.clone(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    let call = json!({ "command": "true" });
    let mut prompts = Prompts(0);
    // The first turn starts the hidden shell and writes the snapshot; it does not count.
    debug_log();
    model.push(&call, CALLS as usize);
    turn_cost(&daemon, prompts.next(), &project, Mode::Auto, CALLS).await;
    take_log();
    let mut running = 0;
    for jobs in JOBS {
        if jobs > running {
            start_jobs(&daemon, &model, &mut prompts, &project, jobs - running).await;
            running = jobs;
        }
        take_log();
        model.push(&call, CALLS as usize);
        let per_call = turn_cost(&daemon, prompts.next(), &project, Mode::Auto, CALLS).await;
        let lines = phase_lines(&take_log());
        let label = format!("{files} files {jobs} jobs");
        report.time(&format!("{label} turn per call"), &[per_call]);
        let mut order: Vec<&str> = Vec::new();
        for (phase, _) in &lines {
            if !order.contains(&phase.as_str()) {
                order.push(phase);
            }
        }
        for phase in order {
            let times: Vec<Duration> = lines
                .iter()
                .filter(|(known, _)| known == phase)
                .map(|(_, ms)| Duration::from_secs_f64(ms / 1000.0))
                .collect();
            report.time(&format!("{label} phase={phase}"), &times);
            // NOTE: a phase that runs other than once per call (a snapshot per root, the
            // turn's end) also gets its total over the turn, per call.
            if times.len() != CALLS as usize {
                let total: Duration = times.iter().sum();
                report.time(&format!("{label} phase={phase} total per call"), &[total / CALLS]);
            }
        }
    }
    drop(client);
    daemon.stop().await.unwrap();
    Some(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_shell_call_sandbox() {
    let test = "bench_perf_shell_call_sandbox";
    let mut report = Report::new(test);
    if !zsh_enabled(test) {
        report.count("skipped: set EFR_TEST_ZSH=1", 0);
        report.finish();
        return;
    }
    let Some(launcher) = real_launcher(test) else {
        report.count("skipped: set EFR_TEST_SBX_BIN to a built efr-sbx", 0);
        report.finish();
        return;
    };
    for files in PROJECTS {
        let start = std::time::Instant::now();
        if project_steps(&mut report, &launcher, files).await.is_none() {
            break;
        }
        report.time(&format!("{files} files whole step"), &[start.elapsed()]);
    }
    report.finish();
}
