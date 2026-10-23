//! One snapshot of one root: the tree of what is there now, written into the root's
//! store.
//!
//! A snapshot asks git what differs from the persistent index (the modified, deleted
//! and untracked files: one `ls-files`, or for an index of [`SPLIT_FROM_ENTRIES`] or
//! more, `diff-files` and `ls-files -o` at the same time, which stat the entries on
//! several threads), so a root where nothing changed costs one listing and the tree of
//! the last snapshot comes back. The turn's first snapshot of a root first brings the
//! stat data of the index up to date ([`refresh`]), so the later listings of the turn
//! read no unchanged file. New untracked files above the size limit and nested
//! repositories are left out, and a file of the index that grew past its limit leaves
//! the index, so no call hashes it again. Only the changed paths are added, literally
//! ([`stage`]), and `write-tree` writes the tree when one is needed ([`write_tree`]).
//! With the ignored files of `IgnoredFiles::Small`, one more `ls-files` lists the
//! ignored files and directories at the same time as the listing, and the small ones
//! outside build and dependency directories and outside nested repositories are added
//! too. In a root where nothing changed, the refresh and these runs are all that the
//! first snapshot of a turn costs. A root without a snapshot yet lists its ignored
//! files only after the `max_files` check, so a root that is too big never pays for
//! that listing.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use crate::SnapshotError;
use crate::runner::{MAX_STDERR_BYTES, Output, Run, Runner};
use crate::store::{FORMAT, Store};

mod record;

pub(crate) use record::Record;
pub use record::{LeftOut, LeftOutReason};

/// The largest ignored file a snapshot takes.
pub const MAX_IGNORED_BYTES: u64 = 1024 * 1024;

/// The most ignored files that one snapshot adds, so a large ignored directory that
/// the skip list does not name cannot make it slow.
const MAX_IGNORED_FILES: usize = 2000;

/// Directories whose ignored files a snapshot never takes: builds and package
/// managers make them again.
pub const SKIPPED_DIRS: &[&str] =
    &["target", "node_modules", ".venv", "venv", "__pycache__", "dist", "build", ".next", ".cache"];

/// What a snapshot takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// New untracked files above this size are left out.
    pub max_file_bytes: u64,
    /// True takes small ignored files (up to 1 MiB, outside [`SKIPPED_DIRS`]) at the
    /// first and last snapshot of a turn.
    pub ignored_small: bool,
    /// A root with more files than this is not snapshotted.
    pub max_files: u64,
    /// The largest object dir of a project whose refs a turn mirrors, and the most
    /// that one turn's mirror adds to a store; 0 turns the mirror off.
    pub mirror_max_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_file_bytes: 10 * 1024 * 1024,
            ignored_small: true,
            max_files: 20_000,
            mirror_max_bytes: 512 * 1024 * 1024,
        }
    }
}

/// What [`stage`] did to the persistent index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Staged {
    /// Nothing differed from the index.
    Unchanged,
    /// The index took the changes of the root.
    Changed,
    /// The root was not snapshotted, and why, for the debug log.
    Skipped(&'static str),
}

/// What `ls-files` said differs from the index.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Listing {
    /// Tracked files that changed or are gone.
    pub(crate) changed: BTreeSet<String>,
    /// Untracked files that are not ignored; a nested repository ends with `/`.
    pub(crate) others: Vec<String>,
    /// Names that are not UTF-8, lossy.
    pub(crate) not_utf8: Vec<String>,
}

/// What one [`stage`] did and found.
#[derive(Debug)]
pub(crate) struct Captured {
    pub(crate) staged: Staged,
    /// What it left out and the modes it saw.
    pub(crate) record: Record,
    /// True when it looked at the ignored files, so its record holds them.
    pub(crate) scanned_ignored: bool,
}

impl Captured {
    fn skipped(reason: &'static str) -> Captured {
        let mut record = Record::default();
        record.leave("", LeftOutReason::RootTooLarge);
        Captured { staged: Staged::Skipped(reason), record, scanned_ignored: false }
    }
}

