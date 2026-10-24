//! A running call that the user moves into a background job (Ctrl+B in `efr`,
//! `call.background`; v0.0.4 shell items DS15-DS25 and DS55) end to end: efrd's engine,
//! proxy and move, the real `efr-sbx` of `just test-sandbox` (`EFR_TEST_SBX_BIN`) and a
//! real hidden zsh (`EFR_TEST_ZSH=1`). The tests of `jobs_moved` run the same rules with
//! the fake launcher; here the moved call is a real sandbox or a real exit child.
//!
//! Each command prints `efr-ready` before the move. A command that must go on after the
//! move reads a FIFO below the home dir, which a contained command can read and which
//! the test writes after the move. No test waits on a fixed time: the daemon's clock
//! moves only while a sleep of the daemon waits for it.
//!
//! The tests say `skipped:` without the launcher, zsh or curl, or when the probe says
//! that the sandbox is unavailable; `just test-sandbox` fails on such a line on a ready
//! machine. Their dirs stay below the target dir, not below /tmp, which the sandbox
//! replaces with its own.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ApprovalDecision,
    ApprovalRespond, ApprovalRespondResult, CallBackground, CallBackgroundResult, CallId,
    ConversationId, ConversationSubscribe, ConversationSubscribeItem, Event, EventEnvelope,
    JobInfo, JobState, JobStopBy, JobsStop, JobsStopResult, Launch, Method, Mode, PromptSendResult,
    Seq, TurnSettings,
};
use efr_provider::Provider;
use efr_stdx::time::Clock as _;
use efr_test_daemon::{Client, ItemStream, TTY, TestDaemon, command_id, events_until};
use efr_test_support::{TestDirs, Wait};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::netview::target_dirs;
use super::proxy::registry::{Answer, Logged, Server, TestCa};
use super::proxy::{Options, REGISTRY, can_run, proxy_daemon};
use super::{REVIEWER_ASKS, ScriptedModel, real_launcher, skip, with_zsh};
use crate::support::zsh_enabled;

/// The marker that each command prints before the move.
const READY: &str = "efr-ready";

/// How long a test lets pass in real time while it moves the daemon's clock: the
/// launcher starts and stops in real time, which a loaded machine makes slow, and
/// nextest ends a test after 60 s.
const REAL_TIME_LIMIT: Duration = Duration::from_secs(30);

/// The real time with no event before [`until_job_ended`] moves the daemon's clock one
/// second. It only paces the clock: no outcome depends on it.
const PACE: Duration = Duration::from_millis(200);

/// A FIFO below the home dir of `dirs`, which a contained command can read.
fn gate(dirs: &TestDirs) -> PathBuf {
    let fifo = dirs.create_dir("home/gate").unwrap().join("go");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    fifo
}

/// Lets the command go on past the FIFO; it waits for the reader.
async fn release(fifo: &Path) {
    let fifo = fifo.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::write(&fifo, b"go\n")).await.unwrap().unwrap();
}

/// Writes an executable script.
fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Sends the prompt `text` from the test's terminal in `auto`, follows it on the same
/// client, which can answer, and allows each question, until the output of a call
/// shows [`READY`]. Returns the conversation, that call and the stream.
async fn start(
    daemon: &TestDaemon,
    client: &Client,
    text: &str,
) -> (ConversationId, CallId, ItemStream<ConversationSubscribeItem>) {
    let Method::PromptSend(mut params) = daemon.prompt(1, text, TTY) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let conversation_id = sent.conversation_id;
    let subscribe = ConversationSubscribe {
        conversation_id,
        after_seq: Some(Seq::ZERO),
        answers_input: true,
        drafts: false,
    };
    let mut stream = client.stream(Method::ConversationSubscribe(subscribe)).await.unwrap();
    let mut answers = 100;
    let mut call = None;
    loop {
        let seen = events_until(&mut stream, |event| {
            matches!(event, Event::ToolCallOutputUpdated { tail, .. } if tail.contains(READY))
                || matches!(
                    event,
                    Event::ApprovalRequested { .. }
                        | Event::ToolCallCompleted { .. }
                        | Event::TurnCompleted { .. }
                        | Event::TurnFailed { .. }
                )
        })
        .await
        .unwrap();
        for envelope in &seen {
            if let Event::ToolCallStarted { call_id, .. } = &envelope.event {
                call = Some(*call_id);
            }
        }
        match seen.last().map(|envelope| envelope.event.clone()) {
            Some(Event::ApprovalRequested { call_id, .. }) => {
                answers += 1;
                let allow = Method::ApprovalRespond(ApprovalRespond {
                    command_id: command_id(answers),
                    conversation_id,
                    call_id,
                    decision: ApprovalDecision::Allow,
                });
                let _: ApprovalRespondResult = client.call(allow).await.unwrap();
            }
            Some(Event::ToolCallOutputUpdated { .. }) => {
                return (conversation_id, call.unwrap(), stream);
            }
            // NOTE: an earlier call of the turn ended; the one that prints comes next.
            Some(Event::ToolCallCompleted { .. }) => {}
            other => panic!("the call did not start: {other:?} {seen:#?}"),
        }
    }
}

