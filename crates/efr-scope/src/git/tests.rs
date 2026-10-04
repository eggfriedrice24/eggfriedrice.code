use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;

use pretty_assertions::assert_eq;

use super::{DEFAULT_GIT_TIMEOUT, Discovery, Git, Repo};
use crate::testing::{InstantClock, Sandbox, StoppedClock, git};
use crate::{Home, ScopeError};

#[tokio::test]
async fn a_repository_is_found_from_inside() {
    let sandbox = Sandbox::new();
    let app = sandbox.init(&sandbox.in_home("p/app")).await;
    let src = sandbox.mkdir(&app.join("src/bin"));
    let expected = Discovery::WorkTree(Repo { root: app.clone(), branch: Some("main".into()) });
    assert_eq!(git().discover(&app, sandbox.home()).await.unwrap(), expected);
    assert_eq!(git().discover(&src, sandbox.home()).await.unwrap(), expected);
}

#[tokio::test]
async fn a_repository_outside_home_is_found() {
    let sandbox = Sandbox::new();
    let site = sandbox.init(&sandbox.root().join("srv/site")).await;
    let discovery = git().discover(&site.join("."), sandbox.home()).await.unwrap();
    assert_eq!(discovery.work_tree().map(|repo| repo.root.as_path()), Some(site.as_path()));
}

#[tokio::test]
async fn a_dotfiles_home_is_not_found_from_below() {
    let sandbox = Sandbox::new();
    sandbox.init(sandbox.home().path()).await;
    let nvim = sandbox.mkdir(&sandbox.in_home(".config/nvim"));
    assert_eq!(git().discover(&nvim, sandbox.home()).await.unwrap(), Discovery::NotARepository);
}

#[tokio::test]
async fn a_dotfiles_home_is_guarded_from_home_itself() {
    let sandbox = Sandbox::new();
    sandbox.init(sandbox.home().path()).await;
    assert_eq!(
        git().discover(sandbox.home().path(), sandbox.home()).await.unwrap(),
        Discovery::Guarded { root: sandbox.home().path().to_path_buf() }
    );
}

#[tokio::test]
async fn a_repository_above_home_is_guarded_and_not_climbed_into() {
    let sandbox = Sandbox::new();
    // The sandbox root holds the home directory, like a repository at /home would.
    sandbox.init(sandbox.root()).await;
    assert_eq!(
        git().discover(sandbox.root(), sandbox.home()).await.unwrap(),
        Discovery::Guarded { root: sandbox.root().to_path_buf() }
    );
    let notes = sandbox.mkdir(&sandbox.in_home("notes"));
    assert_eq!(git().discover(&notes, sandbox.home()).await.unwrap(), Discovery::NotARepository);
}

#[tokio::test]
async fn a_repository_inside_a_dotfiles_home_is_its_own_work_tree() {
    let sandbox = Sandbox::new();
    sandbox.init(sandbox.home().path()).await;
    let app = sandbox.init(&sandbox.in_home("p/app")).await;
    let discovery = git().discover(&app, sandbox.home()).await.unwrap();
    assert_eq!(discovery.work_tree().map(|repo| repo.root.as_path()), Some(app.as_path()));
}

#[tokio::test]
async fn a_detached_head_has_no_branch() {
    let sandbox = Sandbox::new();
    let app = sandbox.init(&sandbox.in_home("app")).await;
    sandbox.git(&app, &["commit", "--quiet", "--allow-empty", "-m", "first"]).await;
    sandbox.git(&app, &["checkout", "--quiet", "--detach"]).await;
    assert_eq!(
        git().discover(&app, sandbox.home()).await.unwrap(),
        Discovery::WorkTree(Repo { root: app.clone(), branch: None })
    );
}

#[tokio::test]
async fn a_linked_directory_reports_the_resolved_root() {
    let sandbox = Sandbox::new();
    let app = sandbox.init(&sandbox.in_home("p/app")).await;
    let link = sandbox.in_home("app-link");
    std::os::unix::fs::symlink(&app, &link).unwrap();
    let discovery = git().discover(&link, sandbox.home()).await.unwrap();
    assert_eq!(discovery.work_tree().map(|repo| repo.root.as_path()), Some(app.as_path()));
}

