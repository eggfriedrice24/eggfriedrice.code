//! The split listing (`diff-files` and `ls-files -o` at the same time) against the
//! listing in one `ls-files`, on a root that random changes keep changing, with the
//! index brought up to the root between the rounds as a snapshot does. The split one
//! must name each change that the one run names, the same untracked files and the same
//! names that are not UTF-8, and more only for a file that still holds the bytes of its
//! entry in the index.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use efr_scope::{Git, Home};
use efr_stdx::rng::Rng as _;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;

use crate::capture::{Limits, Listing, Reported, Take, one_listing, split_listing, stage};
use crate::runner::{Run, Runner};
use crate::store::Store;
use crate::{blob_id, link_id};

/// A root, its store and a runner.
struct Fixture {
    _base: tempfile::TempDir,
    root: PathBuf,
    runner: Runner,
    store: Store,
    reported: Reported,
    ready: AtomicBool,
}

impl Fixture {
    fn new() -> Fixture {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("home");
        fs::create_dir_all(home.join(".config")).unwrap();
        let root = base.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let clock = TestClock::starting_at("2026-10-23T10:00:00Z".parse().unwrap());
        let runner = Runner::new(
            Git::new(clock.shared()).isolated(),
            Home::new(&home).unwrap(),
            clock.shared(),
            Duration::from_secs(30),
            None,
        );
        let store = Store::new(&base.path().join("data/snapshots"), &root);
        Fixture {
            _base: base,
            root,
            runner,
            store,
            reported: Reported::default(),
            ready: AtomicBool::new(false),
        }
    }

    /// Brings the index up to the root, as a snapshot does: with `refresh` as the
    /// turn's first, with `fresh` as the first of a new store.
    async fn stage(&self, refresh: bool, fresh: bool) {
        let skip = ["jobs".to_owned()];
        let take = Take { ignored: false, refresh, fresh, skip: &skip };
        let limits = Limits { max_file_bytes: 4096, ..Limits::default() };
        stage(&self.runner, &self.reported, &self.store, &limits, take, &self.ready).await.unwrap();
    }

    /// Both listings of the root now.
    async fn listings(&self) -> (Listing, Listing) {
        let skip = ["jobs".to_owned()];
        let one = one_listing(&self.runner, &self.store, &self.root, &skip).await.unwrap();
        let split = split_listing(&self.runner, &self.store, &self.root, &skip).await.unwrap();
        (one, split)
    }

    /// The blob id of each entry of the persistent index, by path.
    async fn index_blobs(&self) -> HashMap<String, String> {
        let run = Run { work_tree: Some(&self.root), ..Run::default() };
        let out = self.runner.checked(&self.store, "ls-files", &["ls-files", "-s", "-z"], run);
        out.await
            .unwrap()
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .map(|record| {
                let text = std::str::from_utf8(record).unwrap();
                let (meta, path) = text.split_once('\t').unwrap();
                (path.to_owned(), meta.split(' ').nth(1).unwrap().to_owned())
            })
            .collect()
    }
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// The regular files and links below `root` that a snapshot takes: not in `jobs`, a
/// nested repository, `.gitignore` or an ignored `*.log`, and with a UTF-8 name.
fn taken_files(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(at) = stack.pop() {
        let Ok(entries) = fs::read_dir(root.join(&at)) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let relative = at.join(entry.file_name());
            let Some(text) = relative.to_str().map(str::to_owned) else { continue };
            if entry.file_type().unwrap().is_dir() {
                if text != "jobs" && !root.join(&relative).join(".git").exists() {
                    stack.push(relative);
                }
            } else if text != ".gitignore" && !text.ends_with(".log") {
                found.push(text);
            }
        }
    }
    found.sort();
    found
}