/// `call.background` of `call` with the command id `n`.
fn background(
    n: u128,
    conversation_id: ConversationId,
    call_id: CallId,
    confirm: Option<u64>,
) -> Method {
    Method::CallBackground(CallBackground {
        conversation_id,
        call_id,
        // NOTE: apart from the ids of the prompt and the answers.
        command_id: command_id(20_000 + n),
        confirm,
    })
}

/// The events of `stream` up to the first `turn_completed` and the job that the move
/// started.
async fn moved_job(stream: &mut ItemStream<ConversationSubscribeItem>) -> (JobInfo, u32, String) {
    let seen = events_until(stream, |event| {
        matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
    })
    .await
    .unwrap();
    let started = seen.iter().find_map(|envelope| match &envelope.event {
        Event::JobStarted { job, launcher_pid, .. } => Some((job.clone(), *launcher_pid)),
        _ => None,
    });
    let (job, shell_pid) = started.unwrap_or_else(|| panic!("no job started: {seen:#?}"));
    let output = seen.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { call_id, output, .. } if *call_id == job.call_id => {
            Some(output.clone())
        }
        _ => None,
    });
    (job, shell_pid, output.unwrap_or_else(|| panic!("the moved call answered: {seen:#?}")))
}

/// The events of one subscription item.
fn envelopes(item: ConversationSubscribeItem) -> Vec<EventEnvelope> {
    match item {
        ConversationSubscribeItem::Event(envelope) => vec![envelope],
        ConversationSubscribeItem::Snapshot(snapshot) => snapshot.events,
        _ => Vec::new(),
    }
}

/// The events of `stream` up to the first `job_ended`, while the daemon's clock moves
/// one second whenever a sleep of the daemon ends within it and no event came for
/// [`PACE`] of real time, at most `max_seconds` times. The job's processes end in real
/// time, so a slow or busy machine must get that time before the clock moves on.
async fn until_job_ended(
    daemon: &TestDaemon,
    stream: &mut ItemStream<ConversationSubscribeItem>,
    max_seconds: u32,
) -> Vec<EventEnvelope> {
    let step = Duration::from_secs(1);
    let started = Instant::now();
    let mut seen = Vec::new();
    let mut moved = 0;
    loop {
        tokio::select! {
            biased;
            item = stream.next() => {
                for envelope in envelopes(item.unwrap().unwrap()) {
                    let done = matches!(envelope.event, Event::JobEnded { .. });
                    seen.push(envelope);
                    if done {
                        return seen;
                    }
                }
            }
            () = efr_stdx::time::SystemClock.sleep(PACE) => {
                let waited = started.elapsed();
                assert!(waited < REAL_TIME_LIMIT, "no end in {waited:?} ({moved}s): {seen:#?}");
                let clock = daemon.clock();
                let soon = clock.now().checked_add(step).unwrap();
                if clock.next_deadline().is_some_and(|deadline| deadline <= soon) {
                    assert!(moved < max_seconds, "nothing came in {max_seconds}s: {seen:#?}");
                    clock.advance(step);
                    moved += 1;
                }
            }
        }
    }
}

/// True when the process `pid` is gone or only waits to be reaped.
fn gone(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| stat.contains(") Z "))
}

