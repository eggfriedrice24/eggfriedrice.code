//! Always-allow rules (phase E, efr's auto spec 11.2 and 11.3) on the real `efr-sbx` of
//! `just test-sandbox` with a real hidden zsh: a write that a `[[sandbox.allow]]` rule
//! covers runs with no review and no question, and the surface guard still checks the
//! git dirs below the dir that the rule opened (DE41), so a hook that the call plants
//! there goes to the quarantine.
//!
//! The test says `skipped:` without the launcher, zsh or git, or when the probe says that
//! the sandbox is unavailable; `just test-sandbox` fails on such a line on a ready
//! machine. Its dirs stay below the target dir, not below /tmp, which the sandbox
//! replaces with its own.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ApprovalDecision,
    Event, JudgeKind, Method, Mode, Verdict,
};
use efr_provider::Provider;
use efr_test_daemon::TestDaemon;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::netview::target_dirs;
use super::{ScriptedModel, git, kinds, real_launcher, run_turn, skip, with_zsh};
use crate::jobs_wake::REVIEWER_ASKS;
use crate::support::zsh_enabled;

/// safety 3 of the phase E plan: a rule covers a write grant of a repository that is not
/// a project; the call makes a new repository below it with a hook. The guard of the
/// launcher records the git dirs below the granted dir, so the hook goes to the
/// quarantine when the call ends, as it would below a write root.
#[tokio::test]
async fn always_allow_write_rule_repo_hooks_guarded() {
    let test = "always_allow_write_rule_repo_hooks_guarded";
    if !zsh_enabled(test) {
        return;
    }
    let Some(launcher) = real_launcher(test) else { return };
    if !["/usr/bin/git", "/bin/git"].iter().any(|git| Path::new(git).is_file()) {
        skip(test, "git is not in /usr/bin or /bin");
        return;
    }
    let dirs = target_dirs();
    let project = dirs.create_dir("home/project").unwrap();
    let repo = dirs.create_dir("home/repo").unwrap();
    git(&repo, &["init", "-q", "-b", "main"]).await;
    // NOTE: the rule opens a dir of the repository, not its top: efrd copies a dir that
    // a write opens outside the roots into its home store first, and that store cannot
    // hold a path with a `.git` part.
    let docs = dirs.create_dir("home/repo/docs").unwrap();
    // NOTE: efr reads `./plant.sh` as a name, so only the model's `needs.write` names
    // the repository; the script plants the hook as a hostile build script would.
    let plant = project.join("plant.sh");
    std::fs::write(
        &plant,
        "#!/bin/sh\ncd \"$1\" || exit 9\ngit init -q sub\n\
         printf '#!/bin/sh\\n' > sub/.git/hooks/pre-commit\necho planted=$?\n",
    )
    .unwrap();
    std::fs::set_permissions(&plant, std::fs::Permissions::from_mode(0o755)).unwrap();
    let under = docs.display().to_string();
    let config = format!(
        "[permissions]\nmode = \"auto\"\n\n[[sandbox.allow]]\nkind = \"write\"\nunder = {under:?}\n"
    );
    std::fs::create_dir_all(dirs.dirs().config()).unwrap();
    std::fs::write(dirs.dirs().config().join("config.toml"), config).unwrap();

    let input = json!({
        "command": format!("./plant.sh '{under}'"),
        "needs": { "write": [under.clone()] },
    });
    // NOTE: the reviewer hands each exit to the user, who says no: only the rule can
    // let the call run.
    let model = ScriptedModel::reviewed(vec![("shell", input)], REVIEWER_ASKS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
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
    let rule = efr_config::AllowRule::Write { under: under.clone(), project: None };
    let daemon = with_zsh(TestDaemon::builder())
        .clock(efr_test_support::TestClock::starting_at(start))
        .dirs(Arc::clone(&dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .config(move |config| {
            config.permissions.mode = Mode::Auto;
            config.sandbox.allow.push(rule);
        })
        .custom_provider(provider)
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
        return;
    }
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: project.clone(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    drop(client);

    let (_, events) =
        run_turn(&daemon, "set up the sub repo", Mode::Auto, ApprovalDecision::Deny).await;
    daemon.stop().await.unwrap();
    let report = format!("{:?}\n{events:#?}", kinds(&events));
    assert_eq!(model.reviews(), 0, "the rule judged the exit: {report}");
    assert!(!kinds(&events).contains(&"approval_requested"), "{report}");
    let judged = events.iter().find_map(|envelope| match &envelope.event {
        Event::ExitJudged { judge, verdict, category, .. } => {
            Some((*judge, *verdict, category.clone()))
        }
        _ => None,
    });
    assert_eq!(
        judged,
        Some((JudgeKind::Always, Verdict::Allow, Some("sandbox.allow[0]".to_owned()))),
        "{report}"
    );
    let (output, summary) = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { output, sandbox, .. } => {
                Some((output.clone(), sandbox.clone()))
            }
            _ => None,
        })
        .unwrap();
    let summary = summary.unwrap_or_else(|| panic!("no summary: {output}"));
    assert!(output.contains("planted=0"), "the covered call wrote below the rule: {output}");
    let hook = docs.join("sub/.git/hooks/pre-commit");
    let quarantined: Vec<_> =
        summary.surface_changes.iter().filter(|change| change.quarantined).collect();
    assert!(!quarantined.is_empty(), "{output}\n{:?}", summary.surface_changes);
    assert!(!hook.exists(), "the guard moved the hook to the quarantine: {output}");
}
