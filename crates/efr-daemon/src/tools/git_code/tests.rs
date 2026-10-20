//! The git code check of an outside git line in the toolbox: the facts of the
//! question, the check again before the run, and the calls past their timeout.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use efr_config::Settings;
use efr_conversation::Toolbox as _;
use efr_permissions::{
    Approver, ConversationPolicy, DecisionInput, Engine, Locations, Requirements,
};
use efr_protocol::{ExitKind, GitCodeFacts, Mode, Origin, Scope};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::GIT_CODE_CHANGED;
use crate::tools::tests::{Kept, call_of, kept};

async fn git(dir: &Path, args: &[&str]) {
    let status = efr_stdx::process::command("git", dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .await
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A toolbox whose home holds the registered project `p/app`, a git repository with
/// the nested repository `vendor/x`. Returns the toolbox, the project and the temp dir.
async fn machine() -> (Kept, PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let kept = kept(&root, Settings::default()).await;
    let app = kept.home.join("p/app");
    std::fs::create_dir_all(app.join("vendor/x")).unwrap();
    git(&app, &["init", "-q", "-b", "main"]).await;
    git(&app.join("vendor/x"), &["init", "-q", "-b", "main"]).await;
    let id = efr_protocol::ProjectId::from_uuid(uuid::Uuid::from_u128(7));
    let mut projects = efr_scope::Registry::empty();
    projects.register(id, &app, None).unwrap();
    projects.save(&root.join("c/projects.toml")).unwrap();
    (kept, app, dir)
}

/// An executable hook `name` in `hooks`.
fn hook(hooks: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(hooks).unwrap();
    let path = hooks.join(name);
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The git code fact of the call `n` of turn 2 that runs `line` in `dir`.
async fn git_code(kept: &Kept, n: u128, line: &str, dir: &Path) -> Option<GitCodeFacts> {
    requirements(kept, n, line, dir).await.facts.unwrap().git_code
}

/// The requirements of the call `n` of turn 2 that runs `line` in `dir`, with its facts.
async fn requirements(kept: &Kept, n: u128, line: &str, dir: &Path) -> Requirements {
    let call = call_of(2, n, "shell", json!({ "command": line }), dir);
    kept.toolbox.requirements(&call).await.unwrap()
}

/// Who may approve the `upload` exit of `requirements` in `auto`.
fn upload_approver(home: &Path, requirements: Requirements) -> Approver {
    let engine = Engine::with_defaults(Locations::new(home).unwrap());
    let decision = engine.decide(&DecisionInput {
        requirements,
        scope: Scope::Machine,
        origin: Origin::Shell,
        mode: Mode::Auto,
        conversation_policy: ConversationPolicy::new(home.join("scratch")),
    });
    decision.exits().find(|need| need.kind == ExitKind::Upload).unwrap().approver
}

/// The writable parts of `facts`, as `(item, file)`.
fn writable(facts: &GitCodeFacts) -> Vec<(String, PathBuf)> {
    facts
        .items
        .iter()
        .filter(|item| item.writable)
        .map(|item| (item.item.clone(), item.file.clone()))
        .collect()
}

#[tokio::test]
async fn git_code_changed_after_the_answer_asks_again() {
    let (kept, app, _dir) = machine().await;
    let nested = app.join("vendor/x");
    let line = "git push origin main";
    let call = call_of(2, 1, "shell", json!({ "command": line }), &nested);
    let requirements = kept.toolbox.requirements(&call).await.unwrap();
    let asked = requirements.facts.unwrap().git_code.unwrap();
    assert!(asked.clean(), "{asked:?}");
    let (sandbox, _) = kept.toolbox.sandbox.as_ref().unwrap();

    // With nothing changed, the check before the run lets the call start.
    assert_eq!(kept.toolbox.recheck_git_code(&call.context, sandbox).await, None);

    // A hook that a contained call planted after the question stops the run; the
    // question of the next call names it.
    let call = call_of(2, 2, "shell", json!({ "command": line }), &nested);
    let _ = kept.toolbox.requirements(&call).await.unwrap();
    let planted = hook(&nested.join(".git/hooks"), "pre-push");
    let stop = kept.toolbox.recheck_git_code(&call.context, sandbox).await;
    assert_eq!(stop.as_deref(), Some(GIT_CODE_CHANGED));
    let again = git_code(&kept, 3, line, &nested).await.unwrap();
    assert_eq!(writable(&again), [("hooks/pre-push".to_owned(), planted)]);
    // The check is the call's own: a second check of the same call finds nothing kept.
    assert_eq!(kept.toolbox.recheck_git_code(&call.context, sandbox).await, None);
}

#[tokio::test]
async fn timed_out_call_makes_outside_git_user_only() {
    let (kept, app, dir) = machine().await;
    // The project's own hook lies in its pinned git dir, which no plan lets a call
    // write.
    let own = hook(&app.join(".git/hooks"), "pre-push");
    let line = "git push origin main";
    let facts = git_code(&kept, 1, line, &app).await.unwrap();
    assert!(facts.clean(), "{facts:?}");

    // A contained call that runs on past its timeout in the project keeps its write
    // roots, and a job's plan is not kept, so its pins count for nothing.
    let call_dir = dir.path().join("r/sbx/c/calls/1");
    std::fs::create_dir_all(&call_dir).unwrap();
    kept.toolbox.git_checks.keep_timed_out(call_dir.clone(), vec![app.clone()]);
    let found = requirements(&kept, 2, line, &app).await;
    let facts = found.facts.as_ref().unwrap().git_code.clone().unwrap();
    assert_eq!(writable(&facts), [("hooks/pre-push".to_owned(), own.clone())]);
    assert_eq!(upload_approver(&kept.home, found), Approver::User);

    // Once its launcher wrote its result, the roots count no more.
    std::fs::write(call_dir.join(efr_sandbox::RESULT_FILE), "{}").unwrap();
    let facts = git_code(&kept, 3, line, &app).await.unwrap();
    assert!(facts.clean(), "{facts:?}");
}

#[tokio::test]
async fn a_line_with_no_git_code_ask_keeps_no_check() {
    let (kept, app, _dir) = machine().await;
    let call = call_of(2, 1, "shell", json!({ "command": "git status" }), &app);
    let requirements = kept.toolbox.requirements(&call).await.unwrap();
    assert_eq!(requirements.facts.unwrap().git_code, None);
    assert!(!kept.toolbox.git_checks.has(call.context.call_id));
}