/// The bytes of the shell state files of the conversation's sandbox, `state.json` and
/// `state.zsh`, or `None` for a file that is not there.
fn state_files(dirs: &TestDirs, conversation_id: ConversationId) -> [Option<Vec<u8>>; 2] {
    let shell_dir = dirs.dirs().runtime().join("sbx").join(conversation_id.to_string());
    ["state.json", "state.zsh"].map(|name| std::fs::read(shell_dir.join(name)).ok())
}

#[tokio::test]
async fn moved_contained_call_keeps_proxy_and_writes_no_state() {
    let test = "moved_contained_call_keeps_proxy_and_writes_no_state";
    if !can_run(test) {
        return;
    }
    let dirs = target_dirs();
    let fifo = gate(&dirs);
    let ca = TestCa::new(&dirs.root().join("ca"), &[REGISTRY.0]).await;
    let server = Server::start(REGISTRY.1, &ca, |_: &Logged| Answer::ok("ok\n"));
    // NOTE: efr reads `./after.sh` as a name; the proxy decides the host inside.
    script(
        &dirs.home().join("project/after.sh"),
        &format!(
            "curl -sS --cacert '{}' -o /dev/null -w 'get=%{{http_code}}\\n' \
             https://registry.test/after",
            ca.pem.display()
        ),
    );
    // The first call exports a variable that stays in the sandbox's shell state; the moved
    // call changes it after the move, and its end must not write it back (DS23).
    let moved_line =
        format!("echo {READY}; read go < ~/gate/go; ./after.sh; export MOVE_KEPT=moved");
    let model = ScriptedModel::reviewed(
        vec![
            ("shell", json!({ "command": "export MOVE_KEPT=first" })),
            ("shell", json!({ "command": moved_line, "timeout_seconds": 600 })),
        ],
        REVIEWER_ASKS,
    );
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let mut options = Options::new(&["curl"]);
    options.hosts = vec![(REGISTRY.0, server.address())];
    options.allow = vec![REGISTRY.0.to_owned()];
    let Some(daemon) = proxy_daemon(test, &dirs, provider, options).await else { return };
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, "check the registry").await;

    let before = state_files(&dirs, conversation_id);
    let kept =
        before.iter().flatten().any(|bytes| String::from_utf8_lossy(bytes).contains("first"));
    assert!(kept, "the first call kept its export in the sandbox's state: {before:?}");
    let call_dir = dirs
        .dirs()
        .runtime()
        .join("sbx")
        .join(conversation_id.to_string())
        .join(call_id.to_string());
    // A contained call keeps its sandbox, so the key press is the answer (OQS2).
    let moved: CallBackgroundResult =
        client.call(background(1, conversation_id, call_id, None)).await.unwrap();
    let CallBackgroundResult::Moved { job_id } = moved else { panic!("{moved:?}") };
    let (job, _, output) = moved_job(&mut stream).await;
    assert_eq!(job.id, job_id);
    assert!(matches!(job.launch, Launch::Contained { .. }), "{job:?}");
    assert!(output.contains("the user moved this command to the background"), "{output}");
    assert!(call_dir.join(efr_sandbox::MOVED_FILE).exists(), "the launcher sees the move");
    assert_eq!(server.log(), Vec::new(), "no request before the move");

    release(&fifo).await;
    let seen = until_job_ended(&daemon, &mut stream, 30).await;

    let Some(Event::JobEnded { state, exit_code, launcher_lost, blocked, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    let text = std::fs::read_to_string(&job.output).unwrap_or_default();
    let log = server.log();
    let report = format!("log: {log:#?}\noutput:\n{text}");
    assert_eq!((state, exit_code), (JobState::Exited, Some(0)), "{report}");
    assert!(!launcher_lost, "the launcher's result in the call's dir was read: {report}");
    // The call reached its proxy host after the move: the job kept the call's proxy.
    assert!(text.contains("get=200"), "{report}");
    let paths: Vec<&str> = log.iter().map(|request| request.path.as_str()).collect();
    assert_eq!(paths, ["/after"], "{report}");
    assert_eq!(blocked, Vec::new(), "{report}");
    // The moved call's end wrote no shell state over the first call's.
    assert_eq!(state_files(&dirs, conversation_id), before, "{report}");
    drop((stream, client));
    daemon.stop().await.unwrap();
}

/// Ctrl+B on a contained command that its call left running in the hidden shell (OQS16,
/// DS30): it moves at once, as a running contained call does. The job gets the call's
/// proxy socket from the list of calls past their timeout, so the command reaches its
/// proxy host after the move, and the launcher's end writes no shell state.
#[tokio::test]
async fn left_contained_command_moves_and_keeps_its_proxy() {
    let test = "left_contained_command_moves_and_keeps_its_proxy";
    if !can_run(test) {
        return;
    }
    let dirs = target_dirs();
    let fifo = gate(&dirs);
    let ca = TestCa::new(&dirs.root().join("ca"), &[REGISTRY.0]).await;
    let server = Server::start(REGISTRY.1, &ca, |_: &Logged| Answer::ok("ok\n"));
    script(
        &dirs.home().join("project/after.sh"),
        &format!(
            "curl -sS --cacert '{}' -o /dev/null -w 'get=%{{http_code}}\\n' \
             https://registry.test/after",
            ca.pem.display()
        ),
    );
    let left_line = format!("echo {READY}; read go < ~/gate/go; ./after.sh");
    let model = ScriptedModel::reviewed(
        vec![("shell", json!({ "command": left_line, "timeout_seconds": 2 }))],
        REVIEWER_ASKS,
    );
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let mut options = Options::new(&["curl"]);
    options.hosts = vec![(REGISTRY.0, server.address())];
    options.allow = vec![REGISTRY.0.to_owned()];
    let Some(daemon) = proxy_daemon(test, &dirs, provider, options).await else { return };
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (conversation_id, call_id, mut stream) =
        start(&daemon, &client, "check the registry").await;
    let before = state_files(&dirs, conversation_id);
    // NOTE: the call's timeout passes on the daemon's clock while the command waits for
    // the gate: the call answers, and its command is left running.
    let started = Instant::now();
    let mut left = false;
    while !left {
        tokio::select! {
            biased;
            item = stream.next() => {
                left = envelopes(item.unwrap().unwrap()).iter().any(|envelope| matches!(
                    envelope.event,
                    Event::ToolCallCompleted { left_running: true, .. }
                ));
            }
            () = efr_stdx::time::SystemClock.sleep(PACE) => {
                assert!(started.elapsed() < REAL_TIME_LIMIT, "the call did not answer");
                daemon.clock().advance(Duration::from_secs(1));
            }
        }
    }
    let moved: CallBackgroundResult =
        client.call(background(1, conversation_id, call_id, None)).await.unwrap();
    let CallBackgroundResult::Moved { job_id } = moved else { panic!("{moved:?}") };
    let call_dir = dirs
        .dirs()
        .runtime()
        .join("sbx")
        .join(conversation_id.to_string())
        .join(call_id.to_string());
    assert!(call_dir.join(efr_sandbox::MOVED_FILE).exists(), "the launcher sees the move");

    release(&fifo).await;
    let seen = until_job_ended(&daemon, &mut stream, 30).await;
    let job = seen
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::JobStarted { job, .. } => Some(job.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no job started: {seen:#?}"));
    assert_eq!((job.id, job.moved_from.map(|from| from.call_id)), (job_id, Some(call_id)));
    assert!(matches!(job.launch, Launch::Contained { .. }));
    let Some(Event::JobEnded { state, exit_code, launcher_lost, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    let text = std::fs::read_to_string(&job.output).unwrap_or_default();
    let report = format!("log: {:#?}\noutput:\n{text}", server.log());
    assert_eq!((state, exit_code), (JobState::Exited, Some(0)), "{report}");
    assert!(!launcher_lost, "{report}");
    assert!(text.contains("get=200"), "the job kept the call's proxy: {report}");
    assert_eq!(state_files(&dirs, conversation_id), before, "{report}");
    drop((stream, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn moved_exit_child_call_is_swept_at_stop() {
    let test = "moved_exit_child_call_is_swept_at_stop";
    if !zsh_enabled(test) {
        return;
    }
    let Some(launcher) = real_launcher(test) else { return };
    let dirs = target_dirs();
    let bin = dirs.create_dir("bin").unwrap();
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let pid_file = dirs.root().join("child");
    // A fake `scp` on the hidden shell's `PATH`: an upload, which runs in the exit child
    // after an answer. It leaves a process in a session of its own (`setsid`) and then
    // runs until a signal ends it. `sh` runs no job control, so `setsid` is no group
    // leader and execs the sleep at once: `$!` is its pid.
    script(
        &bin.join("scp"),
        &format!(
            "setsid sleep 600 > /dev/null 2>&1 &\necho $! > '{}'\necho {READY}\nexec sleep 600",
            pid_file.display()
        ),
    );
    let env = BTreeMap::from([
        ("HOME".to_owned(), dirs.home().display().to_string()),
        ("PATH".to_owned(), format!("{}:/usr/bin:/bin", bin.display())),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.display().to_string()),
    ]);
    crate::full::write_config(
        &dirs,
        "[permissions]\nmode = \"auto\"\n\n[jobs]\nmax_wakes_per_hour = 0\n",
    );
    let model = ScriptedModel::new(vec![
        json!({ "command": "scp notes.txt h:notes.txt", "timeout_seconds": 600 }),
    ]);
    // NOTE: the clock starts after the files of the test were written, so the fake
    // `scp` does not count as a program that changed in the turn.
    let start_at = jiff::Timestamp::now().checked_add(jiff::SignedDuration::from_mins(1)).unwrap();
    let daemon = with_zsh(TestDaemon::builder())
        .clock(efr_test_support::TestClock::starting_at(start_at))
        .dirs(Arc::clone(&dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .config(|config| {
            config.permissions.mode = Mode::Auto;
            config.jobs.max_wakes_per_hour = 0;
        })
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    let sandbox = status.sandbox.unwrap();
    if !sandbox.available {
        // NOTE: just test-sandbox fails when this says skipped on a ready machine.
        skip(test, &format!("the probe says the sandbox is unavailable: {:?}", sandbox.reason));
        drop(client);
        daemon.stop().await.unwrap();
        return;
    }
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: daemon.cwd().to_path_buf(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    std::fs::write(daemon.cwd().join("notes.txt"), "notes\n").unwrap();
    let (conversation_id, call_id, mut stream) = start(&daemon, &client, "copy my notes").await;
    let child: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    assert!(!gone(child), "the command left a process in a session of its own");

    // A call outside the sandbox asks first: the preview, then the yes (DS53).
    let preview: CallBackgroundResult =
        client.call(background(1, conversation_id, call_id, None)).await.unwrap();
    let CallBackgroundResult::Preview { token, .. } = preview else { panic!("{preview:?}") };
    let moved: CallBackgroundResult =
        client.call(background(2, conversation_id, call_id, Some(token))).await.unwrap();
    let CallBackgroundResult::Moved { job_id } = moved else { panic!("{moved:?}") };
    let (job, shell_pid, _) = moved_job(&mut stream).await;
    assert_eq!(job.launch, Launch::Unsandboxed, "{job:?}");

    let stop = Method::JobsStop(JobsStop {
        command_id: command_id(30_000),
        conversation_id: Some(conversation_id),
        job_id: job_id.clone(),
    });
    let stopper = {
        let client = daemon.client_for_tty(TTY).await.unwrap();
        tokio::spawn(async move { client.call::<JobsStopResult>(stop).await })
    };
    let seen = until_job_ended(&daemon, &mut stream, 20).await;

    let Some(Event::JobEnded { state, stopped_by, survivors, .. }) =
        seen.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    assert_eq!((state, stopped_by), (JobState::Stopped, Some(JobStopBy::User)));
    assert_eq!(survivors, Vec::<String>::new());
    let stopped = stopper.await.unwrap().unwrap();
    assert_eq!(stopped.job.id, job_id);
    // Nothing of the moved call is left: not the process that left its session, and
    // not the old hidden shell (DS55).
    for (what, pid) in [("the setsid child", child), ("the old hidden shell", shell_pid)] {
        let ended = Wait::new(what).until(|| gone(pid)).await;
        assert!(ended.is_ok(), "{what} ({pid}) still runs after the stop");
    }
    drop((stream, client));
    daemon.stop().await.unwrap();
}
