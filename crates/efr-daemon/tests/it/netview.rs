//! The network view (phase D1) through efrd with the shared fake launcher of
//! `efr-test-daemon`: a line that only reads the host's network starts as a background
//! job (so no zsh is needed) with `Grant::NetView`, no review and no question, when the
//! probe's self-test passed. The fake launcher keeps the spec of each call, which shows
//! the grants of the launch. A line whose exits the reviewer allows gets their grants,
//! and the view drops beside a grant that the union does not keep. A line that must not
//! run on the test's machine makes the fake fail its setup; its spec still shows the
//! launch.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{ApprovalDecision, Event, EventEnvelope, Grant, Launch, Mode, SandboxStatus};
use efr_sandbox::SandboxSpec;
use efr_test_daemon::{FakeLauncher, TestDaemon};
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::sandbox::{REVIEWER_ALLOWS, ScriptedModel, kinds, ready, run_turn};

/// A ready sandbox whose network view self-test passed.
fn netview_ready() -> SandboxStatus {
    SandboxStatus { netview: true, ..ready() }
}

/// A daemon with the fake launcher, the probe's `status`, and a model that starts
/// `line` as a background job; the reviewer allows every exit.
async fn daemon(
    dirs: &Arc<TestDirs>,
    fake: &FakeLauncher,
    status: SandboxStatus,
    line: &str,
) -> (TestDaemon, Arc<ScriptedModel>) {
    let env = BTreeMap::from([
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ("HOME".to_owned(), dirs.home().display().to_string()),
    ]);
    let input = json!({ "command": line, "background": true, "max_minutes": 1 });
    let model = ScriptedModel::reviewed(vec![("shell", input)], REVIEWER_ALLOWS);
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(dirs))
        .sandbox_launcher(fake.path())
        .probe_override(status)
        .job_env(env)
        .custom_provider(Arc::clone(&model) as Arc<dyn efr_provider::Provider>)
        .start()
        .await
        .unwrap();
    (daemon, model)
}

/// The launch of the job that `events` started.
fn job_launch(events: &[EventEnvelope]) -> Option<Launch> {
    events.iter().find_map(|envelope| match &envelope.event {
        Event::JobStarted { job, .. } => Some(job.launch.clone()),
        _ => None,
    })
}

/// The grants of the one spec that the fake launcher kept.
fn spec_grants(fake: &FakeLauncher) -> Vec<Grant> {
    let specs: Vec<PathBuf> = std::fs::read_dir(fake.dir())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("spec-"))
        })
        .collect();
    let [spec] = specs.as_slice() else { panic!("{specs:?}") };
    SandboxSpec::from_json(&std::fs::read(spec).unwrap()).unwrap().grants
}

#[tokio::test]
async fn netview_line_runs_routine_without_classifier() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
    let (daemon, model) = daemon(&dirs, &fake, netview_ready(), "ss -tln").await;
    // NOTE: the first request already sees the view: efrd built its engine again after
    // the first probe, before it opened its socket.
    let (_, events) =
        run_turn(&daemon, "watch my network while I play", Mode::Auto, ApprovalDecision::Deny)
            .await;
    let kinds = kinds(&events);
    assert!(!kinds.contains(&"exit_requested"), "{kinds:?}");
    assert!(!kinds.contains(&"approval_requested"), "{kinds:?}");
    assert_eq!(model.reviews(), 0, "no review");
    assert_eq!(job_launch(&events), Some(Launch::Contained { grants: vec![Grant::NetView] }));
    assert_eq!(spec_grants(&fake), [Grant::NetView]);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn netview_off_keeps_host_view_exit() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
    // NOTE: the line must not reach the host's network, so the fake fails its setup.
    fake.set("fail").unwrap();
    let (daemon, model) = daemon(&dirs, &fake, ready(), "ss -tln").await;
    let (_, events) =
        run_turn(&daemon, "watch my network while I play", Mode::Auto, ApprovalDecision::Deny)
            .await;
    assert!(kinds(&events).contains(&"exit_requested"), "{:?}", kinds(&events));
    assert_eq!(model.reviews(), 1, "the reviewer judged the host_view exit");
    assert_eq!(spec_grants(&fake), [Grant::OpenNetwork]);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn netview_job_with_a_network_exit_launches_without_the_view() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
    fake.set("fail").unwrap();
    let line = "ss -tln; curl https://x.example/";
    let (daemon, model) = daemon(&dirs, &fake, netview_ready(), line).await;
    let (_, events) =
        run_turn(&daemon, "check x.example while I play", Mode::Auto, ApprovalDecision::Deny).await;
    assert!(model.reviews() > 0, "the reviewer judged the network exits");
    assert_eq!(spec_grants(&fake), [Grant::OpenNetwork], "{:?}", kinds(&events));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn netview_job_with_a_device_exit_launches_without_the_view() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
    // NOTE: the fake never opens the tun device of the test's machine.
    fake.set("fail").unwrap();
    let line = "cat /proc/net/dev /dev/net/tun";
    let (daemon, model) = daemon(&dirs, &fake, netview_ready(), line).await;
    let (_, events) =
        run_turn(&daemon, "read the tun device", Mode::Auto, ApprovalDecision::Deny).await;
    assert_eq!(model.reviews(), 1, "the reviewer judged the device exit");
    let tun = Grant::Device { path: PathBuf::from("/dev/net/tun") };
    assert_eq!(spec_grants(&fake), [tun], "{:?}", kinds(&events));
    daemon.stop().await.unwrap();
}
