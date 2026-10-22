//! The git code of an outside git line end to end (v0.0.4, B4), on the real `efr-sbx`
//! of `just test-sandbox` (`EFR_TEST_SBX_BIN`) and a real hidden zsh
//! (`EFR_TEST_ZSH=1`): a hook that a contained call planted makes the push one that only
//! the user may approve, and a running job's plant goes to the quarantine before an
//! outside git line of any project.
//!
//! The tests say `skipped:` without the launcher or zsh, or when the probe says that the
//! sandbox is unavailable, and keep their dirs below the target dir, not below /tmp,
//! which the sandbox replaces with its own. No push of these tests reaches a host: the
//! user says no, or the remote is a local path.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ApprovalDecision,
    ApprovalRespond, ApprovalRespondResult, Event, EventEnvelope, ExitInfo, JobStopBy, Method,
    Mode, PromptSendResult, SandboxSurfaceRespond, SandboxSurfaceRespondResult, TurnSettings,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_daemon::{TTY, TestDaemon, command_id, events_until};
use efr_test_support::{TestDirs, Wait};
use serde_json::{Value, json};

use super::{JOB_WORK, REVIEWER_ASKS, git, kinds, real_launcher, skip, with_zsh};
use crate::support::zsh_enabled;

/// A model whose calls the test adds before each turn: a `None` ends a turn. The
/// reviewer hands each exit to the user.
struct Steps {
    id: ProviderId,
    calls: Mutex<VecDeque<Option<(String, Value)>>>,
    reviews: AtomicUsize,
}

impl std::fmt::Debug for Steps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Steps").finish_non_exhaustive()
    }
}

impl Steps {
    fn new() -> Arc<Steps> {
        Arc::new(Steps {
            id: ProviderId::new("test").unwrap(),
            calls: Mutex::new(VecDeque::new()),
            reviews: AtomicUsize::new(0),
        })
    }

    /// The next turn calls `shell` with `command`, then ends.
    fn shell(&self, command: &str) {
        let mut calls = self.calls.lock().unwrap();
        calls.push_back(Some(("shell".to_owned(), json!({ "command": command }))));
        calls.push_back(None);
    }

    /// How many exits the reviewer judged.
    fn reviews(&self) -> usize {
        self.reviews.load(Ordering::SeqCst)
    }
}

fn stream(events: Vec<ProviderEvent>) -> ProviderStream {
    Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
}

#[async_trait]
impl Provider for Steps {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        if request.one_shot {
            self.reviews.fetch_add(1, Ordering::SeqCst);
            return Ok(stream(vec![
                ProviderEvent::TextDelta { text: REVIEWER_ASKS.to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ]));
        }
        let next = self.calls.lock().unwrap().pop_front().flatten();
        let Some((name, input)) = next else {
            return Ok(stream(vec![
                ProviderEvent::TextDelta { text: "done".to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ]));
        };
        let call_id = format!("call_{}", input.to_string().len());
        Ok(stream(vec![
            ProviderEvent::ToolCallStart { call_id: call_id.clone(), name, freeform: false },
            ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() },
            ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None },
        ]))
    }
}

/// Dirs below the target dir, not /tmp, which the real sandbox replaces with its own.
fn target_dirs() -> Arc<TestDirs> {
    Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap())
}

