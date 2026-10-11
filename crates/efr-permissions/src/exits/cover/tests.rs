//! The asks of the destructive forms and the copies of a home write, below the
//! engine. The routes themselves are tables in `engine/tests/auto.rs` and
//! `engine/tests/destructive.rs`.

use std::path::{Path, PathBuf};

use efr_protocol::ExitKind;
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{cleans_ignored, program_asks, write_asks, write_copies};
use crate::exits::WriteBind;
use crate::{AutoSupport, CoverAsk, Locations, TargetKind};

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

#[test]
fn a_write_asks_for_the_file_the_dir_and_a_parent_that_a_bind_can_open() {
    let home = Path::new(HOME);
    let notes = PathBuf::from("/home/u/Documents/notes.txt");
    assert_eq!(
        write_asks(&notes, home),
        [
            CoverAsk::Home(notes.clone()),
            CoverAsk::HomeDir(notes),
            CoverAsk::HomeDir(PathBuf::from("/home/u/Documents")),
        ]
    );
    // A shared dir is never bound whole, so its walk is never needed.
    let zshrc = PathBuf::from("/home/u/.zshrc");
    assert_eq!(write_asks(&zshrc, home), [CoverAsk::Home(zshrc.clone()), CoverAsk::HomeDir(zshrc)]);
    let conf = PathBuf::from("/home/u/.config/app.conf");
    assert_eq!(write_asks(&conf, home).len(), 2);
}

#[test]
fn copies_follow_the_kind_and_the_bind_of_a_write() {
    let backup = AutoSupport { floor_backup: true, ..AutoSupport::default() };
    let path = Path::new("/home/u/Documents/notes.txt");
    let parent = PathBuf::from("/home/u/Documents");
    let file = Some(Some(TargetKind::File));
    let dir = Some(Some(TargetKind::Dir));
    let copies =
        |kind, fact, bind: Option<&WriteBind>| write_copies(kind, path, fact, bind, &backup);
    assert_eq!(
        copies(ExitKind::Write, file, Some(&WriteBind::TargetOnly)),
        [CoverAsk::Home(path.into())]
    );
    assert_eq!(
        copies(ExitKind::Write, file, Some(&WriteBind::Parent(parent.clone()))),
        [CoverAsk::Home(path.into()), CoverAsk::HomeDir(parent)]
    );
    assert_eq!(
        copies(ExitKind::Write, dir, Some(&WriteBind::Target)),
        [CoverAsk::HomeDir(path.into())]
    );
    assert_eq!(copies(ExitKind::Persistence, None, None), [CoverAsk::Home(path.into())]);
    assert_eq!(copies(ExitKind::Persistence, dir, None), [CoverAsk::HomeDir(path.into())]);
    assert_eq!(copies(ExitKind::AboveRoot, None, None), [CoverAsk::HomeDir(path.into())]);
    assert!(copies(ExitKind::Write, Some(Some(TargetKind::Other)), None).is_empty());
    assert!(copies(ExitKind::Secret, file, None).is_empty());
    assert!(copies(ExitKind::Config, file, None).is_empty());
    // Without the home store efrd copies nothing.
    let none = write_copies(ExitKind::Write, path, file, None, &AutoSupport::default());
    assert!(none.is_empty());
}