/// One random change of the root.
fn change(root: &Path, rng: &TestRng, n: u64) {
    let files = taken_files(root);
    let pick = || files[(rng.next_u64() % files.len() as u64) as usize].clone();
    let regular = |path: &str| fs::symlink_metadata(root.join(path)).is_ok_and(|m| m.is_file());
    match rng.next_u64() % 12 {
        0 if !files.is_empty() => {
            let path = pick();
            if regular(&path) {
                write(&root.join(&path), format!("longer bytes of round {n}\n").as_bytes());
            }
        }
        // NOTE: the same size, often in the second of the index: only a look at the
        // content finds it.
        1 if !files.is_empty() => {
            let path = pick();
            if regular(&path) {
                let mut bytes = fs::read(root.join(&path)).unwrap();
                if let Some(first) = bytes.first_mut() {
                    *first = if *first == b'x' { b'y' } else { b'x' };
                }
                write(&root.join(&path), &bytes);
            }
        }
        2 if !files.is_empty() => {
            let _ = fs::remove_file(root.join(pick()));
        }
        3 => write(&root.join(format!("d{}/new{n}.txt", n % 3)), b"new\n"),
        // NOTE: the same bytes again with an older time: new stat data, the same
        // content. git compares whole seconds, so a new time alone could match.
        4 if !files.is_empty() => {
            let path = pick();
            if regular(&path) {
                let file = root.join(&path);
                let bytes = fs::read(&file).unwrap();
                write(&file, &bytes);
                let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000 + n);
                fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
            }
        }
        5 if !files.is_empty() => {
            let path = pick();
            if regular(&path) {
                let file = root.join(&path);
                let mode = fs::metadata(&file).unwrap().permissions().mode();
                fs::set_permissions(&file, fs::Permissions::from_mode(mode ^ 0o111)).unwrap();
            }
        }
        6 if !files.is_empty() => {
            let path = pick();
            let _ = fs::remove_file(root.join(&path));
            symlink(format!("target{n}"), root.join(&path)).unwrap();
        }
        7 if !files.is_empty() => {
            let path = pick();
            let _ = fs::remove_file(root.join(&path));
            write(&root.join(&path).join("inner.txt"), b"inner\n");
        }
        8 => write(&root.join(format!("jobs/out{n}")), b"job\n"),
        9 => write(&root.join(format!("d{}/run{n}.log", n % 3)), b"log\n"),
        10 => {
            fs::create_dir_all(root.join(format!("nested{n}/.git"))).unwrap();
            write(&root.join(format!("nested{n}/x")), b"x\n");
        }
        _ => {
            let name = [b"d1/bad".as_slice(), &n.to_le_bytes()[..1], b"\xff.txt"].concat();
            write(&root.join(OsStr::from_bytes(&name)), b"bad\n");
        }
    }
}

fn sorted(list: &[String]) -> Vec<String> {
    let mut list = list.to_vec();
    list.sort();
    list
}

/// The paths of `changed` that are not in `one` and whose file or link does not hold
/// the bytes of its entry in `index`: none may be.
fn changed_bytes<'a>(
    root: &Path,
    changed: &'a BTreeSet<String>,
    one: &'a BTreeSet<String>,
    index: &HashMap<String, String>,
) -> Vec<&'a String> {
    changed
        .difference(one)
        .filter(|path| {
            let file = root.join(path);
            let id = match fs::symlink_metadata(&file) {
                Ok(meta) if meta.is_symlink() => {
                    link_id(fs::read_link(&file).unwrap().as_os_str().as_bytes())
                }
                Ok(meta) if meta.is_file() => blob_id(&fs::read(&file).unwrap()),
                _ => return true,
            };
            index.get(*path).map(String::as_str) != Some(id.as_str())
        })
        .collect()
}

#[tokio::test]
async fn the_split_listing_names_each_change_that_one_listing_names() {
    let fixture = Fixture::new();
    for n in 0..30 {
        let path = fixture.root.join(format!("d{}/f{n}.txt", n % 3));
        write(&path, format!("file {n}\n").as_bytes());
    }
    write(&fixture.root.join(".gitignore"), b"*.log\n");
    fixture.stage(false, true).await;
    let rng = TestRng::new(23);
    let mut more = 0;
    for round in 0..40_u64 {
        for step in 0..1 + rng.next_u64() % 5 {
            change(&fixture.root, &rng, round * 10 + step);
        }
        let (one, split) = fixture.listings().await;
        assert_eq!(sorted(&split.others), sorted(&one.others), "round {round}");
        assert_eq!(sorted(&split.not_utf8), sorted(&one.not_utf8), "round {round}");
        let missing: Vec<&String> = one.changed.difference(&split.changed).collect();
        assert!(missing.is_empty(), "round {round}: the split listing missed {missing:?}");
        let index = fixture.index_blobs().await;
        let changed = changed_bytes(&fixture.root, &split.changed, &one.changed, &index);
        assert!(changed.is_empty(), "round {round}: {changed:?} only in the split listing");
        more += split.changed.len() - one.changed.len();
        fixture.stage(round % 4 == 0, false).await;
        let (one, split) = fixture.listings().await;
        assert!(one.changed.is_empty(), "round {round}: {one:?}");
        let index = fixture.index_blobs().await;
        let changed = changed_bytes(&fixture.root, &split.changed, &one.changed, &index);
        assert!(changed.is_empty(), "round {round}: {changed:?} after the stage");
    }
    assert!(more > 0, "no round wrote a file again with its own bytes");
}
