//! The git code of a git or gh line that runs outside the sandbox (v0.0.4, B4): a part
//! that a sandboxed command can write, or a check that efrd could not make, asks only
//! the user (DV22); git options that choose the code ask only the user (DV50); and a
//! package publish runs project code, so only the user approves it (DV25).

use std::path::PathBuf;

use efr_protocol::{ExitKind, GitCodeFact, GitCodeFacts};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::proxy::{in_auto, proxy_engine, remote, remote_facts, shell};
use crate::exits::fact_requests;
use crate::{Approver, CallFacts, ExitNeed, GitCodeAsk};

const APP: &str = "/home/u/p/app";
const SCRATCH: &str = "/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d";

/// The facts of a push from the project, whose `origin` is github.com, with `code`.
fn facts(code: Option<GitCodeFacts>) -> CallFacts {
    let mut facts = remote_facts(vec![remote("origin", &["github.com"], &["github.com"])]);
    facts.git_code = code;
    facts
}

/// A checked fact with `items`, each `(item, file, writable)` of the repository `repo`.
fn checked(repo: &str, items: &[(&str, &str, bool)]) -> GitCodeFacts {
    GitCodeFacts {
        checked: true,
        items: items
            .iter()
            .map(|(item, file, writable)| GitCodeFact {
                repo: repo.into(),
                item: (*item).to_owned(),
                file: (*file).into(),
                writable: *writable,
            })
            .collect(),
    }
}

/// The exits of `line` that run outside the sandbox (`outside` and `upload`).
fn leaving(line: &str, facts: CallFacts) -> Vec<ExitNeed> {
    let decision = proxy_engine().decide(&in_auto(shell(line).with_facts(facts)));
    let found: Vec<ExitNeed> = decision
        .exits()
        .filter(|need| matches!(need.kind, ExitKind::Outside | ExitKind::Upload))
        .cloned()
        .collect();
    assert!(!found.is_empty(), "{line:?}: no exit runs outside: {decision:?}");
    found
}

/// The one exit of `line` that runs outside the sandbox.
fn one(line: &str, facts: CallFacts) -> ExitNeed {
    let [need] = leaving(line, facts).try_into().unwrap();
    need
}

#[test]
fn git_code_writable_hook_makes_upload_user_only() {
    let hook = format!("{APP}/vendor/x/.git/hooks/pre-push");
    let code = checked(&format!("{APP}/vendor/x"), &[("hooks/pre-push", &hook, true)]);
    let need = one("git push origin main", facts(Some(code)));
    assert_eq!(need.kind, ExitKind::Upload);
    assert_eq!(need.approver, Approver::User);
    assert_eq!(
        need.notes,
        ["git runs ~/p/app/vendor/x/.git/hooks/pre-push, which a sandboxed command can write"]
    );
    // The same push with a clean check goes to the reviewer when a message names it.
    let need = one("git push origin main", facts(Some(checked(APP, &[]))));
    assert_eq!(need.approver, Approver::ClassifierWhenNamed);
    assert_eq!(need.notes, Vec::<String>::new());
}

#[test]
fn git_code_not_checked_makes_upload_user_only() {
    let late = GitCodeFacts { checked: false, items: Vec::new() };
    for code in [Some(late), None] {
        let need = one("git push origin main", facts(code.clone()));
        assert_eq!(need.approver, Approver::User, "{code:?}");
        assert_eq!(need.notes, ["efr could not check the git code of ~/p/app"], "{code:?}");
    }
    // A repository that efr cannot tell, such as one after a `cd`, is not checked either.
    let need = one("cd sub && git push origin main", facts(Some(checked(APP, &[]))));
    assert_eq!(need.approver, Approver::User);
    assert_eq!(need.notes, ["efr could not check the git code of the line's repository"]);
}

#[test]
fn git_code_user_global_ssh_command_keeps_reviewer() {
    // Your own `~/.gitconfig` lies where no contained call writes.
    let code = checked(APP, &[("core.sshCommand", "/home/u/.gitconfig", false)]);
    let need = one("git push origin main", facts(Some(code)));
    assert_eq!(need.approver, Approver::ClassifierWhenNamed);
    assert_eq!(need.notes, Vec::<String>::new());
}

#[test]
fn git_code_pinned_project_hooks_path_keeps_reviewer() {
    // The hooks path that the pinned config of a registered project names is a floor
    // of every call that may write the project, so efrd says it is not writable.
    let hooks = format!("{APP}/.githooks/pre-push");
    let code = checked(APP, &[("hooks/pre-push", &hooks, false)]);
    let need = one("git push origin main", facts(Some(code)));
    assert_eq!(need.approver, Approver::ClassifierWhenNamed);
}