/// Reads the output of a split listing ([`split_listing`]): `changed` of `diff-files
/// -z --name-only` and `others` of `ls-files -z -o`, a path per record.
pub(crate) fn parse_split(changed: &[u8], others: &[u8]) -> Listing {
    let mut listing = Listing::default();
    for (out, other) in [(changed, false), (others, true)] {
        for record in out.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
            // NOTE: as in `parse_listing`, a name that is not UTF-8 waits for a full
            // snapshot.
            let Ok(path) = std::str::from_utf8(record) else {
                listing.not_utf8.push(String::from_utf8_lossy(record).into_owned());
                continue;
            };
            if other {
                listing.others.push(path.to_owned());
            } else {
                listing.changed.insert(path.to_owned());
            }
        }
    }
    listing
}

/// Reads `ls-files -z -t` output: a tag, a space and a path per record.
pub(crate) fn parse_listing(out: &[u8]) -> Listing {
    let mut listing = Listing::default();
    for record in out.split(|byte| *byte == 0).filter(|record| record.len() > 2) {
        let Ok(text) = std::str::from_utf8(record) else {
            // NOTE: a name that is not UTF-8 cannot be passed back as a pathspec
            // through this crate's text, so it waits for a full snapshot.
            listing.not_utf8.push(String::from_utf8_lossy(&record[2..]).into_owned());
            continue;
        };
        let (tag, path) = text.split_at(2);
        match tag {
            "? " => listing.others.push(path.to_owned()),
            _ => {
                listing.changed.insert(path.to_owned());
            }
        }
    }
    listing
}

/// What one [`stage`] takes besides the changed files.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Take<'a> {
    /// Also take the small ignored files.
    pub(crate) ignored: bool,
    /// First bring the stat data of the index up to the root ([`refresh`]): the turn's
    /// first snapshot of the root does it.
    pub(crate) refresh: bool,
    /// No tree of the store is known in this process: copy the project's exclude file
    /// first, and drop what the index holds below `skip`.
    pub(crate) fresh: bool,
    /// The directories at the root's top that the snapshot leaves out.
    pub(crate) skip: &'a [String],
}

