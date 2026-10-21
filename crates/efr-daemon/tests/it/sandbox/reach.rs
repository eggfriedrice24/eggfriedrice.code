//! The reach of a contained call (v0.0.4 shell items, DS34-DS48) end to end: the
//! model's calls, efrd's engine, its job table, its forwards and the reach task of each
//! call, the real `efr-sbx` of `just test-sandbox` (`EFR_TEST_SBX_BIN`) with the
//! forwarder in the job and the reach helper in the call, and a real hidden zsh
//! (`EFR_TEST_ZSH=1`). The tests of `efr-sbx` (`reach`) and of efrd's `reach`
//! (`FakeReach`) test each side alone.
//!
//! The dev server of a job is the launcher's own `self-test --echo` on a fixed port of
//! the job's own loopback, in its own network namespace, and the client of a call is
//! `self-test --dial` in the call's own namespace, so no test binds or dials a port of
//! the machine. efrd binds each forwarded port through the test seam, which takes a
//! free port of `127.0.0.1` in its place. A test waits for an event, never for a fixed
//! time.
//!
//! The tests say `skipped:` without the launcher or zsh, or when the probe says that
//! the sandbox is unavailable; `just test-sandbox` fails on such a line on a ready
//! machine. Their dirs stay below the target dir, not below /tmp, which the sandbox
//! replaces with its own.

use std::collections::BTreeMap;
use std::sync::Arc;

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ConversationId, Event,
    EventEnvelope, ForwardOutcome, Launch, Method, Mode, PromptSendResult, TurnSettings,
};
use efr_test_daemon::{TTY, TestDaemon, events_until};
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::netview::target_dirs;
use super::{real_launcher, skip, with_zsh};
use crate::jobs_wake::{Answer, Recorder};
use crate::support::zsh_enabled;

/// The port of the first job's server, in the job's own network namespace.
const PORT: u16 = 20090;

/// The port of the second conversation's job, in that job's own network namespace.
const OTHER_PORT: u16 = 20091;

/// The terminal of a second conversation.
const OTHER_TTY: &str = "/dev/pts/efr-test-b";

/// A background `shell` call of the line of a dev server on `127.0.0.1:port`.
fn server(port: u16) -> Answer {
    let line = format!("\"$_efr_child_dir/efr-sbx\" self-test --echo '127.0.0.1:{port}'");
    Some(("shell", json!({ "command": line, "background": true })))
}

/// The part of a line that dials `127.0.0.1:port` from the call, sends 64 KiB and prints
/// one `dial` line.
fn dial(port: u16) -> String {
    format!("\"$_efr_child_dir/efr-sbx\" self-test --dial '127.0.0.1:{port}' --dial-bytes 65536")
}

/// A foreground `shell` call of `line`.
fn line(line: String) -> Answer {
    Some(("shell", json!({ "command": line })))
}

