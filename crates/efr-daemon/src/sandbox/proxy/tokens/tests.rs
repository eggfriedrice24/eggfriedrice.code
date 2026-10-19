//! The hosts that a call may not reach because a file that the sandbox reads holds a
//! credential for them (DP25), from real files and real git configs with `.test` hosts.

use std::path::Path;

use efr_permissions::Preset;
use efr_scope::{Git, Home};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;

use super::denied;

/// Runs git in `dir` without the user's configuration.
async fn git(dir: &Path, args: &[&str]) {
    let status = efr_stdx::process::command("git", dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .await
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

async fn repo(dir: &Path, url: &str) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]).await;
    git(dir, &["remote", "add", "origin", url]).await;
}

struct World {
    _temp: tempfile::TempDir,
    home: Home,
    app: std::path::PathBuf,
}

fn world() -> World {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let home = root.join("home");
    let app = home.join("p/app");
    std::fs::create_dir_all(&app).unwrap();
    World { _temp: temp, home: Home::new(&home).unwrap(), app }
}

fn isolated() -> Git {
    Git::new(TestClock::new().shared()).isolated()
}

#[tokio::test]
async fn project_npmrc_token_denies_registry_for_call() {
    let world = world();
    std::fs::write(world.app.join(".npmrc"), "//registry.test/:_authToken=x\n").unwrap();
    std::fs::create_dir_all(world.app.join(".cargo")).unwrap();
    std::fs::write(world.app.join(".cargo/config.toml"), "[http]\ncainfo = \"ca.pem\"\n").unwrap();
    let dirs = [world.app.clone()];
    let found = denied(&isolated(), &world.home, &dirs).await;
    assert!(found.hosts.contains(&"registry.test".to_owned()), "{found:?}");
    for host in Preset::Npm.hosts() {
        assert!(found.hosts.contains(&(*host).to_owned()), "the default registry: {found:?}");
    }
    assert!(!found.hosts.iter().any(|host| Preset::Crates.hosts().contains(&host.as_str())));
    let [note] = found.notes.as_slice() else { panic!("{found:?}") };
    assert!(note.contains(".npmrc") && note.contains("registry.test"), "{note}");
}

#[tokio::test]
async fn remote_with_token_denies_host_for_call() {
    let world = world();
    repo(&world.app, "https://u:tok@git.test/r.git").await;
    let dirs = [world.app.clone()];
    let found = denied(&isolated(), &world.home, &dirs).await;
    assert_eq!(found.hosts, ["git.test"]);
    assert!(found.notes[0].contains("the git config of"), "{:?}", found.notes);
}

#[tokio::test]
async fn user_gitconfig_insteadof_token_denies_host() {
    let world = world();
    repo(&world.app, "https://git.test/r.git").await;
    std::fs::write(
        world.home.path().join(".gitconfig"),
        "[url \"https://u:tok@git.test/\"]\n\tinsteadOf = https://git.test/\n",
    )
    .unwrap();
    let dirs = [world.app.clone()];
    // NOTE: with the user's configuration, as efrd runs git: the home is the test's.
    let user = Git::new(TestClock::new().shared());
    let found = denied(&user, &world.home, &dirs).await;
    assert_eq!(found.hosts, ["git.test"], "the global file counts");
    let none = denied(&isolated(), &world.home, &dirs).await;
    assert!(none.hosts.is_empty(), "{none:?}");
}

#[tokio::test]
async fn unscoped_extraheader_denies_github_read() {
    let world = world();
    repo(&world.app, "https://git.test/r.git").await;
    git(&world.app, &["config", "http.extraHeader", "Authorization: Bearer x"]).await;
    let dirs = [world.app.clone()];
    let found = denied(&isolated(), &world.home, &dirs).await;
    assert!(found.hosts.contains(&"git.test".to_owned()), "every remote: {found:?}");
    for host in Preset::GithubRead.hosts() {
        assert!(found.hosts.contains(&(*host).to_owned()), "@github-read: {found:?}");
    }
}

#[tokio::test]
async fn a_clean_project_denies_nothing() {
    let world = world();
    repo(&world.app, "https://git.test/r.git").await;
    std::fs::write(world.app.join(".npmrc"), "registry=https://registry.test/\n").unwrap();
    let dirs = [world.app.clone()];
    let found = denied(&isolated(), &world.home, &dirs).await;
    assert_eq!(found, super::Denied::default());
}

#[tokio::test]
async fn call_outside_any_project_denies_user_gitconfig_token_host() {
    let world = world();
    let scratch = world.home.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(
        world.home.path().join(".gitconfig"),
        "[url \"https://u:tok@git.test/\"]\n\tinsteadOf = https://git.test/\n",
    )
    .unwrap();
    // NOTE: no project root, and the call's dir is in no repository.
    let user = Git::new(TestClock::new().shared());
    let found = denied(&user, &world.home, &[scratch]).await;
    assert_eq!(found.hosts, ["git.test"], "{found:?}");
    let [note] = found.notes.as_slice() else { panic!("{found:?}") };
    assert!(note.contains("your git config"), "{note}");
}

#[tokio::test]
async fn project_at_repository_subdir_denies_repository_token_host() {
    let world = world();
    repo(&world.app, "https://u:tok@git.test/r.git").await;
    let service = world.app.join("services/api");
    std::fs::create_dir_all(&service).unwrap();
    let found = denied(&isolated(), &world.home, &[service]).await;
    assert_eq!(found.hosts, ["git.test"], "{found:?}");
    assert!(found.notes[0].contains(&world.app.display().to_string()), "{:?}", found.notes);
}

#[tokio::test]
async fn one_git_run_per_work_tree() {
    let world = world();
    repo(&world.app, "https://u:tok@git.test/r.git").await;
    let sub = world.app.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    let found = denied(&isolated(), &world.home, &[world.app.clone(), sub]).await;
    assert_eq!(found.hosts, ["git.test"]);
    assert_eq!(found.notes.len(), 1, "{:?}", found.notes);
}