/// Brings the persistent index of `store` up to its root with `limits`, as `take`
/// says. `ready` is false until this process checked the store's format
/// ([`prepare`]). `reported` keeps which roots the log named for their unreadable
/// files.
pub(crate) async fn stage(
    runner: &Runner,
    reported: &Reported,
    store: &Store,
    limits: &Limits,
    take: Take<'_>,
    ready: &AtomicBool,
) -> Result<Captured, SnapshotError> {
    let Take { ignored, refresh: refreshed, fresh, skip } = take;
    let root = store.root().to_path_buf();
    if !store.exists() {
        let prepared = store.clone();
        runner.blocking(store.dir(), move || prepared.make_dir()).await??;
        runner.init(store).await?;
        let prepared = store.clone();
        runner.blocking(store.dir(), move || prepared.finish_init()).await??;
    }
    let format_checked = ready.load(Ordering::Acquire);
    if fresh || !format_checked {
        prepare(runner, store, !format_checked).await?;
        ready.store(true, Ordering::Release);
    }
    let unlocked = store.clone();
    if runner.blocking(&root, move || unlocked.clear_leftover_locks()).await? {
        tracing::warn!(root = %root.display(), "removed a lock that a killed git left in a snapshot store");
    }
    if ignored || fresh {
        let copied = store.clone();
        runner.blocking(&root, move || copied.copy_exclude()).await??;
    }
    let stamped = store.clone();
    let now = SystemTime::from(runner.clock().now());
    runner.blocking(&root, move || stamped.stamp(now)).await?;
    if fresh && !skip.is_empty() {
        drop_skipped(runner, store, skip).await?;
    }

    // NOTE: with `snapshot.ignored` off, the ignored files are still listed at the
    // first and last snapshot of a turn, only to name them in the record.
    let scan_ignored = ignored;
    let entries = store.index_entries().unwrap_or(0);
    if entries > limits.max_files {
        return Ok(Captured::skipped(TOO_MANY_FILES));
    }
    if refreshed && entries > 0 {
        refresh(runner, store, &root).await?;
    }
    // NOTE: both listings only read the root and the index. A root whose index holds
    // files had a snapshot, so it was below max_files: its two listings run together,
    // and the scan of the ignored files adds no git run to the time of the snapshot. A
    // root with an empty index (its first snapshot, or one that max_files skipped) runs
    // the ignored listing only once the max_files check passed: a root that is too big
    // never pays for it.
    let together = scan_ignored && entries > 0;
    let (listed, early) = tokio::join!(listing(runner, store, &root, skip, entries), async {
        if together {
            ignored_listing(runner, store, &root, skip).await.map(Some)
        } else {
            Ok(None)
        }
    });
    let listing = listed?;
    let early = early?;
    if entries + listing.others.len() as u64 > limits.max_files {
        return Ok(Captured::skipped(TOO_MANY_FILES));
    }
    let ignored_listed = match early {
        Some(listed) => listed,
        None if scan_ignored => ignored_listing(runner, store, &root, skip).await?,
        None => Vec::new(),
    };
    let mut record = Record::default();
    for path in &listing.not_utf8 {
        record.leave(path, LeftOutReason::NotUtf8);
    }
    let max_file_bytes = limits.max_file_bytes;
    let changed: Vec<String> = listing.changed.into_iter().collect();
    let base = root.clone();
    let sized = runner.blocking(&root, move || by_size(&base, changed, max_file_bytes)).await?;
    record.extend(sized.record);
    let mut paths = sized.small;
    for path in &sized.large {
        record.leave(path, LeftOutReason::TooLarge);
    }
    let mut large = sized.large;
    if !sized.mid.is_empty() {
        let (ignored, kept) = split_ignored(runner, store, &root, sized.mid).await?;
        for path in &ignored {
            record.leave(path, LeftOutReason::IgnoredTooLarge);
        }
        large.extend(ignored);
        paths.extend(kept);
    }
    let left = !large.is_empty();
    if left {
        drop_large(runner, store, &root, &large).await?;
    }
    let others = listing.others;
    let base = root.clone();
    let (others, kept_record) =
        runner.blocking(&root, move || keep_others(&base, others, max_file_bytes)).await?;
    record.extend(kept_record);
    paths.extend(others);

    let mut forced = Vec::new();
    if scan_ignored {
        let mut entries: Vec<String> = Vec::new();
        for entry in ignored_listed.split(|byte| *byte == 0).filter(|entry| !entry.is_empty()) {
            match std::str::from_utf8(entry) {
                Ok(entry) => entries.push(entry.to_owned()),
                Err(_) => record.leave(&String::from_utf8_lossy(entry), LeftOutReason::NotUtf8),
            }
        }
        if limits.ignored_small {
            let base = root.clone();
            let (taken, ignored_record) =
                runner.blocking(&root, move || small_ignored(&base, &entries)).await?;
            record.extend(ignored_record);
            forced = taken;
        } else {
            for entry in &entries {
                record.leave(entry, LeftOutReason::IgnoredOff);
            }
        }
    }
    let captured =
        |staged: Staged, record: Record| Captured { staged, record, scanned_ignored: scan_ignored };

    if paths.is_empty() && forced.is_empty() {
        return Ok(captured(if left { Staged::Changed } else { Staged::Unchanged }, record));
    }
    if !paths.is_empty() {
        let output = runner
            .run(
                store,
                &["add", "-A", "--ignore-errors", "--pathspec-from-file=-", "--pathspec-file-nul"],
                Run {
                    work_tree: Some(&root),
                    stdin: Some(nul_list(&paths)),
                    literal: true,
                    stderr: true,
                    ..Run::default()
                },
            )
            .await?;
        if !output.success {
            locked(store, "add")?;
            // NOTE: with --ignore-errors git adds what it can read and fails for the
            // rest, such as a file that only root may read; the tree holds the rest.
            log_failed_add(reported, &root, Added::Files, &output);
            for path in unreadable(&output.stderr).paths {
                record.leave(&path, LeftOutReason::Unreadable);
            }
        }
    }
    if !forced.is_empty() {
        let output = runner
            .run(
                store,
                &["add", "-f", "--ignore-errors", "--pathspec-from-file=-", "--pathspec-file-nul"],
                Run {
                    work_tree: Some(&root),
                    stdin: Some(nul_list(&forced)),
                    literal: true,
                    stderr: true,
                    ..Run::default()
                },
            )
            .await?;
        if !output.success {
            locked(store, "add")?;
            log_failed_add(reported, &root, Added::Ignored, &output);
            for path in unreadable(&output.stderr).paths {
                record.leave(&path, LeftOutReason::Unreadable);
            }
        }
    }
    Ok(captured(Staged::Changed, record))
}

