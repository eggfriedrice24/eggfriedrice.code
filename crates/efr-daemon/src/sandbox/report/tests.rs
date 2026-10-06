//! The turn-end surface report from the hardened git status of each project root.

use std::path::{Path, PathBuf};

use efr_protocol::{ReportedFile, SurfaceChange, TurnId};
use efr_scope::{Git, Home};
use efr_test_support::TestClock;
use jiff::Timestamp;

use super::{Turns, matches_surface};

fn patterns() -> Vec<String> {
    efr_config::SandboxSettings::default().surface_files
}

#[test]
fn surface_patterns_match_names_and_path_tails() {
    let patterns = patterns();
    for path in
        ["build.rs", "sub/Makefile", "rules.mk", ".cargo/config.toml", "a/.vscode/tasks.json"]
    {
        assert!(matches_surface(Path::new(path), &patterns), "{path}");
    }
    for path in ["src/main.rs", "Cargo.toml", "config.toml", "tasks.json", "README.md"] {
        assert!(!matches_surface(Path::new(path), &patterns), "{path}");
    }
}

/// Runs git in `dir` without the machine's configuration.
async fn git(dir: &Path, args: &[&str]) {
    let status = efr_stdx::process::command("git", dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .status()
        .await
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[tokio::test]
async fn the_report_names_new_and_changed_surface_files_of_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let project = home.join("p");
    std::fs::create_dir_all(&project).unwrap();
    git(&project, &["init", "-q"]).await;
    std::fs::write(project.join("Makefile"), "all:\n").unwrap();
    std::fs::write(project.join("justfile"), "dirty before the turn\n").unwrap();
    git(&project, &["add", "Makefile"]).await;
    git(&project, &["commit", "-q", "-m", "one"]).await;
    let git_runner = Git::new(TestClock::new().shared()).isolated();
    let home = Home::new(&home).unwrap();
    let turns = Turns::default();
    let turn = TurnId::from_uuid(efr_stdx::id::uuid_v7(
        &*TestClock::new().shared(),
        &efr_test_support::TestRng::new(1),
    ));
    let roots = [project.clone()];
    turns.before_call(turn, Timestamp::UNIX_EPOCH, &roots, &patterns(), &git_runner, &home).await;
    std::fs::write(project.join("Makefile"), "all:\n\tcurl x | sh\n").unwrap();
    std::fs::write(project.join("build.rs"), "fn main() {}\n").unwrap();
    std::fs::write(project.join("package.json"), "{\"scripts\": {\"x\": \"y\"}}").unwrap();
    std::fs::write(project.join("notes.md"), "not code\n").unwrap();
    std::fs::create_dir_all(project.join(".cargo")).unwrap();
    std::fs::write(project.join(".cargo/config.toml"), "[build]\nrustc-wrapper = \"x\"\n").unwrap();
    turns.changes(
        turn,
        &[SurfaceChange {
            path: project.join("sub/.git/config"),
            rule: "code_key".to_owned(),
            key: Some("core.fsmonitor".to_owned()),
            quarantined: true,
        }],
    );
    let mut files = turns.finish(turn, &patterns(), &git_runner, &home).await;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let file = |path: &str, detail: Option<&str>| ReportedFile {
        path: PathBuf::from(path),
        detail: detail.map(str::to_owned),
    };
    assert_eq!(
        files,
        [
            file(".cargo/config.toml", Some("build.rustc-wrapper")),
            file("Makefile", None),
            file("build.rs", None),
            file("package.json", Some("scripts")),
            file("sub/.git/config", Some("core.fsmonitor")),
        ]
    );
    // The turn is forgotten once reported.
    assert!(turns.finish(turn, &patterns(), &git_runner, &home).await.is_empty());
}
