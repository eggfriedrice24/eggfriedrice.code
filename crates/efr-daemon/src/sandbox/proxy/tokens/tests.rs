//! The hosts that a call may not reach because a file that the sandbox reads holds a
//! credential for them (DP25), from real files and real git configs with `.test` hosts.

use std::collections::BTreeMap;
use std::path::Path;

use efr_permissions::Preset;
use efr_scope::{Git, Home};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;

use super::{HelperFacts, denied};

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

/// The facts of a call that hides gh's token: the default config dir, which the
/// sandbox masks as an engine secret, no kept variable and no session bus.
fn safe(world: &World) -> HelperFacts {
    HelperFacts {
        env: Some(BTreeMap::new()),
        masks: vec![world.home.path().join(".config/gh/hosts.yml")],
        ..HelperFacts::default()
    }
}

/// A repository of `git.test` whose config has gh's helper for `https://git.test`.
async fn gh_repo(world: &World) {
    repo(&world.app, "https://git.test/r.git").await;
    git(&world.app, &["config", "credential.https://git.test.helper", ""]).await;
    let helper = "!/usr/bin/gh auth git-credential";
    git(&world.app, &["config", "--add", "credential.https://git.test.helper", helper]).await;
}

#[tokio::test]
async fn gh_helper_safe_with_default_hosts_yml() {
    let world = world();
    gh_repo(&world).await;
    let found =
        denied(&isolated(), &world.home, std::slice::from_ref(&world.app), &safe(&world)).await;
    assert_eq!(found, super::Denied::default());
    let (xdg, gh) = (world.home.path().join("xdg"), world.home.path().join("gh"));
    let env =
        |name: &str, dir: &Path| BTreeMap::from([(name.to_owned(), dir.display().to_string())]);
    for (env, mask) in
        [(env("XDG_CONFIG_HOME", &xdg), xdg.join("gh")), (env("GH_CONFIG_DIR", &gh), gh.clone())]
    {
        let facts = HelperFacts { env: Some(env), masks: vec![mask], ..safe(&world) };
        assert_eq!(facts.gh_open(world.home.path()), None, "{facts:?}");
    }
}

#[tokio::test]
async fn gh_helper_denies_with_gh_config_dir_outside_masks() {
    let world = world();
    gh_repo(&world).await;
    let dir = world.home.path().join("dotfiles/gh");
    let facts = HelperFacts {
        env: Some(BTreeMap::from([("GH_CONFIG_DIR".to_owned(), dir.display().to_string())])),
        ..safe(&world)
    };
    let found = denied(&isolated(), &world.home, std::slice::from_ref(&world.app), &facts).await;
    assert_eq!(found.hosts, ["git.test"], "only the scope's host: {found:?}");
    let [note] = found.notes.as_slice() else { panic!("{found:?}") };
    assert!(note.contains(&dir.join("hosts.yml").display().to_string()), "{note}");
    assert!(note.contains("which the sandbox does not hide"), "{note}");
    // A relative dir and an unknown environment deny too.
    let relative = HelperFacts {
        env: Some(BTreeMap::from([("XDG_CONFIG_HOME".to_owned(), "cfg".to_owned())])),
        ..safe(&world)
    };
    assert!(relative.gh_open(world.home.path()).is_some_and(|why| why.contains("XDG_CONFIG_HOME")));
    let unknown = HelperFacts { env: None, ..safe(&world) };
    assert!(unknown.gh_open(world.home.path()).is_some());
}

#[tokio::test]
async fn gh_helper_denies_with_env_keep_gh_token() {
    let world = world();
    gh_repo(&world).await;
    let facts = HelperFacts { keep: vec!["GH_TOKEN".to_owned()], ..safe(&world) };
    let found = denied(&isolated(), &world.home, std::slice::from_ref(&world.app), &facts).await;
    assert_eq!(found.hosts, ["git.test"]);
    assert!(found.notes[0].contains("sandbox.env_keep keeps GH_TOKEN"), "{found:?}");
    // `env_deny` wins over `env_keep`, so the variable never reaches the call.
    let denied_too = HelperFacts { deny: vec!["GH_TOKEN".to_owned()], ..facts };
    assert_eq!(denied_too.gh_open(world.home.path()), None);
}

#[tokio::test]
async fn gh_helper_denies_with_env_keep_pattern() {
    let world = world();
    gh_repo(&world).await;
    for pattern in ["GH_*", "*TOKEN*", "GITHUB_*"] {
        let facts = HelperFacts { keep: vec![pattern.to_owned()], ..safe(&world) };
        assert!(facts.gh_open(world.home.path()).is_some(), "{pattern}");
    }
    let other = HelperFacts { keep: vec!["NPM_TOKEN".to_owned()], ..safe(&world) };
    assert_eq!(other.gh_open(world.home.path()), None);
}