/// Makes sure that `store` takes exact bytes: its `info/attributes` holds
/// [`ATTRIBUTES`](crate::store::ATTRIBUTES), and with `format`, a store of an older
/// format drops its index once, so no blob that an in-tree `.gitattributes` changed
/// stays in its stat cache. The format mark comes last, so a stop on the way repeats
/// the check.
pub(crate) async fn prepare(
    runner: &Runner,
    store: &Store,
    format: bool,
) -> Result<(), SnapshotError> {
    let checked = store.clone();
    runner.blocking(store.dir(), move || checked.ensure_attributes()).await??;
    if !format {
        return Ok(());
    }
    let format = runner.run(store, &["config", "--get", "efr.format"], Run::default()).await?;
    if String::from_utf8_lossy(&format.stdout).trim() == FORMAT {
        return Ok(());
    }
    tracing::info!(root = %store.root().display(), "a snapshot store of an older format takes a full snapshot once");
    let dropped = store.clone();
    runner.blocking(store.dir(), move || dropped.drop_index()).await??;
    runner.checked(store, "config", &["config", "efr.format", FORMAT], Run::default()).await?;
    Ok(())
}

/// The exit code of a git that stopped at a fatal error, such as a path that went away
/// between the listing and the add: it added nothing.
const FATAL_EXIT: i32 = 128;

/// Which `git add` of a snapshot failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Added {
    /// The changed and new files.
    Files,
    /// The small ignored files.
    Ignored,
}

/// The roots and adds whose unreadable files the log named already, so that a root
/// with files that git cannot read gives one line for the life of the process, not one
/// at each snapshot.
#[derive(Debug, Default)]
pub(crate) struct Reported(Mutex<HashSet<(PathBuf, Added)>>);

impl Reported {
    /// True the first time for `root` and `added`.
    fn first(&self, root: &Path, added: Added) -> bool {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).insert((root.to_path_buf(), added))
    }
}

/// What git said about the files that it could not read: how many, and the first.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Unreadable {
    pub(crate) count: usize,
    pub(crate) first: Option<String>,
    /// Every path named, in order.
    pub(crate) paths: Vec<String>,
}

/// Reads the `error: open("<path>"): <reason>` lines of a `git add --ignore-errors`.
pub(crate) fn unreadable(stderr: &[u8]) -> Unreadable {
    let mut found = Unreadable::default();
    for line in String::from_utf8_lossy(stderr).lines() {
        let Some((path, _)) =
            line.strip_prefix("error: open(\"").and_then(|rest| rest.rsplit_once("\"): "))
        else {
            continue;
        };
        found.count += 1;
        if found.first.is_none() {
            found.first = Some(path.to_owned());
        }
        found.paths.push(path.to_owned());
    }
    found
}

/// Logs a `git add` of `root` that failed. A fatal exit added nothing, so it is a
/// warning each time. Files that git could not read, such as files that only root may
/// read, stay unreadable at each later snapshot: their count and the first path go to
/// the debug log once per root and add for the life of the process.
pub(crate) fn log_failed_add(reported: &Reported, root: &Path, added: Added, output: &Output) {
    if output.code == Some(FATAL_EXIT) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let error = stderr.lines().find(|line| line.starts_with("fatal:")).unwrap_or_default();
        tracing::warn!(root = %root.display(), ?added, error, "git stopped and added no file to a snapshot");
        return;
    }
    if !reported.first(root, added) {
        return;
    }
    let found = unreadable(&output.stderr);
    tracing::debug!(
        root = %root.display(),
        ?added,
        count = found.count,
        first = found.first.as_deref().unwrap_or_default(),
        cut = output.stderr.len() >= MAX_STDERR_BYTES,
        "some files could not be added to a snapshot; this is logged once per root"
    );
}