#[test]
fn git_code_ext_transport_in_scratch_clone_is_user_only() {
    let repo = format!("{SCRATCH}/r");
    let config = format!("{repo}/.git/config");
    let code = checked(&repo, &[("remote.origin.url", &config, true)]);
    let need = one("git push origin main", facts(Some(code)));
    assert_eq!(need.approver, Approver::User);
    assert_eq!(
        need.notes,
        ["git reads remote.origin.url from \
             ~/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d/r/.git/config, which a \
             sandboxed command can write"]
    );
}

#[test]
fn git_code_ssh_from_writable_path_is_user_only() {
    let ssh = format!("{APP}/node_modules/.bin/ssh");
    let code = checked(APP, &[("ssh (PATH)", &ssh, true)]);
    let mut facts = remote_facts(vec![remote("origin", &["git@github.com:o/r.git"], &[])]);
    facts.git_code = Some(code);
    let need = one("git fetch", facts);
    assert_eq!(need.kind, ExitKind::Outside);
    assert_eq!(need.approver, Approver::User);
    assert_eq!(
        need.notes,
        ["git runs ~/p/app/node_modules/.bin/ssh, which a sandboxed command can write"]
    );
}

#[test]
fn git_code_notes_stop_at_three_and_count_the_rest() {
    let items: Vec<(String, String)> =
        (0..5).map(|n| (format!("hooks/h{n}"), format!("{SCRATCH}/r/.git/hooks/h{n}"))).collect();
    let items: Vec<(&str, &str, bool)> =
        items.iter().map(|(item, file)| (item.as_str(), file.as_str(), true)).collect();
    let need = one("git push origin main", facts(Some(checked(SCRATCH, &items))));
    assert_eq!(need.notes.len(), 4, "{:?}", need.notes);
    assert_eq!(need.notes[3], "and 2 more parts of the git code");
}

#[rstest]
#[case::exec_path("git --exec-path=/home/u/p/app/bin push origin main")]
#[case::after_other_options("git --no-pager --exec-path=./bin push origin main")]
fn push_with_exec_path_is_user_only(#[case] line: &str) {
    let need = one(line, facts(Some(checked(APP, &[]))));
    assert_eq!(need.approver, Approver::User, "{line:?}");
    assert_eq!(need.notes, ["an option of the git command chooses the code that git runs"]);
}

#[rstest]
#[case::separate("git push --receive-pack ./rp origin main")]
#[case::inline("git push --receive-pack=./rp origin main")]
#[case::exec("git push --exec=./rp origin main")]
fn push_with_receive_pack_is_user_only(#[case] line: &str) {
    let need = one(line, facts(Some(checked(APP, &[]))));
    assert_eq!(need.approver, Approver::User, "{line:?}");
}

#[test]
fn fetch_with_upload_pack_is_user_only() {
    let mut facts = remote_facts(vec![remote("origin", &["git@github.com:o/r.git"], &[])]);
    facts.git_code = Some(checked(APP, &[]));
    for line in ["git fetch --upload-pack=./up origin", "git clone -u ./up git@github.com:o/r.git"]
    {
        let need = one(line, facts.clone());
        assert_eq!(need.approver, Approver::User, "{line:?}");
    }
}

#[rstest]
#[case::config("git -c core.sshCommand=x push origin")]
#[case::config_env("git --config-env=core.sshCommand=X push origin")]
#[case::clone_config("git clone -c core.sshCommand=x git@github.com:o/r.git")]
fn push_with_dash_c_is_user_only(#[case] line: &str) {
    for need in leaving(line, facts(Some(checked(APP, &[])))) {
        assert_eq!(need.approver, Approver::User, "{line:?}");
    }
}

#[test]
fn push_with_git_dir_checks_that_repo() {
    let locations = super::auto::auto_locations();
    let line = "git --git-dir=../other/.git --work-tree ../other -C sub push origin";
    let request = fact_requests(line, Some(APP.as_ref()), &locations);
    assert_eq!(
        request.git_code,
        [GitCodeAsk {
            dir: PathBuf::from(format!("{APP}/sub")),
            git_dir: Some(PathBuf::from(format!("{APP}/other/.git"))),
            work_tree: Some(PathBuf::from(format!("{APP}/other"))),
            recurse: true,
        }]
    );
    // Each `-C` is taken from the one before it.
    let request = fact_requests("git -C ~/p -C lib fetch", Some(APP.as_ref()), &locations);
    assert_eq!(request.git_code[0].dir, PathBuf::from("/home/u/p/lib"));
    // A line that only reads asks nothing; `ls-remote` checks with no submodules.
    for line in ["git status", "git log origin/main", "git clone https://x.test/r"] {
        assert!(fact_requests(line, Some(APP.as_ref()), &locations).git_code.is_empty(), "{line}");
    }
    let request = fact_requests("git ls-remote origin", Some(APP.as_ref()), &locations);
    assert!(!request.git_code[0].recurse);
    // `--git-dir` also chooses the code, so the push is for the user whatever the facts.
    let need = one(line, facts(Some(checked(APP, &[]))));
    assert_eq!(need.approver, Approver::User);
}