#[tokio::test]
async fn gh_helper_denies_with_session_bus_grant() {
    let world = world();
    gh_repo(&world).await;
    let facts = HelperFacts { session_bus: true, ..safe(&world) };
    let found = denied(&isolated(), &world.home, std::slice::from_ref(&world.app), &facts).await;
    assert_eq!(found.hosts, ["git.test"]);
    assert!(found.notes[0].contains("whole session bus"), "{found:?}");
}

#[tokio::test]
async fn unknown_helper_denies_every_remote_and_names_it() {
    let world = world();
    repo(&world.app, "https://git.test/r.git").await;
    git(&world.app, &["remote", "add", "mirror", "https://mirror.test/r.git"]).await;
    git(&world.app, &["config", "credential.helper", "/usr/local/bin/pass-git-helper"]).await;
    let found =
        denied(&isolated(), &world.home, std::slice::from_ref(&world.app), &safe(&world)).await;
    assert_eq!(found.hosts, ["git.test", "mirror.test"]);
    let [note] = found.notes.as_slice() else { panic!("{found:?}") };
    assert!(note.contains("pass-git-helper"), "{note}");
    assert!(note.contains("a credential helper that efr does not know"), "{note}");
}

#[tokio::test]
async fn scoped_shell_helper_denies_its_host_only() {
    let world = world();
    repo(&world.app, "https://git.test/r.git").await;
    git(&world.app, &["config", "credential.https://other.test.helper", "!cat ~/token"]).await;
    let found =
        denied(&isolated(), &world.home, std::slice::from_ref(&world.app), &safe(&world)).await;
    assert_eq!(found.hosts, ["other.test"], "{found:?}");
    git(&world.app, &["config", "credential.helper", "!cat ~/token"]).await;
    let found =
        denied(&isolated(), &world.home, std::slice::from_ref(&world.app), &safe(&world)).await;
    assert_eq!(found.hosts, ["git.test", "other.test"], "{found:?}");
    assert_eq!(found.notes.len(), 1, "{:?}", found.notes);
}

#[test]
fn gh_env_reads_only_the_config_dir_variables() {
    let bytes = b"HOME=/home/u\0GH_CONFIG_DIR=/x/gh\0GH_TOKEN=t\0XDG_CONFIG_HOME=/x/cfg\0";
    assert_eq!(
        HelperFacts::gh_env(bytes),
        BTreeMap::from([
            ("GH_CONFIG_DIR".to_owned(), "/x/gh".to_owned()),
            ("XDG_CONFIG_HOME".to_owned(), "/x/cfg".to_owned()),
        ])
    );
}

#[tokio::test]
async fn project_npmrc_token_denies_registry_for_call() {
    let world = world();
    std::fs::write(world.app.join(".npmrc"), "//registry.test/:_authToken=x\n").unwrap();
    std::fs::create_dir_all(world.app.join(".cargo")).unwrap();
    std::fs::write(world.app.join(".cargo/config.toml"), "[http]\ncainfo = \"ca.pem\"\n").unwrap();
    let dirs = [world.app.clone()];
    let found = denied(&isolated(), &world.home, &dirs, &safe(&world)).await;
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
    let found = denied(&isolated(), &world.home, &dirs, &safe(&world)).await;
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
    let found = denied(&user, &world.home, &dirs, &safe(&world)).await;
    assert_eq!(found.hosts, ["git.test"], "the global file counts");
    let none = denied(&isolated(), &world.home, &dirs, &safe(&world)).await;
    assert!(none.hosts.is_empty(), "{none:?}");
}

#[tokio::test]
async fn unscoped_extraheader_denies_github_read() {
    let world = world();
    repo(&world.app, "https://git.test/r.git").await;
    git(&world.app, &["config", "http.extraHeader", "Authorization: Bearer x"]).await;
    let dirs = [world.app.clone()];
    let found = denied(&isolated(), &world.home, &dirs, &safe(&world)).await;
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
    let found = denied(&isolated(), &world.home, &dirs, &safe(&world)).await;
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
    let found = denied(&user, &world.home, &[scratch], &safe(&world)).await;
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
    let found = denied(&isolated(), &world.home, &[service], &safe(&world)).await;
    assert_eq!(found.hosts, ["git.test"], "{found:?}");
    assert!(found.notes[0].contains(&world.app.display().to_string()), "{:?}", found.notes);
}

#[tokio::test]
async fn one_git_run_per_work_tree() {
    let world = world();
    repo(&world.app, "https://u:tok@git.test/r.git").await;
    let sub = world.app.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    let found = denied(&isolated(), &world.home, &[world.app.clone(), sub], &safe(&world)).await;
    assert_eq!(found.hosts, ["git.test"]);
    assert_eq!(found.notes.len(), 1, "{:?}", found.notes);
}