/// Why a root has no snapshot when it holds more files than `snapshot.max_files`.
const TOO_MANY_FILES: &str = "the root has more files than snapshot.max_files";

/// From this many entries of the index on, a listing stats them on several threads
/// ([`split_listing`]); below it, one git run lists all ([`one_listing`]). git's
/// preload of the index gives a thread to each 500 entries, so a smaller index gets
/// one thread, and the second run only adds its start.
pub(crate) const SPLIT_FROM_ENTRIES: u64 = 1000;

/// The modified, deleted and untracked files of the root of `store`, whose index holds
/// `entries` entries, without the directories of `skip`.
async fn listing(
    runner: &Runner,
    store: &Store,
    root: &Path,
    skip: &[String],
    entries: u64,
) -> Result<Listing, SnapshotError> {
    if entries >= SPLIT_FROM_ENTRIES {
        split_listing(runner, store, root, skip).await
    } else {
        one_listing(runner, store, root, skip).await
    }
}

/// [`listing`] in one `ls-files`, which stats each entry of the index in turn, then
/// walks the root for the untracked files.
pub(crate) async fn one_listing(
    runner: &Runner,
    store: &Store,
    root: &Path,
    skip: &[String],
) -> Result<Listing, SnapshotError> {
    let mut args: Vec<String> = ["ls-files", "-z", "-t", "-m", "-d", "-o", "--exclude-standard"]
        .map(str::to_owned)
        .to_vec();
    args.extend(skipped_pathspecs(skip));
    let out = runner
        .checked(store, "ls-files", &args, Run { work_tree: Some(root), ..Run::default() })
        .await?;
    Ok(parse_listing(&out))
}

/// [`listing`] in two git runs at the same time: `diff-files` stats the entries of the
/// index on several threads and names the changed and deleted ones, and `ls-files -o`
/// walks the root for the untracked files. Both compare the stat data of an entry as
/// `ls-files -m` does, a racily clean entry by its content, so each change that
/// [`one_listing`] finds is in it. It also names an entry whose stat data changed
/// while its content did not (`ls-files` reads that content to drop it); `git add`
/// then only brings its stat data up to date. `--ignore-submodules=dirty` compares a
/// nested repository's commit as `ls-files` does, without a git run inside it.
pub(crate) async fn split_listing(
    runner: &Runner,
    store: &Store,
    root: &Path,
    skip: &[String],
) -> Result<Listing, SnapshotError> {
    let mut changed: Vec<String> = [
        "diff-files",
        "-z",
        "--name-only",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=dirty",
    ]
    .map(str::to_owned)
    .to_vec();
    changed.extend(skipped_pathspecs(skip));
    let mut others: Vec<String> =
        ["ls-files", "-z", "-o", "--exclude-standard"].map(str::to_owned).to_vec();
    others.extend(skipped_pathspecs(skip));
    let run = || Run { work_tree: Some(root), ..Run::default() };
    let (changed, others) = tokio::join!(
        runner.checked(store, "diff-files", &changed, run()),
        runner.checked(store, "ls-files", &others, run()),
    );
    Ok(parse_split(&changed?, &others?))
}

/// Brings the stat data of the persistent index of `store` up to its root
/// (`update-index -q --refresh`), once per turn. A listing reads the content of each
/// entry that is racily clean (git compares whole seconds, so a file written in the
/// second of the index or later may have changed with the same stat data) or whose
/// stat data changed while its content did not, such as a file that a tool wrote
/// again with the same bytes. A listing never writes the index, so without this each
/// listing of the turn would read those files again. When an entry needs it, the
/// refresh writes the index again, which ends the racy state of an older file. It
/// changes no blob and no entry: an entry whose content changed keeps its old stat
/// data, so a listing still names it, and an entry whose file is as new as the index
/// stays racy, so a later listing still reads its content.
async fn refresh(runner: &Runner, store: &Store, root: &Path) -> Result<(), SnapshotError> {
    let output = runner
        .run(
            store,
            &["update-index", "-q", "--ignore-submodules", "--refresh"],
            Run { work_tree: Some(root), ..Run::default() },
        )
        .await?;
    if !output.success {
        // NOTE: the refresh only saves work; a listing reads the index as it is.
        locked(store, "update-index")?;
        tracing::debug!(root = %root.display(), code = ?output.code, "the stat data of a snapshot index could not be refreshed");
    }
    Ok(())
}

