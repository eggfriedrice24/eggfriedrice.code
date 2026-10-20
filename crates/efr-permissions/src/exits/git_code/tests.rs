//! The words of a git or gh command that decide the git code check.

use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{Trigger, trigger};
use crate::{GitCodeAsk, Locations};

const APP: &str = "/home/u/p/app";

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

fn locations() -> Locations {
    Locations::new("/home/u").unwrap()
}

#[rstest]
#[case::push("git push", true)]
#[case::send_pack("git send-pack x", true)]
#[case::fetch("git fetch --all", true)]
#[case::pull("git pull --rebase", true)]
#[case::submodule_update("git submodule update --init", true)]
#[case::ls_remote("git ls-remote origin", false)]
#[case::remote_update("git remote -v update", false)]
fn git_commands_that_reach_a_remote_ask(#[case] line: &str, #[case] recurse: bool) {
    let found = trigger(&words(line), Some(APP.as_ref()), &locations());
    let ask = GitCodeAsk { dir: PathBuf::from(APP), git_dir: None, work_tree: None, recurse };
    assert_eq!(found, Some(Trigger::Ask(ask)), "{line:?}");
}

#[rstest]
#[case::status("git status")]
#[case::commit("git commit -m x")]
#[case::remote_show("git remote show origin")]
#[case::submodule_status("git submodule status")]
#[case::clone("git clone https://x.test/r")]
#[case::other("cargo build")]
fn other_commands_ask_nothing(#[case] line: &str) {
    assert_eq!(trigger(&words(line), Some(APP.as_ref()), &locations()), None, "{line:?}");
}

#[rstest]
#[case::no_dir("git push", None)]
#[case::opaque_dir("git -C \u{1} push", Some(APP))]
#[case::opaque_git_dir("git --git-dir=\u{1} push", Some(APP))]
fn a_repository_that_efr_cannot_tell_is_unknown(#[case] line: &str, #[case] dir: Option<&str>) {
    let found = trigger(&words(line), dir.map(AsRef::as_ref), &locations());
    assert_eq!(found, Some(Trigger::Unknown), "{line:?}");
}
