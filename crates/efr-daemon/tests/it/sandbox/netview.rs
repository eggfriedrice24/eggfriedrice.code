//! The network view (phase D1, efr's auto spec, section 8.8) end to end: efrd's own
//! probe, engine and launch, the real `efr-sbx` of `just test-sandbox`
//! (`EFR_TEST_SBX_BIN`) and a real hidden zsh (`EFR_TEST_ZSH=1`).
//!
//! A line that only reads the host's network runs in the sandbox with
//! `Grant::NetView`, with no review and no question, and sees the host's sockets: a
//! listener that the test binds on `127.0.0.1:0` is the marker. Any code in such a call
//! runs in the host's network namespace, so a test also runs code that tries to send:
//! a function named `ss` that an earlier call defined. seccomp refuses its socket.
//!
//! The tests say `skipped:` without the launcher, zsh or `ss`, or when the probe says
//! that the sandbox is unavailable; `just test-sandbox` fails on such a line on a ready
//! machine. Their dirs stay below the target dir, not below /tmp, which the sandbox
//! replaces with its own.

use std::collections::BTreeMap;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    AdminStatus, AdminStatusResult, ApprovalDecision, Event, EventEnvelope, Grant, Launch, Method,
    Mode,
};
use efr_provider::Provider;
use efr_test_daemon::TestDaemon;
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{REVIEWER_ALLOWS, ScriptedModel, kinds, real_launcher, run_turn, skip, with_zsh};
use crate::jobs_wake::{REVIEWER_ASKS, Recorder, turn};
use crate::support::zsh_enabled;

/// Dirs below the target dir, not /tmp, which the real sandbox replaces with its own.
pub(super) fn target_dirs() -> Arc<TestDirs> {
    Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap())
}

/// True when `tool` is a file in a dir of the hidden shell's `PATH`.
fn on_path(tool: &str) -> bool {
    ["/usr/bin", "/bin"].iter().any(|dir| Path::new(dir).join(tool).is_file())
}

/// A daemon over `model` with the real launcher, efrd's own probe and a real hidden
/// zsh. `None` after a `skipped:` line without zsh, the
/// launcher or one of `tools`, or when the probe says that the sandbox is unavailable.
/// The probe's network view self-test must pass on a machine where the sandbox is
/// ready.
pub(super) async fn real_daemon(
    test: &str,
    dirs: &Arc<TestDirs>,
    model: Arc<dyn Provider>,
    tools: &[&str],
) -> Option<TestDaemon> {
    if !zsh_enabled(test) {
        return None;
    }
    let launcher = real_launcher(test)?;
    if let Some(tool) = tools.iter().find(|tool| !on_path(tool)) {
        skip(test, &format!("{tool} is not on PATH"));
        return None;
    }
    std::fs::create_dir_all(dirs.home().join("project")).unwrap();
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let env = BTreeMap::from([
        ("HOME".to_owned(), dirs.home().display().to_string()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.display().to_string()),
    ]);
    // NOTE: the clock starts after the files of the test were written, so no file of
    // the test counts as a program that changed in the turn (which asks the user).
    let start = jiff::Timestamp::now().checked_add(jiff::SignedDuration::from_mins(1)).unwrap();
    let daemon = with_zsh(TestDaemon::builder())
        .clock(efr_test_support::TestClock::starting_at(start))
        .dirs(Arc::clone(dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .config(|config| config.permissions.mode = Mode::Auto)
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    drop(client);
    let sandbox = status.sandbox.unwrap();
    if !sandbox.available {
        // NOTE: just test-sandbox fails when this says skipped on a ready machine.
        skip(test, &format!("the probe says the sandbox is unavailable: {:?}", sandbox.reason));
        daemon.stop().await.unwrap();
        return None;
    }
    assert!(sandbox.netview, "the network view self-test failed: {:?}", sandbox.warnings);
    Some(daemon)
}

/// The launch of each `shell` call of `events`, from `tool_call_started`.
pub(super) fn launches(events: &[EventEnvelope]) -> Vec<Launch> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallStarted { launch: Some(launch), .. } => Some(launch.clone()),
            _ => None,
        })
        .collect()
}

/// The output of each finished tool call of `events`.
pub(super) fn outputs(events: &[EventEnvelope]) -> Vec<String> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { output, .. } => Some(output.clone()),
            _ => None,
        })
        .collect()
}

/// Asserts that the turn of `events` made no exit, asked nothing and reviewed nothing.
pub(super) fn assert_routine(events: &[EventEnvelope], reviews: usize) {
    let kinds = kinds(events);
    assert!(!kinds.contains(&"exit_requested"), "{kinds:?}");
    assert!(!kinds.contains(&"approval_requested"), "{kinds:?}");
    assert_eq!(reviews, 0, "no review");
}

/// A listener on the host's loopback, the test's own marker: its port shows in the
/// host's socket table only.
fn listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

