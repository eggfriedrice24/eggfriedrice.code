//! Full mode next to `auto` on the real launcher (`just test-sandbox`, a real hidden
//! zsh): a full turn runs with no sandbox, and that leaves nothing open for the `auto`
//! work of the same daemon. An `auto` turn after a full turn is contained again, and a
//! job that an `auto` turn started keeps its network proxy while a full turn runs.
//!
//! Every line of a full turn here writes only below the stand-in home. The tests say
//! `skipped:` without zsh or the launcher, or when the probe says that the sandbox is
//! unavailable; `just test-sandbox` fails on such a line on a ready machine. Their dirs
//! stay below the target dir, not below /tmp, which the sandbox replaces with its own.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ApprovalDecision,
    Event, Launch, Method, Mode, NetworkMode,
};
use efr_test_daemon::{TestDaemon, events_until};
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::netview::{launches, outputs, target_dirs};
use super::{real_launcher, skip, with_zsh};
use crate::jobs_wake::{Recorder, turn};
use crate::support::zsh_enabled;

/// A daemon over `model` that allows full mode, with the real launcher, efrd's own
/// probe, a real hidden zsh, `network.mode = "proxy"` with no host behind it, and the
/// project `home/project` registered. `None` after a `skipped:` line without zsh or the
/// launcher, or when the probe says that the sandbox is unavailable.
async fn daemon(test: &str, dirs: &Arc<TestDirs>, model: Arc<Recorder>) -> Option<TestDaemon> {
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
    // NOTE: the clock starts after the files of the test were written, so no script of
    // the test counts as a program that changed in the turn (which asks the user).
    let start = jiff::Timestamp::now().checked_add(jiff::SignedDuration::from_mins(1)).unwrap();
    // NOTE: `admin.project_add` reloads `config.toml`, so the file holds what the
    // builder sets.
    std::fs::create_dir_all(dirs.dirs().config()).unwrap();
    std::fs::write(
        dirs.dirs().config().join("config.toml"),
        "[permissions]\nmode = \"auto\"\nallow_full = true\n\n[network]\nmode = \"proxy\"\n",
    )
    .unwrap();
    let daemon = with_zsh(TestDaemon::builder())
        .clock(efr_test_support::TestClock::starting_at(start))
        .dirs(Arc::clone(dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .network_mode(NetworkMode::Proxy)
        .config(|config| {
            config.permissions.mode = Mode::Auto;
            config.permissions.allow_full = true;
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
    assert!(sandbox.proxy, "the proxy self-test failed: {:?}", sandbox.warnings);
    let add =
        Method::AdminProjectAdd(AdminProjectAdd { path: project, name: None, git_root: false });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    drop(client);
    Some(daemon)
}

/// A `shell` call of `command`.
fn shell(command: &str) -> Option<(&'static str, serde_json::Value)> {
    Some(("shell", json!({ "command": command })))
}

#[tokio::test]
async fn an_auto_turn_after_a_full_turn_is_contained_again() {
    let test = "an_auto_turn_after_a_full_turn_is_contained_again";
    // NOTE: the write to the home dir hides in `sh -c`, so efr predicts no exit, and in
    // auto only the sandbox can stop it.
    let hidden = "sh -c 'echo no > \"$HOME/outside.txt\"'; echo status=$?";
    let model = Recorder::new(vec![shell(hidden), None, shell(hidden), None]);
    let dirs = target_dirs();
    let Some(daemon) = daemon(test, &dirs, model).await else { return };
    let outside = dirs.home().join("outside.txt");

    let (_, events) = turn(&daemon, 1, "write it", Mode::Full, ApprovalDecision::Deny).await;
    assert_eq!(launches(&events), Vec::<Launch>::new(), "a full call names no launch");
    let [output] = outputs(&events).try_into().unwrap();
    assert!(output.contains("status=0"), "{output}");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "no\n");
    std::fs::remove_file(&outside).unwrap();

    let (_, events) = turn(&daemon, 2, "write it again", Mode::Auto, ApprovalDecision::Deny).await;

    assert_eq!(launches(&events), [Launch::contained()]);
    let [output] = outputs(&events).try_into().unwrap();
    assert!(output.contains("Read-only file system"), "{output}");
    assert!(!output.contains("status=0"), "{output}");
    assert!(!outside.exists(), "the sandbox of auto holds again after a full turn");
    daemon.stop().await.unwrap();
}

/// A FIFO at `path`, which a job reads and the test writes.
fn fifo(path: &Path) {
    nix::unistd::mkfifo(path, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
}

#[tokio::test]
async fn a_job_of_an_auto_turn_keeps_its_proxy_after_a_full_turn() {
    let test = "a_job_of_an_auto_turn_keeps_its_proxy_after_a_full_turn";
    if !zsh_enabled(test) {
        return;
    }
    if !Path::new("/usr/bin/curl").is_file() {
        skip(test, "curl is not on PATH");
        return;
    }
    let dirs = target_dirs();
    // NOTE: not `~/go`, which the sandbox reads as the dir of Go's module cache.
    let go = dirs.home().join("release-job");
    fifo(&go);
    // NOTE: efr reads `./get.sh` as a name, so it predicts no host exit for the host
    // inside; the job's proxy refuses it, with 403 for its CONNECT. With no proxy, curl
    // would print 000.
    let script = dirs.home().join("project/get.sh");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nread answer < {}\ncurl -sS -o /dev/null -w 'connect=%{{http_connect}}\\n' https://other.test/\n",
            go.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let job = json!({ "command": "./get.sh", "background": true, "max_minutes": 1 });
    let model =
        Recorder::new(vec![Some(("shell", job)), None, shell("echo full > ~/full.txt"), None]);
    let Some(daemon) = daemon(test, &dirs, model).await else { return };

    let (sent, events) = turn(&daemon, 1, "run get.sh", Mode::Auto, ApprovalDecision::Deny).await;
    let launch = events.iter().find_map(|envelope| match &envelope.event {
        Event::JobStarted { job, .. } => Some(job.launch.clone()),
        _ => None,
    });
    assert_eq!(launch, Some(Launch::contained()), "{:?}", outputs(&events));
    let (_, events) = turn(&daemon, 2, "note it", Mode::Full, ApprovalDecision::Deny).await;
    assert_eq!(launches(&events), Vec::<Launch>::new(), "a full call names no launch");
    assert_eq!(std::fs::read_to_string(dirs.home().join("full.txt")).unwrap(), "full\n");

    // The job reaches the network only after the full turn ended.
    let release = go.clone();
    tokio::task::spawn_blocking(move || std::fs::write(release, b"go\n")).await.unwrap().unwrap();
    let client = daemon.client().await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let events =
        events_until(&mut follow, |event| matches!(event, Event::JobEnded { .. })).await.unwrap();
    let tail = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::JobEnded { tail, .. } => Some(tail.clone()),
            _ => None,
        })
        .unwrap();
    assert!(tail.contains("connect=403"), "the job's proxy refused the host: {tail}");
    drop(client);
    daemon.stop().await.unwrap();
}