/// A daemon over `model` with the real launcher and a real hidden zsh, whose working
/// directory `<home>/project` and the dirs of `others` are registered projects. `None`
/// after a `skipped:` line.
async fn real_daemon(
    test: &str,
    dirs: &Arc<TestDirs>,
    model: Arc<Steps>,
    others: &[&Path],
) -> Option<TestDaemon> {
    if !zsh_enabled(test) {
        return None;
    }
    let launcher = real_launcher(test)?;
    std::fs::create_dir_all(dirs.home().join("project")).unwrap();
    let user_runtime = dirs.root().join("xrt");
    std::fs::create_dir_all(&user_runtime).unwrap();
    let env = BTreeMap::from([
        ("HOME".to_owned(), dirs.home().display().to_string()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
    ]);
    // NOTE: the clock starts after the files of the test were written, so no script of
    // the test counts as a program that changed in the turn (which asks the user).
    let start = jiff::Timestamp::now().checked_add(jiff::SignedDuration::from_mins(1)).unwrap();
    let daemon = with_zsh(TestDaemon::builder())
        .clock(efr_test_support::TestClock::starting_at(start))
        .dirs(Arc::clone(dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .config(|config| {
            config.permissions.mode = Mode::Auto;
            // NOTE: no wake turn takes the calls of the test's next turn.
            config.jobs.max_wakes_per_hour = 0;
        })
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    let sandbox = status.sandbox.unwrap();
    if !sandbox.available {
        // NOTE: just test-sandbox fails when this says skipped on a ready machine.
        skip(test, &format!("the probe says the sandbox is unavailable: {:?}", sandbox.reason));
        drop(client);
        daemon.stop().await.unwrap();
        return None;
    }
    let mut roots: Vec<PathBuf> = vec![daemon.cwd().to_path_buf()];
    roots.extend(others.iter().map(|other| other.to_path_buf()));
    for path in roots {
        let add = Method::AdminProjectAdd(AdminProjectAdd { path, name: None, git_root: false });
        let _: AdminProjectAddResult = client.call(add).await.unwrap();
    }
    Some(daemon)
}

/// The turn `n` of `text` in `auto` from the test's terminal: each exit question gets
/// `decision`, each question about a quarantined change gets `keep`.
async fn turn(
    daemon: &TestDaemon,
    n: u128,
    text: &str,
    decision: ApprovalDecision,
    keep: bool,
) -> (PromptSendResult, Vec<EventEnvelope>) {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(n, text, TTY) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
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
                        | Event::SurfaceQuestionRequested { .. }
                        | Event::TurnCompleted { .. }
                        | Event::TurnFailed { .. }
                        | Event::TurnInterrupted { .. }
                )
        })
        .await
        .unwrap();
        let last = events.last().unwrap().event.clone();
        seen.extend(events);
        answers += 1;
        match last {
            Event::ApprovalRequested { call_id, .. } => {
                let answer = Method::ApprovalRespond(ApprovalRespond {
                    command_id: command_id(answers),
                    conversation_id: sent.conversation_id,
                    call_id,
                    decision,
                });
                let _: ApprovalRespondResult = client.call(answer).await.unwrap();
            }
            Event::SurfaceQuestionRequested { question_id, .. } => {
                let answer = Method::SandboxSurfaceRespond(SandboxSurfaceRespond {
                    command_id: command_id(answers),
                    conversation_id: sent.conversation_id,
                    question_id,
                    keep,
                });
                let _: SandboxSurfaceRespondResult = client.call(answer).await.unwrap();
            }
            _ => break,
        }
    }
    seen.retain(|envelope| envelope.seq >= sent.seq);
    (sent, seen)
}

/// The exit question of `events`.
fn question(events: &[EventEnvelope]) -> ExitInfo {
    let exit = events.iter().find_map(|envelope| match &envelope.event {
        Event::ApprovalRequested { exit, .. } => exit.clone(),
        _ => None,
    });
    exit.unwrap_or_else(|| panic!("no exit question: {:?}", kinds(events)))
}

/// A hook script that makes `marker` when git runs it.
fn marking_hook(marker: &Path) -> String {
    format!("#!/bin/sh\\ntouch {}\\n", marker.display())
}