/// The ignored files and directories of the root of `store`, from `ls-files`, without
/// the directories of `skip`.
async fn ignored_listing(
    runner: &Runner,
    store: &Store,
    root: &Path,
    skip: &[String],
) -> Result<Vec<u8>, SnapshotError> {
    let mut args: Vec<String> = ["ls-files", "-z", "-o", "-i", "--exclude-standard", "--directory"]
        .map(str::to_owned)
        .to_vec();
    args.extend(skipped_pathspecs(skip));
    runner.checked(store, "ls-files", &args, Run { work_tree: Some(root), ..Run::default() }).await
}

/// The pathspecs that leave the directories of `skip` out of a listing: `--` and one
/// exclude for each, read literally from the root's top. A listing with only excludes
/// lists the rest of the root.
pub(crate) fn skipped_pathspecs(skip: &[String]) -> Vec<String> {
    if skip.is_empty() {
        return Vec::new();
    }
    let mut args = vec!["--".to_owned()];
    args.extend(skip.iter().map(|name| format!(":(top,exclude,literal){name}")));
    args
}

/// Drops from the index of `store` what an older snapshot took below the directories
/// of `skip`, so no later tree holds it.
async fn drop_skipped(
    runner: &Runner,
    store: &Store,
    skip: &[String],
) -> Result<(), SnapshotError> {
    let mut args: Vec<String> =
        ["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--"].map(str::to_owned).to_vec();
    args.extend(skip.iter().map(|name| format!(":(top,literal){name}")));
    let root = store.root().to_path_buf();
    runner.checked(store, "rm", &args, Run { work_tree: Some(&root), ..Run::default() }).await?;
    Ok(())
}

/// Fails when a `command` that failed left the index as it was because another git
/// held its lock: the index then does not hold the root's changes, and a comparison
/// with it would list none.
fn locked(store: &Store, command: &'static str) -> Result<(), SnapshotError> {
    if store.index_lock().exists() {
        return Err(SnapshotError::GitFailed { command, store: store.git_dir() });
    }
    Ok(())
}

/// Writes the tree of the persistent index of `store`.
pub(crate) async fn write_tree(runner: &Runner, store: &Store) -> Result<String, SnapshotError> {
    let tree = runner.checked(store, "write-tree", &["write-tree"], Run::default()).await?;
    let tree = String::from_utf8(tree)
        .map(|tree| tree.trim().to_owned())
        .map_err(|_| SnapshotError::BadOutput { command: "write-tree" })?;
    if tree.is_empty() || !tree.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SnapshotError::BadOutput { command: "write-tree" });
    }
    Ok(tree)
}

/// The changed files of the index by their size now.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Sized {
    /// Up to [`MAX_IGNORED_BYTES`], gone, or not a regular file: added as before.
    pub(crate) small: Vec<String>,
    /// Above [`MAX_IGNORED_BYTES`] and up to the limit of untracked files: added unless
    /// the file is ignored.
    pub(crate) mid: Vec<String>,
    /// Above the limit of untracked files: left out.
    pub(crate) large: Vec<String>,
    /// The modes that `lstat` saw.
    pub(crate) record: Record,
}

/// Sorts `changed`, paths of the index that `ls-files` named, by the size of their
/// file now. The limits of new files apply to a file of the index too: an ignored file
/// taken while small, or a database taken below the limit, may grow without bound, and
/// each call would hash it and store it again.
pub(crate) fn by_size(root: &Path, changed: Vec<String>, max_file_bytes: u64) -> Sized {
    let mut sized = Sized::default();
    for path in changed {
        let metadata = fs::symlink_metadata(root.join(&path)).ok();
        sized.record.look(&path, metadata.as_ref());
        let bytes = match metadata {
            Some(metadata) if metadata.is_file() => metadata.len(),
            _ => 0,
        };
        if bytes > max_file_bytes {
            sized.large.push(path);
        } else if bytes > MAX_IGNORED_BYTES {
            sized.mid.push(path);
        } else {
            sized.small.push(path);
        }
    }
    sized
}

