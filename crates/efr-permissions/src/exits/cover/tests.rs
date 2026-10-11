//! The asks of the destructive forms, below the engine. The routes themselves are tables in `engine/tests/auto.rs` and
//! `engine/tests/destructive.rs`.

use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{cleans_ignored, program_asks};
use crate::{CoverAsk, Locations};

const HOME: &str = "/home/u";
const APP: &str = "/home/u/p/app";

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

fn asks(line: &str, assigned: bool) -> Vec<CoverAsk> {
    let locations = Locations::new(HOME).unwrap();
    let dir = Path::new(APP);
    let at = |word: &str| super::super::envelope::resolve(word, Some(dir), &locations);
    program_asks(&words(line), assigned, Some(dir), &locations, &at)
}

fn app() -> PathBuf {
    PathBuf::from(APP)
}

#[rstest]
#[case::reset("git reset --hard HEAD~5", vec![CoverAsk::Tracked(app()), CoverAsk::Refs(app())])]
#[case::checkout("git checkout -- .", vec![CoverAsk::Tracked(app()), CoverAsk::Refs(app())])]
#[case::switch(
    "git switch --discard-changes main",
    vec![CoverAsk::Tracked(app()), CoverAsk::Refs(app())]
)]
#[case::restore("git restore src/a.rs", vec![CoverAsk::Tracked(app())])]
#[case::clean("git clean -fd", vec![CoverAsk::Untracked(app())])]
#[case::branch("git branch -D old", vec![CoverAsk::Refs(app())])]
#[case::stash_drop("git stash drop", vec![CoverAsk::Stash(app())])]
#[case::stash_clear("git stash clear", vec![CoverAsk::Stash(app())])]
#[case::other_dir(
    "git -C sub reset --hard",
    vec![CoverAsk::Tracked(app().join("sub")), CoverAsk::Refs(app().join("sub"))]
)]
#[case::dd("dd if=/dev/zero of=db.sqlite", vec![CoverAsk::Path(app().join("db.sqlite"))])]
#[case::truncate("truncate -s 0 a.log b.log", vec![
    CoverAsk::Path(app().join("a.log")),
    CoverAsk::Path(app().join("b.log")),
])]
#[case::find("find build -name x -delete", vec![CoverAsk::Path(app().join("build"))])]
fn destructive_forms_ask_what_undo_needs(#[case] line: &str, #[case] expected: Vec<CoverAsk>) {
    assert_eq!(asks(line, false), expected, "{line:?}");
}

#[rstest]
#[case::shred("shred -u secrets.txt")]
#[case::clean_x("git clean -fdx")]
#[case::clean_big_x("git clean -f -X")]
#[case::reflog("git reflog expire --expire=now --all")]
#[case::gc("git gc --prune=now")]
#[case::git_dir("git --git-dir=/elsewhere/.git reset --hard")]
#[case::work_tree("git --work-tree /elsewhere reset --hard")]
#[case::config("git -c core.worktree=/elsewhere reset --hard")]
#[case::two_dirs("git -C a -C b reset --hard")]
#[case::pattern("truncate -s 0 *.log")]
fn forms_that_undo_never_brings_back_ask_nothing(#[case] line: &str) {
    assert_eq!(asks(line, false), Vec::<CoverAsk>::new(), "{line:?}");
}

#[test]
fn an_assignment_before_git_asks_nothing() {
    assert_eq!(asks("git reset --hard", true), Vec::<CoverAsk>::new());
    assert_eq!(asks("git --no-pager reset --hard", false).len(), 2);
}

#[rstest]
#[case("-x", true)]
#[case("-X", true)]
#[case("-fdx", true)]
#[case("-fd", false)]
#[case("--exclude=x", false)]
fn clean_options_that_remove_ignored_files(#[case] word: &str, #[case] ignored: bool) {
    assert_eq!(cleans_ignored(word), ignored, "{word}");
}