#[tokio::test]
async fn plain_and_missing_directories_are_no_repository() {
    let sandbox = Sandbox::new();
    let plain = sandbox.mkdir(&sandbox.in_home("Documents"));
    assert_eq!(git().discover(&plain, sandbox.home()).await.unwrap(), Discovery::NotARepository);
    let missing = sandbox.in_home("gone");
    assert_eq!(git().discover(&missing, sandbox.home()).await.unwrap(), Discovery::NotARepository);
}

#[tokio::test]
async fn inside_a_git_directory_is_no_work_tree() {
    let sandbox = Sandbox::new();
    let app = sandbox.init(&sandbox.in_home("app")).await;
    assert_eq!(
        git().discover(&app.join(".git"), sandbox.home()).await.unwrap(),
        Discovery::NotARepository
    );
}

#[tokio::test]
async fn a_relative_directory_is_an_error() {
    let sandbox = Sandbox::new();
    let error = git().discover(Path::new("p/app"), sandbox.home()).await.unwrap_err();
    assert!(matches!(error, ScopeError::NotAbsolute { path } if path == Path::new("p/app")));
}

#[tokio::test]
async fn git_that_cannot_start_is_an_error() {
    let sandbox = Sandbox::new();
    let git = git().with_program("/nonexistent/efr-test/git");
    let error = git.discover(sandbox.home().path(), sandbox.home()).await.unwrap_err();
    assert!(
        matches!(error, ScopeError::RunGit { program, .. } if program == "/nonexistent/efr-test/git")
    );
}

#[tokio::test]
async fn git_that_hangs_times_out_without_waiting() {
    let sandbox = Sandbox::new();
    let script = sandbox.root().join("hanging-git");
    std::fs::write(&script, "#!/bin/sh\nexec sleep 600\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cwd = sandbox.mkdir(&sandbox.in_home("p"));
    let git = Git::new(Arc::new(InstantClock)).isolated().with_program(&script);
    let error = git.discover(&cwd, sandbox.home()).await.unwrap_err();
    assert!(matches!(error, ScopeError::GitTimedOut { after } if after == DEFAULT_GIT_TIMEOUT));
}

#[test]
fn the_command_runs_in_the_directory_with_a_guarded_environment() {
    let home = Home::new("/home/u").unwrap();
    let git = Git::new(Arc::new(StoppedClock));
    let command = git.command(Path::new("/home/u/p"), &home, ["status"]);
    let command = command.as_std();
    assert_eq!(command.get_program(), "git");
    assert_eq!(command.get_current_dir(), Some(Path::new("/home/u/p")));
    let env = |name: &str| {
        command
            .get_envs()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.map(OsStr::to_owned))
    };
    assert_eq!(env("GIT_CEILING_DIRECTORIES"), Some(Some("/home/u:/".into())));
    assert_eq!(env("HOME"), Some(Some("/home/u".into())));
    assert_eq!(env("GIT_OPTIONAL_LOCKS"), Some(Some("0".into())));
    assert_eq!(env("GIT_TERMINAL_PROMPT"), Some(Some("0".into())));
    for name in
        ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT"]
    {
        assert_eq!(env(name), Some(None), "{name} is removed");
    }
    assert_eq!(env("GIT_CONFIG_GLOBAL"), None);
}

#[test]
fn an_isolated_command_ignores_the_user_configuration() {
    let home = Home::new("/home/u").unwrap();
    let git = Git::new(Arc::new(StoppedClock)).isolated();
    let command = git.command(Path::new("/home/u"), &home, ["status"]);
    let envs: Vec<_> = command.as_std().get_envs().collect();
    assert!(envs.contains(&(OsStr::new("GIT_CONFIG_NOSYSTEM"), Some(OsStr::new("1")))));
    assert!(envs.contains(&(OsStr::new("GIT_CONFIG_GLOBAL"), Some(OsStr::new("/dev/null")))));
    assert!(envs.contains(&(OsStr::new("XDG_CONFIG_HOME"), Some(OsStr::new("/home/u/.config")))));
}

#[test]
fn ceilings_list_both_forms_of_home_and_skip_unlistable_ones() {
    let home = Home::linked("/home/u", "/var/home/u");
    assert_eq!(super::ceilings(&home), "/home/u:/var/home/u:/");
    let colon = Home::linked("/home/a:b", "/home/a:b");
    assert_eq!(super::ceilings(&colon), "/");
}
