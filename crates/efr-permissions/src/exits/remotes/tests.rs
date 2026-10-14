//! Which remote a git command uses, and when the facts of its directory hold.

use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{RemoteCommand, RemoteUse, remote_command, remote_dir, selected, steers_git};
use crate::{CallFacts, RemoteFact, RemoteUrl};

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

#[rstest]
#[case::fetch("git fetch", RemoteUse::Fetch, None, false)]
#[case::fetch_named("git fetch upstream main", RemoteUse::Fetch, Some("upstream"), false)]
#[case::fetch_all("git fetch --all --prune", RemoteUse::Fetch, None, true)]
#[case::pull("git pull --rebase origin main", RemoteUse::Fetch, Some("origin"), false)]
#[case::pull_depth("git pull --depth 1", RemoteUse::Fetch, None, false)]
#[case::ls_remote("git ls-remote origin", RemoteUse::Fetch, Some("origin"), false)]
#[case::remote_update("git remote update", RemoteUse::Fetch, None, true)]
#[case::push("git push", RemoteUse::Push, None, false)]
#[case::push_named("git push -u fork HEAD", RemoteUse::Push, Some("fork"), false)]
#[case::submodules("git submodule update --init", RemoteUse::Submodules, None, true)]
#[case::in_dir("git -C ../lib fetch", RemoteUse::Fetch, None, false)]
fn remote_commands(
    #[case] line: &str,
    #[case] kind: RemoteUse,
    #[case] named: Option<&str>,
    #[case] all: bool,
) {
    let words = words(line);
    assert_eq!(remote_command(&words), Some(RemoteCommand { kind, named, all }), "{line:?}");
}

#[rstest]
#[case::status("git status")]
#[case::commit("git commit -m x")]
#[case::url("git fetch https://x.test/r.git")]
#[case::scp_form("git pull git@x.test:r.git")]
#[case::path("git fetch ../other")]
#[case::config("git -c remote.origin.url=https://evil.test/r fetch")]
#[case::git_dir("git --git-dir=/tmp/x/.git fetch")]
#[case::work_tree("git --work-tree /tmp/x fetch")]
#[case::shell_word("git fetch $REMOTE")]
#[case::remote_add("git remote add x https://x.test/r")]
#[case::not_git("hg pull")]
fn remote_commands_that_name_no_remote_or_steer_the_config(#[case] line: &str) {
    assert_eq!(remote_command(&words(line)), None, "{line:?}");
}

#[test]
fn remote_dir_reads_dash_c() {
    let resolve = |word: &str| Some(Path::new("/p").join(word));
    let dir = Path::new("/p/app");
    assert_eq!(remote_dir(&words("git fetch"), Some(dir), &resolve), Some(dir.to_path_buf()));
    assert_eq!(
        remote_dir(&words("git -C lib fetch"), Some(dir), &resolve),
        Some(PathBuf::from("/p/lib"))
    );
    assert_eq!(remote_dir(&words("git fetch"), None, &resolve), None);
}

#[test]
fn git_variables_steer_the_remotes() {
    assert!(steers_git(&words("GIT_DIR=/x/.git git fetch")));
    assert!(steers_git(&words("env GIT_CONFIG_COUNT=1 git push")));
    assert!(!steers_git(&words("LC_ALL=C git fetch")));
}

fn url(host: &str, ssh: bool) -> RemoteUrl {
    RemoteUrl { host: host.to_owned(), port: if ssh { 22 } else { 443 }, ssh, credentials: false }
}

fn remote(name: &str, host: &str, upstream: bool) -> RemoteFact {
    RemoteFact {
        name: name.to_owned(),
        fetch: vec![url(host, false)],
        push: vec![url(host, false)],
        project: false,
        upstream,
        submodule: false,
    }
}

fn facts(remotes: Vec<RemoteFact>) -> CallFacts {
    CallFacts { remotes: vec![(PathBuf::from("/p/app"), remotes)], ..CallFacts::default() }
}

#[test]
fn selected_takes_the_named_then_the_upstream_then_origin() {
    let dir = Path::new("/p/app");
    let names = |line: &str, facts: &CallFacts| -> Option<Vec<String>> {
        let words = words(line);
        let command = remote_command(&words).unwrap();
        selected(&command, dir, Some(facts))
            .map(|chosen| chosen.iter().map(|remote| remote.name.clone()).collect())
    };
    let two = facts(vec![remote("origin", "a.test", false), remote("fork", "b.test", true)]);
    assert_eq!(names("git fetch origin", &two), Some(vec!["origin".to_owned()]));
    assert_eq!(names("git pull", &two), Some(vec!["fork".to_owned()]));
    assert_eq!(names("git fetch --all", &two), Some(vec!["origin".to_owned(), "fork".to_owned()]));
    let plain = facts(vec![remote("origin", "a.test", false), remote("fork", "b.test", false)]);
    assert_eq!(names("git push", &plain), Some(vec!["origin".to_owned()]));
    // A remote that the facts do not hold, and a directory with no fact, give none.
    assert_eq!(names("git fetch nope", &plain), None);
    let words = words("git fetch");
    let command = remote_command(&words).unwrap();
    assert_eq!(selected(&command, Path::new("/elsewhere"), Some(&plain)), None);
    assert_eq!(selected(&command, dir, None), None);
}