#[tokio::test]
async fn escape_planted_pre_push_in_scratch_clone_asks_user() {
    let test = "escape_planted_pre_push_in_scratch_clone_asks_user";
    let dirs = target_dirs();
    let marker = dirs.root().join("hook-ran");
    let project = dirs.create_dir("home/project").unwrap();
    let model = Steps::new();
    let Some(daemon) = real_daemon(test, &dirs, Arc::clone(&model), &[]).await else { return };
    // The first turn makes the conversation and its `$SCRATCH`.
    model.shell("true");
    let _ = turn(&daemon, 1, "start", ApprovalDecision::Deny, false).await;
    let scratch: Vec<PathBuf> = std::fs::read_dir(dirs.dirs().data().join("scratch"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    let [scratch] = scratch.try_into().unwrap();
    let repo = scratch.join("r");
    let hook = repo.join(".git/hooks/pre-push");

    // NOTE: a script plants the hook, as a hostile build script would; the remote is a
    // known host, so the local-path rule does not decide the push.
    let plant = format!(
        "set -e\ngit init -q {repo}\ngit -C {repo} remote add origin https://git.test/r.git\n\
         printf '{script}' > {hook}\nchmod +x {hook}\n",
        repo = repo.display(),
        hook = hook.display(),
        script = marking_hook(&marker),
    );
    std::fs::write(project.join("plant.sh"), plant).unwrap();

    // The contained call plants the hook in its `$SCRATCH`; the guard asks about it, and
    // the user keeps it.
    model.shell("sh plant.sh");
    let (_, events) = turn(&daemon, 2, "make a scratch clone", ApprovalDecision::Deny, true).await;
    assert!(hook.exists(), "the user kept the planted hook: {:?}", kinds(&events));

    // The push names the hook, and only the user may approve it.
    let line = format!("git -C {} push origin main", repo.display());
    model.shell(&line);
    let (_, events) = turn(&daemon, 3, "push it", ApprovalDecision::Deny, false).await;
    let exit = question(&events);
    assert!(exit.user_only, "{exit:?}");
    let named = exit.facts.iter().any(|fact| {
        fact.contains("r/.git/hooks/pre-push")
            && fact.ends_with("which a sandboxed command can write")
    });
    assert!(named, "{:?}", exit.facts);
    assert_eq!(model.reviews(), 0, "the reviewer never judged it");
    assert!(!marker.exists(), "the hook never ran");

    // The control row: with no plant, the same push goes to the reviewer.
    std::fs::remove_file(&hook).unwrap();
    model.shell(&line);
    let (_, events) = turn(&daemon, 4, "push it", ApprovalDecision::Deny, false).await;
    let exit = question(&events);
    assert_eq!(model.reviews(), 1, "{:?}", kinds(&events));
    assert!(!exit.user_only, "{exit:?}");
    assert!(exit.facts.iter().all(|fact| !fact.contains("git")), "{:?}", exit.facts);
    assert!(!marker.exists(), "the hook never ran");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn outside_git_checks_every_running_job() {
    let test = "outside_git_checks_every_running_job";
    let dirs = target_dirs();
    let project = dirs.create_dir("home/project").unwrap();
    let other = dirs.create_dir("home/other").unwrap();
    git(&other, &["init", "-q", "-b", "main"]).await;
    git(&other, &["commit", "-q", "--allow-empty", "-m", "one"]).await;
    let remote = dirs.root().join("remote.git");
    git(dirs.root(), &["init", "-q", "--bare", &remote.display().to_string()]).await;
    // NOTE: efr refuses a job whose line writes a hook itself, so the job runs a script
    // that does it. It waits for `go`, then makes its own `written` marker.
    let plant = "git init -q sub\nprintf '#!/bin/sh\\n' > sub/.git/hooks/pre-commit\n";
    std::fs::write(project.join("plant.sh"), plant).unwrap();
    let model = Steps::new();
    let Some(daemon) = real_daemon(test, &dirs, Arc::clone(&model), &[&other]).await else {
        return;
    };
    let line = "until [ -e go ]; do sleep 0.1; done; sh plant.sh; : > written; sleep 30";
    model
        .calls
        .lock()
        .unwrap()
        .extend([Some(("shell".to_owned(), json!({ "command": line, "background": true }))), None]);
    let _ = turn(&daemon, 1, "plant it as a job", ApprovalDecision::Deny, false).await;
    std::fs::write(project.join("go"), "").unwrap();
    Wait::new("the job's own marker")
        .limit(JOB_WORK)
        .until(|| project.join("written").exists())
        .await
        .unwrap();
    let hook = project.join("sub/.git/hooks/pre-commit");
    assert!(hook.exists(), "the job planted its hook");

    // A push in another project, whose dirs the job's guard roots do not hold.
    model.shell(&format!("git -C {} push {} main", other.display(), remote.display()));
    let (_, events) =
        turn(&daemon, 2, "push the other project", ApprovalDecision::Deny, false).await;
    let ended = events.iter().find_map(|envelope| match &envelope.event {
        Event::JobEnded { job_id, stopped_by, quarantined, .. } if job_id.as_str() == "j1" => {
            Some((envelope.seq, *stopped_by, *quarantined))
        }
        _ => None,
    });
    let asked = events.iter().find_map(|envelope| {
        matches!(envelope.event, Event::ApprovalRequested { .. }).then_some(envelope.seq)
    });
    let Some((ended, stopped_by, quarantined)) = ended else {
        panic!("the job ran on through the push: {:?}", kinds(&events))
    };
    assert_eq!(stopped_by, Some(JobStopBy::Guard));
    assert!(quarantined);
    assert!(asked.is_some_and(|asked| ended < asked), "the guard ran before the question");
    assert!(!hook.exists(), "the hook went to the quarantine");
    daemon.stop().await.unwrap();
}