/// The daemon of a test: the real launcher, the probe's own result, a real hidden zsh,
/// no network, and no wake turns. Its working directory, `<home>/project`, is a
/// registered project. `None` after a `skipped:` line.
async fn real_daemon(test: &str, dirs: &Arc<TestDirs>, answers: Vec<Answer>) -> Option<TestDaemon> {
    if !zsh_enabled(test) {
        return None;
    }
    let launcher = real_launcher(test)?;
    let project = dirs.home().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let env = BTreeMap::from([
        ("HOME".to_owned(), dirs.home().display().to_string()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.display().to_string()),
    ]);
    // NOTE: a job notice would start a wake turn when a job ends; no test here needs
    // one.
    crate::full::write_config(
        dirs,
        "[permissions]\nmode = \"auto\"\n\n[network]\nmode = \"none\"\n\n\
         [jobs]\nmax_wakes_per_hour = 0\n",
    );
    // NOTE: the clock starts after the files of the test were written, so no file of
    // the test counts as a program that changed in the turn (which asks the user).
    let start = jiff::Timestamp::now().checked_add(jiff::SignedDuration::from_mins(1)).unwrap();
    let daemon = with_zsh(TestDaemon::builder())
        .clock(efr_test_support::TestClock::starting_at(start))
        .dirs(Arc::clone(dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .config(|config| {
            config.permissions.mode = Mode::Auto;
            config.jobs.max_wakes_per_hour = 0;
        })
        .custom_provider(Recorder::new(answers))
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
    let add =
        Method::AdminProjectAdd(AdminProjectAdd { path: project, name: None, git_root: false });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    drop(client);
    Some(daemon)
}

/// The turn of the prompt `text` with the command id `n` from `tty` in `auto`, up to its
/// own end; only the turn's own events. No question comes.
async fn turn(
    daemon: &TestDaemon,
    tty: &str,
    n: u128,
    text: &str,
) -> (PromptSendResult, Vec<EventEnvelope>) {
    let client = daemon.client_for_tty(tty).await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(n, text, tty) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let own = sent.turn_id;
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let mut events = events_until(&mut follow, |event| {
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
    events.retain(|envelope| envelope.seq >= sent.seq);
    let kinds: Vec<&str> = events.iter().map(|envelope| envelope.event.kind()).collect();
    assert!(
        matches!(events.last().map(|envelope| &envelope.event), Some(Event::TurnCompleted { .. })),
        "{kinds:?}"
    );
    assert!(!kinds.contains(&"exit_review_started"), "nobody reviewed it: {kinds:?}");
    (sent, events)
}

/// Starts the dev server of `port` as a job from `tty` and waits for its forward;
/// returns the conversation.
async fn forwarded_job(daemon: &TestDaemon, tty: &str, n: u128, port: u16) -> ConversationId {
    let (sent, events) = turn(daemon, tty, n, "start a dev server").await;
    let started = events.iter().any(|envelope| {
        matches!(&envelope.event, Event::JobStarted { job, .. }
            if matches!(job.launch, Launch::Contained { .. }))
    });
    assert!(started, "a contained job started");
    let conversation = sent.conversation_id;
    let client = daemon.client().await.unwrap();
    let mut follow = daemon.follow(&client, conversation).await.unwrap();
    let seen = events_until(
        &mut follow,
        |event| matches!(event, Event::JobPortForwarded { port: p, .. } if *p == port),
    )
    .await
    .unwrap();
    let Some(Event::JobPortForwarded { outcome, .. }) = seen.last().map(|e| e.event.clone()) else {
        unreachable!()
    };
    assert_eq!(outcome, ForwardOutcome::Forwarded);
    conversation
}

/// The launch and the output of the one foreground call of `events`.
fn the_call(events: &[EventEnvelope]) -> (Option<Launch>, String) {
    let launch = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallStarted { launch, .. } => Some(launch.clone()),
        _ => None,
    });
    let output = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, .. } => Some(output.clone()),
        _ => None,
    });
    (launch.flatten(), output.unwrap_or_else(|| panic!("{events:#?}")))
}

/// The `dial` lines that a call printed, in order: whether each passed, and its detail.
fn dials(output: &str) -> Vec<(bool, String)> {
    output
        .lines()
        .filter_map(|line| {
            let start = line.find("{\"name\":\"dial\"")?;
            let value: serde_json::Value = serde_json::from_str(&line[start..]).ok()?;
            Some((value["ok"].as_bool()?, value["detail"].as_str()?.to_owned()))
        })
        .collect()
}

#[tokio::test]
async fn shell_contained_call_reaches_a_forwarded_job_port() {
    let test = "shell_contained_call_reaches_a_forwarded_job_port";
    let dirs = target_dirs();
    let answers = vec![server(PORT), None, line(dial(PORT)), None];
    let Some(daemon) = real_daemon(test, &dirs, answers).await else { return };
    let conversation = forwarded_job(&daemon, TTY, 1, PORT).await;

    let (sent, events) = turn(&daemon, TTY, 2, "check the dev server").await;

    assert_eq!(sent.conversation_id, conversation);
    let (launch, output) = the_call(&events);
    assert!(matches!(launch, Some(Launch::Contained { .. })), "{launch:?}");
    // 64 KiB both ways through the call's helper, efrd and the job's forwarder, with no
    // exit and no question.
    assert_eq!(dials(&output), [(true, "sent 65536, got 65536 back".to_owned())], "{output}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_contained_call_cannot_reach_other_conversation_job_port() {
    let test = "shell_contained_call_cannot_reach_other_conversation_job_port";
    let dirs = target_dirs();
    let check = format!("{}; {}", dial(PORT), dial(OTHER_PORT));
    let answers = vec![server(PORT), None, server(OTHER_PORT), None, line(check), None];
    let Some(daemon) = real_daemon(test, &dirs, answers).await else { return };
    let first = forwarded_job(&daemon, TTY, 1, PORT).await;
    let second = forwarded_job(&daemon, OTHER_TTY, 2, OTHER_PORT).await;
    assert_ne!(first, second, "two conversations");

    let (sent, events) = turn(&daemon, OTHER_TTY, 3, "check both servers").await;

    assert_eq!(sent.conversation_id, second);
    let (launch, output) = the_call(&events);
    assert!(matches!(launch, Some(Launch::Contained { .. })), "{launch:?}");
    // The call reaches its own conversation's job, but nothing listens in the call on
    // the port of the other conversation's job.
    let [(other_ok, other), (own_ok, own)] =
        dials(&output).try_into().unwrap_or_else(|d: Vec<_>| panic!("{d:?} {output}"));
    assert!(!other_ok && other.contains("ConnectionRefused"), "{other}");
    assert!(own_ok, "{own}");
    daemon.stop().await.unwrap();
}
