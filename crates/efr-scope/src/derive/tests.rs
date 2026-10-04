use std::path::Path;

use efr_protocol::{ProjectId, Scope};
use pretty_assertions::assert_eq;

use super::{Basis, Derivation, derive};
use crate::testing::{Sandbox, git};
use crate::{Registry, Repo, ScopeError};

fn id(n: u8) -> ProjectId {
    format!("0192f0c1-7a00-7000-8000-0000000000{n:02}").parse().unwrap()
}

async fn scope_of(sandbox: &Sandbox, registry: &Registry, cwd: &Path) -> Derivation {
    derive(cwd, sandbox.home(), registry, &git()).await.unwrap()
}

fn machine(basis: Basis) -> Derivation {
    Derivation { scope: Scope::Machine, basis, repo: None }
}

#[tokio::test]
async fn home_and_root_are_machine_unless_registered() {
    let sandbox = Sandbox::new();
    // A dotfiles home: plain discovery would call all of it one repository.
    sandbox.init(sandbox.home().path()).await;
    let empty = Registry::empty();
    let home = sandbox.home().path();
    assert_eq!(scope_of(&sandbox, &empty, home).await, machine(Basis::HomeOrRoot));
    assert_eq!(scope_of(&sandbox, &empty, Path::new("/")).await, machine(Basis::HomeOrRoot));
    assert_eq!(scope_of(&sandbox, &empty, sandbox.root()).await, machine(Basis::HomeOrRoot));

    let mut registry = Registry::empty();
    registry.register(id(1), home, None).unwrap();
    assert_eq!(
        scope_of(&sandbox, &registry, home).await,
        Derivation { scope: Scope::Project(id(1)), basis: Basis::Registered, repo: None }
    );
}

#[tokio::test]
async fn below_a_dotfiles_home_is_machine() {
    let sandbox = Sandbox::new();
    sandbox.init(sandbox.home().path()).await;
    let nvim = sandbox.mkdir(&sandbox.in_home(".config/nvim"));
    assert_eq!(scope_of(&sandbox, &Registry::empty(), &nvim).await, machine(Basis::Elsewhere));
}

#[tokio::test]
async fn a_work_tree_is_a_path_scope() {
    let sandbox = Sandbox::new();
    let app = sandbox.init(&sandbox.in_home("p/app")).await;
    let src = sandbox.mkdir(&app.join("src"));
    let repo = Repo { root: app.clone(), branch: Some("main".into()) };
    assert_eq!(
        scope_of(&sandbox, &Registry::empty(), &src).await,
        Derivation { scope: Scope::Path(app.clone()), basis: Basis::WorkTree, repo: Some(repo) }
    );
}

#[tokio::test]
async fn a_work_tree_outside_home_is_a_path_scope() {
    let sandbox = Sandbox::new();
    let nixos = sandbox.init(&sandbox.root().join("etc/nixos")).await;
    let derivation = scope_of(&sandbox, &Registry::empty(), &nixos.join("modules/..")).await;
    assert_eq!(derivation.scope, Scope::Path(nixos));
    assert_eq!(derivation.basis, Basis::WorkTree);
}

#[tokio::test]
async fn a_registered_root_is_a_project_with_its_work_tree() {
    let sandbox = Sandbox::new();
    let app = sandbox.init(&sandbox.in_home("p/app")).await;
    let docs = sandbox.mkdir(&sandbox.in_home("p/docs"));
    let mut registry = Registry::empty();
    registry.register(id(1), sandbox.in_home("p"), None).unwrap();
    registry.register(id(2), &app, None).unwrap();

    let inner = scope_of(&sandbox, &registry, &sandbox.mkdir(&app.join("src"))).await;
    assert_eq!(inner.scope, Scope::Project(id(2)));
    assert_eq!(inner.basis, Basis::Registered);
    assert_eq!(inner.repo, Some(Repo { root: app.clone(), branch: Some("main".into()) }));

    let outer = scope_of(&sandbox, &registry, &docs).await;
    assert_eq!(
        outer,
        Derivation { scope: Scope::Project(id(1)), basis: Basis::Registered, repo: None }
    );
}

#[tokio::test]
async fn a_linked_directory_matches_the_registered_root() {
    let sandbox = Sandbox::new();
    let app = sandbox.mkdir(&sandbox.root().join("data/app"));
    let link = sandbox.in_home("app");
    std::os::unix::fs::symlink(&app, &link).unwrap();
    let mut registry = Registry::empty();
    registry.register(id(1), &app, None).unwrap();
    assert_eq!(scope_of(&sandbox, &registry, &link).await.scope, Scope::Project(id(1)));
}

#[tokio::test]
async fn plain_directories_are_machine() {
    let sandbox = Sandbox::new();
    let documents = sandbox.mkdir(&sandbox.in_home("Documents"));
    let var_log = sandbox.mkdir(&sandbox.root().join("var/log"));
    let gone = sandbox.in_home("gone");
    for cwd in [documents, var_log, gone] {
        assert_eq!(scope_of(&sandbox, &Registry::empty(), &cwd).await, machine(Basis::Elsewhere));
    }
}

#[tokio::test]
async fn the_scope_follows_the_shell_between_turns() {
    let sandbox = Sandbox::new();
    let app = sandbox.init(&sandbox.in_home("p/app")).await;
    let registry = Registry::empty();
    let turns = [app.as_path(), sandbox.home().path(), app.as_path()];
    let mut scopes = Vec::new();
    for cwd in turns {
        scopes.push(scope_of(&sandbox, &registry, cwd).await.scope);
    }
    assert_eq!(scopes, [Scope::Path(app.clone()), Scope::Machine, Scope::Path(app.clone())]);
}

#[tokio::test]
async fn a_relative_directory_is_an_error() {
    let sandbox = Sandbox::new();
    let error =
        derive(Path::new("p/app"), sandbox.home(), &Registry::empty(), &git()).await.unwrap_err();
    assert!(matches!(error, ScopeError::NotAbsolute { path } if path == Path::new("p/app")));
}

#[tokio::test]
async fn git_failure_is_an_error_but_home_needs_no_git() {
    let sandbox = Sandbox::new();
    let broken = git().with_program("/nonexistent/efr-test/git");
    let documents = sandbox.mkdir(&sandbox.in_home("Documents"));
    let error = derive(&documents, sandbox.home(), &Registry::empty(), &broken).await.unwrap_err();
    assert!(matches!(error, ScopeError::RunGit { .. }));
    let home =
        derive(sandbox.home().path(), sandbox.home(), &Registry::empty(), &broken).await.unwrap();
    assert_eq!(home, machine(Basis::HomeOrRoot));
}