/// Splits `paths` into the ignored ones and the rest, by the ignore rules alone (`git
/// check-ignore --no-index`), because a path of the index is never ignored otherwise.
pub(crate) async fn split_ignored(
    runner: &Runner,
    store: &Store,
    root: &Path,
    paths: Vec<String>,
) -> Result<(Vec<String>, Vec<String>), SnapshotError> {
    let output = runner
        .run(
            store,
            &["check-ignore", "--no-index", "-z", "--stdin"],
            Run { work_tree: Some(root), stdin: Some(nul_list(&paths)), ..Run::default() },
        )
        .await?;
    let ignored: BTreeSet<&[u8]> =
        output.stdout.split(|byte| *byte == 0).filter(|path| !path.is_empty()).collect();
    Ok(paths.into_iter().partition(|path| ignored.contains(path.as_bytes())))
}

/// Takes the files of `large` out of the persistent index of `store`, so the snapshot
/// leaves them out as it leaves out a new file of their size.
async fn drop_large(
    runner: &Runner,
    store: &Store,
    root: &Path,
    large: &[String],
) -> Result<(), SnapshotError> {
    for path in large {
        tracing::debug!(root = %root.display(), path = %path, "a file grew past its limit and is left out of the snapshot");
    }
    let output = runner
        .run(
            store,
            &["update-index", "--force-remove", "-z", "--stdin"],
            Run { work_tree: Some(root), stdin: Some(nul_list(large)), ..Run::default() },
        )
        .await?;
    if !output.success {
        locked(store, "update-index")?;
        tracing::debug!(root = %root.display(), "some large files could not leave a snapshot");
    }
    Ok(())
}

/// The paths of `list`, each followed by a NUL, for `--pathspec-from-file`.
fn nul_list(list: &[String]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(list.iter().map(|path| path.len() + 1).sum());
    for path in list {
        bytes.extend_from_slice(path.as_bytes());
        bytes.push(0);
    }
    bytes
}

/// The new untracked files of `others` that a snapshot takes: no nested repository
/// (an entry that ends with `/`) and no regular file above `max_file_bytes`. A link
/// is taken as a link.
pub(crate) fn keep_others(
    root: &Path,
    others: Vec<String>,
    max_file_bytes: u64,
) -> (Vec<String>, Record) {
    let mut record = Record::default();
    let mut kept = Vec::new();
    for path in others {
        if path.ends_with('/') {
            record.leave(&path, LeftOutReason::NestedRepo);
            continue;
        }
        let metadata = fs::symlink_metadata(root.join(&path));
        record.look(&path, metadata.as_ref().ok());
        let keep = match metadata {
            Ok(metadata) if metadata.is_file() => {
                let keep = metadata.len() <= max_file_bytes;
                if !keep {
                    tracing::debug!(path = %path, bytes = metadata.len(), "a large untracked file is left out of the snapshot");
                    record.leave(&path, LeftOutReason::TooLarge);
                }
                keep
            }
            Ok(metadata) => !metadata.is_dir(),
            // NOTE: gone since git listed it; git add drops it again.
            Err(_) => true,
        };
        if keep {
            kept.push(path);
        }
    }
    (kept, record)
}