#[tokio::test]
async fn shell_netview_line_runs_routine_without_classifier() {
    let test = "shell_netview_line_runs_routine_without_classifier";
    let dirs = target_dirs();
    let (_listener, port) = listener();
    let model =
        ScriptedModel::reviewed(vec![("shell", json!({ "command": "ss -Htln" }))], REVIEWER_ASKS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let Some(daemon) = real_daemon(test, &dirs, provider, &["ss"]).await else { return };
    let (_, events) =
        run_turn(&daemon, "what listens on my machine?", Mode::Auto, ApprovalDecision::Deny).await;
    assert_routine(&events, model.reviews());
    assert_eq!(launches(&events), [Launch::Contained { grants: vec![Grant::NetView] }]);
    let [output] = outputs(&events).try_into().unwrap();
    // NOTE: the sandbox's private namespace has no such listener; the host's has.
    assert!(output.contains(&format!("127.0.0.1:{port}")), "{output}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_netview_send_fails_in_the_call() {
    let test = "shell_netview_send_fails_in_the_call";
    let dirs = target_dirs();
    let (listener, port) = listener();
    // NOTE: the hidden shell drops a function of its rc file that is named like a
    // program that a rule trusts, such as `ss`, so the function comes from the state of
    // an earlier call: the first line defines it (the reviewer allows the network that
    // `ztcp` names). efr reads only `ss -tln` in the second line, so that call gets the
    // network view, and the function runs any code there: the launcher's self-test of a
    // TCP socket, then a connect to the test's listener with zsh's own TCP module.
    let define = format!(
        "ss() {{ \"$_efr_child_dir/efr-sbx\" self-test --net-view --check tcp4; \
         zmodload zsh/net/tcp && ztcp 127.0.0.1 {port}; print -r -- ztcp=$? }}"
    );
    let calls =
        vec![("shell", json!({ "command": define })), ("shell", json!({ "command": "ss -tln" }))];
    let model = ScriptedModel::reviewed(calls, REVIEWER_ALLOWS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let Some(daemon) = real_daemon(test, &dirs, provider, &["ss"]).await else { return };
    let (_, events) =
        run_turn(&daemon, "what listens on my machine?", Mode::Auto, ApprovalDecision::Deny).await;
    let exits = kinds(&events).iter().filter(|kind| **kind == "exit_requested").count();
    assert_eq!((exits, model.reviews()), (1, 1), "only the definition was reviewed");
    assert_eq!(
        launches(&events),
        [
            Launch::Contained { grants: vec![Grant::OpenNetwork] },
            Launch::Contained { grants: vec![Grant::NetView] },
        ]
    );
    let [_, output] = outputs(&events).try_into().unwrap();
    // EAFNOSUPPORT (97) from seccomp at socket(), not a refusal of a later layer.
    assert!(output.contains(r#"{"name":"tcp4","ok":true,"detail":"errno 97"}"#), "{output}");
    assert!(output.contains("ztcp=1"), "{output}");
    assert!(
        listener.accept().is_err_and(|error| error.kind() == std::io::ErrorKind::WouldBlock),
        "the listener accepted nothing"
    );
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_netview_job_runs_routine() {
    let test = "shell_netview_job_runs_routine";
    let dirs = target_dirs();
    let (listener, port) = listener();
    // NOTE: one connection to the marker, so `ss -tin` (no listeners) has a row of it.
    let _client = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let accepted =
        Wait::new("the marker's connection").until_some(|| listener.accept().ok()).await.unwrap();
    let line = "while :; do ss -tin; sleep 1; done";
    let job = json!({ "command": line, "background": true, "max_minutes": 1 });
    let model = Recorder::reviewed(
        vec![Some(("shell", job)), None, Some(("job_stop", json!({ "id": "j1" }))), None],
        REVIEWER_ASKS,
    );
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let Some(daemon) = real_daemon(test, &dirs, provider, &["ss"]).await else { return };
    let (sent, events) =
        turn(&daemon, 1, "watch my network while I play", Mode::Auto, ApprovalDecision::Deny).await;
    assert_routine(&events, model.reviews());
    let info = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::JobStarted { job, .. } => Some(job.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no job started: {:?}", outputs(&events)));
    assert_eq!(info.launch, Launch::Contained { grants: vec![Grant::NetView] });
    // The job sees the host's connection to the marker.
    let shown = format!("127.0.0.1:{port}");
    let output: PathBuf = info.output.clone();
    Wait::new("the job's output")
        .until_some(|| std::fs::read_to_string(&output).ok().filter(|text| text.contains(&shown)))
        .await
        .unwrap();
    let (_, events) = turn(&daemon, 2, "stop watching", Mode::Auto, ApprovalDecision::Deny).await;
    assert_routine(&events, model.reviews());
    let client = daemon.client().await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let ended = efr_test_daemon::events_until(
        &mut follow,
        |event| matches!(event, Event::JobEnded { job_id, .. } if job_id.as_str() == "j1"),
    )
    .await
    .unwrap();
    let Some(Event::JobEnded { state, .. }) = ended.last().map(|envelope| &envelope.event) else {
        unreachable!()
    };
    assert_eq!(*state, efr_protocol::JobState::Stopped);
    drop((client, accepted));
    daemon.stop().await.unwrap();
}
