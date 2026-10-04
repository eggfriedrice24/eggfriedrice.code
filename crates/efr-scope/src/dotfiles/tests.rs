use pretty_assertions::assert_eq;

use super::{Dotfiles, detect_dotfiles};
use crate::ScopeError;
use crate::testing::{Sandbox, git};

async fn detect(sandbox: &Sandbox) -> Vec<Dotfiles> {
    detect_dotfiles(sandbox.home(), None, &git()).await.unwrap()
}

#[tokio::test]
async fn a_plain_home_has_no_dotfiles() {
    let sandbox = Sandbox::new();
    sandbox.mkdir(&sandbox.in_home(".config/nvim"));
    std::fs::write(sandbox.in_home(".zshrc"), "").unwrap();
    assert_eq!(detect(&sandbox).await, []);
}

#[tokio::test]
async fn a_home_work_tree_is_found() {
    let sandbox = Sandbox::new();
    sandbox.init(sandbox.home().path()).await;
    let found = detect(&sandbox).await;
    assert_eq!(found, [Dotfiles::HomeWorkTree { git_dir: sandbox.in_home(".git") }]);
    assert_eq!(found[0].git_dir(), sandbox.in_home(".git"));
}

#[tokio::test]
async fn a_broken_dot_git_is_no_work_tree() {
    let sandbox = Sandbox::new();
    sandbox.mkdir(&sandbox.in_home(".git"));
    assert_eq!(detect(&sandbox).await, []);
}

#[tokio::test]
async fn yadm_is_found_in_its_three_places() {
    for place in [".local/share/yadm/repo.git", ".config/yadm/repo.git", ".yadm/repo.git"] {
        let sandbox = Sandbox::new();
        let home = sandbox.home().path().to_str().unwrap().to_owned();
        let repo = sandbox.init_bare(&sandbox.in_home(place), Some(&home)).await;
        assert_eq!(detect(&sandbox).await, [Dotfiles::Yadm { git_dir: repo }], "{place}");
    }
}

#[tokio::test]
async fn yadm_follows_the_data_home_it_is_given() {
    let sandbox = Sandbox::new();
    let data_home = sandbox.root().join("data");
    let repo = sandbox.init_bare(&data_home.join("yadm/repo.git"), None).await;
    let found = detect_dotfiles(sandbox.home(), Some(&data_home), &git()).await.unwrap();
    assert_eq!(found, [Dotfiles::Yadm { git_dir: repo }]);
    assert_eq!(detect(&sandbox).await, []);
}

#[tokio::test]
async fn a_bare_repository_for_home_is_found() {
    let sandbox = Sandbox::new();
    let home = sandbox.home().path().to_str().unwrap().to_owned();
    let dotfiles = sandbox.init_bare(&sandbox.in_home(".dotfiles"), Some(&home)).await;
    assert_eq!(detect(&sandbox).await, [Dotfiles::BareRepo { git_dir: dotfiles }]);
}

#[tokio::test]
async fn a_relative_or_tilde_worktree_counts_when_it_names_home() {
    let sandbox = Sandbox::new();
    let cfg = sandbox.init_bare(&sandbox.in_home(".cfg"), Some("..")).await;
    let dots = sandbox.init_bare(&sandbox.in_home(".dots"), Some("~")).await;
    assert_eq!(
        detect(&sandbox).await,
        [Dotfiles::BareRepo { git_dir: cfg }, Dotfiles::BareRepo { git_dir: dots }]
    );
}

#[tokio::test]
async fn bare_repositories_for_other_trees_are_not_dotfiles() {
    let sandbox = Sandbox::new();
    let elsewhere = sandbox.root().join("elsewhere").to_str().unwrap().to_owned();
    sandbox.init_bare(&sandbox.in_home(".mirror"), Some(&elsewhere)).await;
    sandbox.init_bare(&sandbox.in_home(".backup"), None).await;
    sandbox.init_bare(&sandbox.in_home("visible.git"), Some("..")).await;
    assert_eq!(detect(&sandbox).await, []);
}

#[tokio::test]
async fn every_layout_is_reported_in_order() {
    let sandbox = Sandbox::new();
    let home = sandbox.home().path().to_str().unwrap().to_owned();
    sandbox.init(sandbox.home().path()).await;
    let yadm = sandbox.init_bare(&sandbox.in_home(".local/share/yadm/repo.git"), Some(&home)).await;
    let zeta = sandbox.init_bare(&sandbox.in_home(".zeta"), Some(&home)).await;
    let alpha = sandbox.init_bare(&sandbox.in_home(".alpha"), Some(&home)).await;
    assert_eq!(
        detect(&sandbox).await,
        [
            Dotfiles::HomeWorkTree { git_dir: sandbox.in_home(".git") },
            Dotfiles::Yadm { git_dir: yadm },
            Dotfiles::BareRepo { git_dir: alpha },
            Dotfiles::BareRepo { git_dir: zeta },
        ]
    );
}

#[tokio::test]
async fn a_missing_home_is_an_error() {
    let sandbox = Sandbox::new();
    let home = crate::Home::new(sandbox.root().join("gone")).unwrap();
    let error = detect_dotfiles(&home, None, &git()).await.unwrap_err();
    assert!(
        matches!(error, ScopeError::ReadDir { path, .. } if path == sandbox.root().join("gone"))
    );
}
