//! The surface guard of a contained job while it runs (efr's auto spec 3.18, phase
//! C), on the real `efr-sbx` of `just test-sandbox`: a job makes a new repository with a
//! hook in the project, and before the next contained call of the conversation runs,
//! efrd's `efr-sbx guard-check` moves the hook to the quarantine and stops the job. The
//! test drives a real zsh for the foreground call, so it needs `EFR_TEST_ZSH=1`, and it
//! skips without the real launcher: the fake launcher has no guard.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ApprovalDecision,
    Event, EventEnvelope, JobStopBy, Method, Mode, PromptSendResult, SurfaceChange, TurnSettings,
};
use efr_test_daemon::{TTY, TestDaemon, events_until};
use efr_test_support::{TestDirs, Wait};
use serde_json::json;

use super::{JOB_WORK, git, real_launcher, run_turn_with, skip, with_zsh};
use crate::jobs_undo::Script;
use crate::support::zsh_enabled;

/// Dirs below the target dir, not /tmp, which the real sandbox replaces with its own.
fn dirs() -> Arc<TestDirs> {
    Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap())
}

#[tokio::test]
async fn shell_job_planted_hook_quarantined_before_next_turn() {
    let test = "shell_job_planted_hook_quarantined_before_next_turn";
    let next = json!({ "command": "cat a.txt" });
    let Some((seen, hook)) = plant_then(test, next).await else { return };
    let (stopped_by, surface_changes, quarantined) = guard_end(&seen);
    assert_eq!(stopped_by, Some(JobStopBy::Guard), "{surface_changes:?}");
    assert!(quarantined, "{surface_changes:?}");
    assert!(!hook.exists(), "the hook went to the quarantine before the next call ran");
}

/// A second contained job in the project gets the same check before its launcher
/// records the surface, so it never takes the planted hook as its baseline.
#[tokio::test]
async fn shell_job_planted_hook_quarantined_before_next_job() {
    let test = "shell_job_planted_hook_quarantined_before_next_job";
    let next = json!({ "command": "cat a.txt", "background": true });
    let Some((seen, hook)) = plant_then(test, next).await else { return };
    let (stopped_by, surface_changes, quarantined) = guard_end(&seen);
    assert_eq!(stopped_by, Some(JobStopBy::Guard), "{surface_changes:?}");
    assert!(quarantined, "{surface_changes:?}");
    let guarded = seen
        .iter()
        .find(|envelope| {
            matches!(&envelope.event, Event::JobEnded { job_id, .. } if job_id.as_str() == "j1")
        })
        .map(|envelope| envelope.seq);
    let second = seen
        .iter()
        .find(|envelope| {
            matches!(&envelope.event, Event::JobStarted { job, .. } if job.id.as_str() == "j2")
        })
        .map(|envelope| envelope.seq);
    let (Some(guarded), Some(second)) = (guarded, second) else {
        panic!("no guard stop or no second job: {:?}", super::kinds(&seen))
    };
    assert!(guarded < second, "the first job's guard stop {guarded} comes before {second}");
    assert!(!hook.exists(), "the hook went to the quarantine before the next job ran");
}

/// The end of the first job of `seen`: how it stopped, the surface changes and whether
/// one went to the quarantine.
fn guard_end(seen: &[EventEnvelope]) -> (Option<JobStopBy>, Vec<SurfaceChange>, bool) {
    let ended = seen.iter().find_map(|envelope| match &envelope.event {
        Event::JobEnded { job_id, stopped_by, surface_changes, quarantined, .. }
            if job_id.as_str() == "j1" =>
        {
            Some((*stopped_by, surface_changes.clone(), *quarantined))
        }
        _ => None,
    });
    let Some(ended) = ended else {
        panic!("the job ran on through the next turn: {:?}", super::kinds(seen))
    };
    ended
}

