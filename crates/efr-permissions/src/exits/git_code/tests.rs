//! The words of a git or gh command that decide the git code check.

use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{Trigger, steers_code, trigger};
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
#[case::send_email("git send-email --to=x@y.test 0001.patch", false)]
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

#[rstest]
#[case::config("git -c a=b push", true)]
#[case::config_attached("git -ca=b push", false)]
#[case::config_env("git --config-env a=B push", true)]
#[case::exec_path("git --exec-path=/x push", true)]
#[case::git_dir("git --git-dir x push", true)]
#[case::work_tree("git --work-tree=x push", true)]
#[case::namespace("git --namespace x push", true)]
#[case::receive_pack("git push --receive-pack=x", true)]
#[case::upload_pack("git fetch --upload-pack x", true)]
#[case::exec("git push --exec x", true)]
#[case::template("git clone --template=t https://x.test/r", true)]
#[case::clone_u("git clone -u x https://x.test/r", true)]
#[case::clone_config("git clone --config a=b https://x.test/r", true)]
#[case::to_cmd("git send-email --to-cmd=./x 0001.patch", true)]
#[case::cc_cmd("git send-email --cc-cmd ./x 0001.patch", true)]
#[case::header_cmd("git send-email --header-cmd=./x 0001.patch", true)]
#[case::sendmail_cmd("git send-email --sendmail-cmd=./x 0001.patch", true)]
#[case::smtp_program("git send-email --smtp-server=./msmtp 0001.patch", true)]
#[case::smtp_program_separate("git send-email --smtp-server ./msmtp 0001.patch", true)]
#[case::smtp_host("git send-email --smtp-server=smtp.example.org 0001.patch", false)]
#[case::smtp_host_separate("git send-email --smtp-server smtp.example.org x.patch", false)]
#[case::commit_c("git commit -c HEAD", false)]
#[case::plain("git push origin main", false)]
#[case::dir("git -C x push", false)]
#[case::not_git("cargo -c x", false)]
fn options_that_choose_the_code(#[case] line: &str, #[case] steers: bool) {
    assert_eq!(steers_code(&words(line)), steers, "{line:?}");
}