/// The ignored files that a snapshot takes from `entries` of `ls-files -o -i
/// --directory`: files up to [`MAX_IGNORED_BYTES`] and links, outside
/// [`SKIPPED_DIRS`] and outside nested repositories; an ignored directory (an entry
/// that ends with `/`) is walked without following links, up to [`MAX_IGNORED_FILES`]
/// files in all. The record names what it left out and the modes it saw.
pub(crate) fn small_ignored(root: &Path, entries: &[String]) -> (Vec<String>, Record) {
    let mut found = Found::default();
    let mut repos = NestedRepos::default();
    for entry in entries {
        let relative = entry.trim_end_matches('/');
        if relative.is_empty() {
            continue;
        }
        if found.full() {
            found.record.leave(relative, LeftOutReason::IgnoredCap);
            continue;
        }
        if in_skipped_dir(relative) {
            found.record.leave(relative, LeftOutReason::SkippedDir);
            continue;
        }
        // NOTE: git lists the ignored files in a nested repository one by one, but
        // `git add` refuses each of them. If the snapshot took them, they would fail
        // again at each turn, and each failure costs an add and a new tree.
        if repos.holds(root, relative, entry.ends_with('/')) {
            continue;
        }
        if entry.ends_with('/') {
            walk(root, PathBuf::from(relative), &mut found, 0);
        } else {
            found.take(root, relative);
        }
    }
    found.kept.truncate(MAX_IGNORED_FILES);
    (found.kept, found.record)
}

/// The ignored files that [`small_ignored`] takes, and its record.
#[derive(Debug, Default)]
struct Found {
    kept: Vec<String>,
    record: Record,
}

impl Found {
    fn full(&self) -> bool {
        self.kept.len() >= MAX_IGNORED_FILES
    }

    /// Takes `relative` when it is a small file or a link, else names it.
    fn take(&mut self, root: &Path, relative: &str) {
        let Ok(metadata) = fs::symlink_metadata(root.join(relative)) else { return };
        self.record.look(relative, Some(&metadata));
        if metadata.file_type().is_symlink()
            || (metadata.is_file() && metadata.len() <= MAX_IGNORED_BYTES)
        {
            self.kept.push(relative.to_owned());
        } else if metadata.is_file() {
            self.record.leave(relative, LeftOutReason::IgnoredTooLarge);
        }
    }
}

/// The directories of a root that are nested repositories (that hold a `.git`). Each
/// directory is looked up once.
#[derive(Debug, Default)]
struct NestedRepos {
    known: HashMap<PathBuf, bool>,
}

impl NestedRepos {
    /// True when a directory above `relative` is a nested repository, or `relative`
    /// itself when it is a directory (`dir`). The root does not count.
    fn holds(&mut self, root: &Path, relative: &str, dir: bool) -> bool {
        let parts: Vec<&str> = relative.split('/').collect();
        let dirs = if dir { parts.len() } else { parts.len().saturating_sub(1) };
        let mut at = PathBuf::new();
        for part in parts.into_iter().take(dirs) {
            at.push(part);
            let nested = *self
                .known
                .entry(at.clone())
                .or_insert_with(|| fs::symlink_metadata(root.join(&at).join(".git")).is_ok());
            if nested {
                return true;
            }
        }
        false
    }
}

/// True when a part of `relative` is a skipped directory or a `.git`.
pub(crate) fn in_skipped_dir(relative: &str) -> bool {
    relative.split('/').any(|part| part == ".git" || SKIPPED_DIRS.contains(&part))
}

/// Adds the small files below the ignored directory `relative` to `found`.
fn walk(root: &Path, relative: PathBuf, found: &mut Found, depth: usize) {
    const MAX_DEPTH: usize = 16;
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(root.join(&relative)) else { return };
    let mut names: Vec<_> = entries.filter_map(Result::ok).map(|entry| entry.file_name()).collect();
    names.sort();
    for name in names {
        let child = relative.join(&name);
        let Some(child_text) = child.to_str() else {
            found.record.leave(&child.to_string_lossy(), LeftOutReason::NotUtf8);
            continue;
        };
        if found.full() {
            found.record.leave(child_text, LeftOutReason::IgnoredCap);
            return;
        }
        if in_skipped_dir(child_text) {
            found.record.leave(child_text, LeftOutReason::SkippedDir);
            continue;
        }
        match fs::symlink_metadata(root.join(&child)) {
            Ok(metadata) if metadata.is_dir() => {
                // NOTE: a nested repository is not captured (efr's auto spec, 10.2).
                if root.join(&child).join(".git").exists() {
                    found.record.leave(child_text, LeftOutReason::NestedRepo);
                } else {
                    walk(root, child, found, depth + 1);
                }
            }
            Ok(_) => found.take(root, child_text),
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests;