/// A job that plants a hook in a project, then a turn of `next`: the events of the two
/// turns and the hook's path, after the daemon stopped. `None` when the test skips.
#[expect(clippy::print_stderr, reason = "the cost of a guard check is a report of the test")]
async fn plant_then(
    test: &'static str,
    next: serde_json::Value,
) -> Option<(Vec<EventEnvelope>, std::path::PathBuf)> {
    if !zsh_enabled(test) {
        return None;
    }
    let launcher = real_launcher(test)?;
    let dirs = dirs();
    let project = dirs.create_dir("home/project").unwrap();
    git(&project, &["init", "-q", "-b", "main"]).await;
    std::fs::write(project.join("a.txt"), "a\n").unwrap();
    // NOTE: efr refuses a job whose line writes a hook itself, so the job runs a
    // script that does it, as a hostile build script would. It waits for `go`, so the
    // test can time one clean check first.
    let plant = "git init -q sub\nprintf '#!/bin/sh\\n' > sub/.git/hooks/pre-commit\n: > planted\n";
    std::fs::write(project.join("plant.sh"), plant).unwrap();
    let line = "until [ -e go ]; do sleep 0.1; done; sh plant.sh; sleep 30";
    // NOTE: a foreground call first starts the hidden shell, which writes the sandbox
    // files that a job's launcher reads too; the job's environment comes from the seam.
    let model = Script::new(vec![
        Some(("shell", json!({ "command": "cat a.txt" }))),
        Some(("shell", json!({ "command": line, "background": true }))),
        None,
        Some(("shell", next)),
        None,
    ]);
    let user_runtime = dirs.create_dir("xrt").unwrap();
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
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .custom_provider(model)
        .shell_env(env)
        .job_env(job_env)
        .sandbox_launcher(&launcher)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    if !status.sandbox.is_some_and(|sandbox| sandbox.available) {
        skip(test, "the probe says the sandbox is unavailable");
        daemon.stop().await.unwrap();
        return None;
    }
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: project.clone(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();

    let (sent, events) =
        run_turn_with(&daemon, TTY, 1, "plant it as a job", Mode::Auto, ApprovalDecision::Deny)
            .await;
    let call = events.iter().find_map(|envelope| match &envelope.event {
        Event::JobStarted { call_id, .. } => Some(*call_id),
        _ => None,
    });
    let Some(call) = call else { panic!("no job started: {:?}", super::completed(&events)) };
    // NOTE: the cost of one check while nothing changed; check C3b-2 of the plan.
    let conversation = sent.conversation_id.to_string();
    let call_dir = dirs.dirs().runtime().join("sbx").join(&conversation).join("jobs");
    let state_dir = dirs.dirs().state().join("sandbox").join(&conversation).join("jobs");
    let (call_dir, state_dir) = (call_dir.join(call.to_string()), state_dir.join(call.to_string()));
    let begin = Instant::now();
    let checked = efr_stdx::process::command(&launcher, "/")
        .args(["guard-check", "--call-dir"])
        .arg(&call_dir)
        .arg("--state-dir")
        .arg(&state_dir)
        .status()
        .await
        .unwrap();
    eprintln!("{test}: one guard check took {} ms ({checked})", begin.elapsed().as_millis());
    assert!(checked.success(), "nothing changed yet");
    std::fs::write(project.join("go"), "").unwrap();
    let hook = project.join("sub/.git/hooks/pre-commit");
    Wait::new("the job's hook")
        .limit(JOB_WORK)
        .until(|| project.join("planted").exists())
        .await
        .unwrap();
    assert!(hook.exists(), "the job planted its hook");

    let Method::PromptSend(mut params) = daemon.prompt(2, "show a.txt", TTY) else {
        unreachable!()
    };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let tty_client = daemon.client_for_tty(TTY).await.unwrap();
    let _: PromptSendResult = tty_client.call(Method::PromptSend(params)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let mut ends = 0;
    let seen = events_until(&mut follow, |event| {
        ends +=
            usize::from(matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. }));
        ends == 2
    })
    .await
    .unwrap();
    daemon.stop().await.unwrap();
    Some((seen, hook))
}
