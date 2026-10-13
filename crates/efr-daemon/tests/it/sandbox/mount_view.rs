//! The mount view (phase D1, efr's auto spec, section 8.8) end to end: efrd takes the
//! snapshot of the host's mounts for a line that runs `df`, the real `efr-sbx` of
//! `just test-sandbox` binds it into the call, and the child shell's `df` prints it.
//! So `df` runs with no review and no question and shows the host's mounts, not the
//! sandbox's binds below the test's dirs.
//!
//! The test says `skipped:` without the launcher, zsh or `df`, or when the probe says
//! that the sandbox is unavailable; `just test-sandbox` fails on such a line on a ready
//! machine.

use std::sync::Arc;

use efr_protocol::{ApprovalDecision, Launch, Mode};
use efr_provider::Provider;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::netview::{assert_routine, launches, outputs, real_daemon, target_dirs};
use super::{ScriptedModel, run_turn};
use crate::jobs_wake::REVIEWER_ASKS;

#[tokio::test]
async fn shell_df_routine_shows_host_mounts() {
    let test = "shell_df_routine_shows_host_mounts";
    let dirs = target_dirs();
    let line = "df --output=target";
    let model = ScriptedModel::reviewed(vec![("shell", json!({ "command": line }))], REVIEWER_ASKS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let Some(daemon) = real_daemon(test, &dirs, provider, &["df"]).await else { return };
    let (_, events) =
        run_turn(&daemon, "how full are my disks?", Mode::Auto, ApprovalDecision::Deny).await;
    assert_routine(&events, model.reviews());
    assert_eq!(launches(&events), [Launch::contained()]);
    let [output] = outputs(&events).try_into().unwrap();
    // NOTE: the last line is efr's note of the exit code and the working directory.
    let targets: Vec<&str> =
        output.lines().filter(|line| !line.starts_with("[exit code")).map(str::trim_end).collect();
    assert_eq!(targets.first(), Some(&"Mounted on"), "{output}");
    assert!(targets.contains(&"/"), "{output}");
    // NOTE: the sandbox binds the project, the scratch dir and the masks below the
    // test's dirs; the host has no mount there.
    let root = dirs.root();
    assert!(
        !targets.iter().any(|target| std::path::Path::new(target).starts_with(root)),
        "a row of the sandbox's own mounts: {output}"
    );
    daemon.stop().await.unwrap();
}