#[rstest]
#[case::pr_create("gh pr create --fill", true)]
#[case::pr_merge("gh pr merge 12 --squash", true)]
#[case::pr_checkout("gh pr checkout 12", true)]
#[case::repo_sync("gh repo sync", true)]
#[case::repo_create_push("gh repo create x --source . --push", true)]
#[case::repo_fork_clone("gh repo fork o/r --clone", true)]
#[case::with_repo("gh -R o/r pr create --fill", true)]
#[case::repo_create("gh repo create x", false)]
#[case::pr_view("gh pr view 12", false)]
#[case::issue("gh issue create", false)]
fn gh_pr_create_triggers_the_check(#[case] line: &str, #[case] asks: bool) {
    let locations = super::auto::auto_locations();
    let request = fact_requests(line, Some(APP.as_ref()), &locations);
    assert_eq!(!request.git_code.is_empty(), asks, "{line:?}");
    if asks {
        assert_eq!(request.git_code[0].dir, PathBuf::from(APP), "{line:?}");
    }
}

#[test]
fn gh_pr_create_with_a_writable_hook_is_user_only() {
    let hook = format!("{SCRATCH}/r/.git/hooks/pre-push");
    let code = checked(&format!("{SCRATCH}/r"), &[("hooks/pre-push", &hook, true)]);
    let need =
        one("gh pr create --fill", CallFacts { git_code: Some(code), ..CallFacts::default() });
    assert_eq!(need.approver, Approver::User);
    let need = one(
        "gh pr create --fill",
        CallFacts { git_code: Some(checked(APP, &[])), ..CallFacts::default() },
    );
    assert_eq!(need.approver, Approver::ClassifierWhenNamed);
}

#[rstest]
#[case::npm("npm publish")]
#[case::pnpm("pnpm publish --access public")]
#[case::yarn("yarn npm publish")]
#[case::bun("bun publish")]
fn npm_publish_user_only(#[case] line: &str) {
    let need = one(line, CallFacts::default());
    assert_eq!(need.kind, ExitKind::Upload, "{line:?}");
    assert_eq!(need.approver, Approver::User, "{line:?}");
    assert_eq!(need.names, ["publish"], "{line:?}");
    assert_eq!(need.notes, ["runs the project's prepublishOnly and prepack scripts"], "{line:?}");
}

#[test]
fn cargo_publish_user_only() {
    let need = one("cargo publish -p efr-cli", CallFacts::default());
    assert_eq!(need.approver, Approver::User);
    assert_eq!(
        need.notes,
        [
            "builds the crate (build.rs) with your rights",
            "reads .cargo/config.toml, which can name a credential program",
        ]
    );
}

#[test]
fn cargo_publish_no_verify_user_only() {
    let need = one("cargo publish --no-verify", CallFacts::default());
    assert_eq!(need.approver, Approver::User);
    assert_eq!(need.notes, ["reads .cargo/config.toml, which can name a credential program"]);
}

#[rstest]
#[case::yank("cargo yank --version 1.0.0 efx", "yank")]
#[case::owner("cargo owner --add someone efx", "owner")]
fn cargo_yank_user_only(#[case] line: &str, #[case] verb: &str) {
    let need = one(line, CallFacts::default());
    assert_eq!(need.approver, Approver::User, "{line:?}");
    assert_eq!(need.names, [verb], "{line:?}");
    assert_eq!(need.notes, ["reads .cargo/config.toml, which can name a credential program"]);
}

#[test]
fn a_contained_fetch_needs_no_git_code() {
    // An https fetch runs in the sandbox through the proxy: its hooks never run outside.
    let facts = facts(None);
    let decision = proxy_engine().decide(&in_auto(shell("git fetch origin").with_facts(facts)));
    assert!(
        decision.exits().all(|need| !matches!(need.kind, ExitKind::Outside | ExitKind::Upload)),
        "{decision:?}"
    );
}
