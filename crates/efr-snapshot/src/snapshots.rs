//! [`Snapshots`]: the store of every root and the snapshots of each call. The turn's
//! refs are in `turn.rs`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use efr_protocol::{ConversationId, FileChanges, TurnId};
use efr_scope::{Git, Home};
use efr_stdx::time::{Clock, Stopwatch};

use crate::SnapshotError;
use crate::capture::{self, LeftOut, Limits, Reported, Staged};
use crate::changes::{self, RootChange, Shown};
use crate::project_git::GitDirs;
use crate::runner::{Run, Runner};
use crate::store::Store;
use crate::turn::TurnRecord;

mod slot;

pub(crate) use slot::{Pin, Slot};

/// How long one git run of a snapshot may take. The first snapshot of a large root
/// hashes every file.
pub const DEFAULT_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);

/// The directory below efr's data root that holds every store.
pub const SNAPSHOTS_DIR: &str = "snapshots";

/// What [`Snapshots`] is built from.
#[derive(Debug, Clone)]
pub struct SnapshotParts {
    /// The directory of the stores, `$D/snapshots`, which the sandbox masks.
    pub dir: PathBuf,
    /// How git runs; the store adds its own hardening.
    pub git: Git,
    /// The home directory, for git's environment.
    pub home: Home,
    /// The clock of git's timeout and of the commits' times.
    pub clock: Arc<dyn Clock>,
    /// The user's own `core.excludesFile`, resolved once by the daemon, because the
    /// store's git reads no global config.
    pub excludes_file: Option<PathBuf>,
    /// How long one git run may take.
    pub timeout: Duration,
}

/// A directory whose files a call can change: the turn's project, `$SCRATCH` or a
/// project that a line names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    path: PathBuf,
    shown: String,
    mode: Option<String>,
    dirs: Option<GitDirs>,
}

impl Root {
    /// The root at `path`, whose changed files show as `shown` and their path, such as
    /// `""`, `"$SCRATCH/"` or `"~/p/app/"` ([`shown_prefix`](crate::shown_prefix)).
    pub fn new(path: impl Into<PathBuf>, shown: impl Into<String>) -> Self {
        Root { path: path.into(), shown: shown.into(), mode: None, dirs: None }
    }

    /// The root with the git dirs of its project that the daemon trusts. Only a root
    /// with them gets a mirror of its refs; this crate never reads a `.git` file of a
    /// root to find them.
    pub fn with_git(mut self, dirs: Option<GitDirs>) -> Self {
        self.dirs = dirs;
        self
    }

    /// The root with the mode of the turn (`auto`, `cautious`, `manual`), which the
    /// turn's first commit of the root records.
    pub fn with_mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = Some(mode.into());
        self
    }

    /// The root's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The prefix of its changed files.
    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// The mode of the turn, when the caller named it.
    pub fn mode(&self) -> Option<&str> {
        self.mode.as_deref()
    }

    /// The trusted git dirs of the root's project.
    pub fn git(&self) -> Option<&GitDirs> {
        self.dirs.as_ref()
    }
}

/// The snapshots before one call, which [`Snapshots::after_call`] compares with the
/// snapshots after it.
///
/// A call snapshot that is dropped without [`Snapshots::after_call`], such as the one
/// of a call that the user interrupted, marks each of its roots as not complete for the
/// turn: the call may have changed paths that the turn's record lacks, so undo acts on
/// them only with `force`, and the end of the turn names the root.
#[derive(Debug, Default)]
pub struct CallSnapshot {
    turn: Option<TurnId>,
    before: Vec<(Root, String, Pin)>,
    /// The store that the drop marks; `None` once the snapshot after the call ran.
    owner: Option<Snapshots>,
    /// True when the call's command may still write after the snapshot after it.
    runs_on: bool,
}

impl CallSnapshot {
    /// True when no root was snapshotted.
    pub fn is_empty(&self) -> bool {
        self.before.is_empty()
    }

    /// Says that the call's command can still run after its tool answered, such as one
    /// that ran past its timeout: [`Snapshots::after_call`] then marks the roots as not
    /// complete for the turn, since a later write would be missing from the record.
    #[must_use]
    pub fn runs_on(mut self, runs_on: bool) -> Self {
        self.runs_on |= runs_on;
        self
    }

    /// Drops the snapshot of a call that did not run: its roots stay complete.
    pub fn discard(mut self) {
        self.owner = None;
    }

    /// Marks each root of the snapshot as not complete for its turn.
    fn mark_unfinished(&self, owner: &Snapshots) {
        let Some(turn) = self.turn else { return };
        for (root, _, _) in &self.before {
            owner.mark_unfinished(turn, root.path());
        }
    }
}

impl Drop for CallSnapshot {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take()
            && !self.before.is_empty()
        {
            tracing::info!(
                roots = self.before.len(),
                "a call ended without its snapshot after it; its roots need --force to undo"
            );
            self.mark_unfinished(&owner);
        }
    }
}

/// One snapshot of a root: its tree and what it left out.
#[derive(Debug, Clone, Default)]
pub(crate) struct Snap {
    pub(crate) tree: String,
    /// The files in the tree.
    pub(crate) files: u32,
    /// How long the snapshot took.
    pub(crate) ms: u64,
    /// What the snapshots of the root left out by now.
    pub(crate) left_out: Vec<LeftOut>,
    /// The modes other than 0644 and 0755 that they saw.
    pub(crate) modes: BTreeMap<String, u32>,
}

/// Why a snapshot has no tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Missed {
    /// The root has more files than `snapshot.max_files`, now or a short time ago.
    TooLarge,
    /// git or the file system failed.
    Failed,
}

/// efr's own snapshot store: one bare repository per root below [`SnapshotParts::dir`]
/// (efr's auto spec, section 10).
///
/// Cheap to clone; the clones share the stores and the running turns. One snapshot of a
/// store runs at a time; snapshots of different roots run together.
#[derive(Debug, Clone)]
pub struct Snapshots {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    dir: PathBuf,
    runner: Runner,
    slots: Mutex<HashMap<PathBuf, Arc<Slot>>>,
    turns: Mutex<HashMap<TurnId, TurnRecord>>,
    /// The roots whose unreadable files the log named already.
    reported: Reported,
}

impl Snapshots {
    /// The store of `parts`. Nothing is read or written until the first snapshot.
    pub fn new(parts: SnapshotParts) -> Self {
        let runner =
            Runner::new(parts.git, parts.home, parts.clock, parts.timeout, parts.excludes_file);
        Snapshots {
            inner: Arc::new(Inner {
                dir: parts.dir,
                runner,
                slots: Mutex::default(),
                turns: Mutex::default(),
                reported: Reported::default(),
            }),
        }
    }

    /// The directory of the stores.
    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    pub(crate) fn runner(&self) -> &Runner {
        &self.inner.runner
    }

    /// Snapshots `roots` before a call of the turn `turn` that can write them. The
    /// first snapshot of a root in a turn is the turn's `pre`: it takes the small
    /// ignored files too, and its commit and ref are written at once. A root that
    /// cannot be snapshotted is left out, with a line in the log; when that happens in
    /// the turn, the turn's record of the root is not complete, and the end of the
    /// turn says so.
    pub async fn before_call(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        roots: Vec<Root>,
        limits: Limits,
    ) -> CallSnapshot {
        let roots = self.canonical(roots).await;
        let taken = futures::future::join_all(roots.into_iter().map(|root| async move {
            let first = !self.in_turn(turn, root.path());
            let snap = self.snap(root.path(), &limits, first).await;
            let snap = if first {
                self.start_root(conversation, turn, &root, snap, &limits).await?
            } else {
                match snap {
                    Ok(snap) => snap,
                    Err(_) => {
                        self.mark_failed(turn, root.path());
                        return None;
                    }
                }
            };
            let pin = Pin::new(self.slot(root.path()), &snap.tree);
            Some((root, snap.tree, pin))
        }))
        .await;
        CallSnapshot {
            turn: Some(turn),
            before: taken.into_iter().flatten().collect(),
            owner: Some(self.clone()),
            runs_on: false,
        }
    }

    /// Snapshots the roots of `call` again after the call and lists what changed.
    /// `None` when nothing changed or nothing could be compared. The paths that
    /// changed join the turn's `touched` paths of the root, and
    /// `refs/efr/<conversation>/<turn>/last` moves to the new tree.
    pub async fn after_call(&self, call: CallSnapshot, limits: Limits) -> Option<FileChanges> {
        let mut call = call;
        call.owner = None;
        // NOTE: marked first, so the `last` ref that this snapshot moves says so too.
        if call.runs_on {
            call.mark_unfinished(self);
        }
        let turn = call.turn;
        let before = std::mem::take(&mut call.before);
        let compared =
            futures::future::join_all(before.into_iter().map(|(root, before, pin)| async move {
                let kept = turn.is_some_and(|turn| self.keeps(turn, root.path()));
                let compared = self.changes_since(root.path(), &before, &limits, kept).await;
                drop(pin);
                let Some((changes, tree)) = compared else {
                    if let Some(turn) = turn {
                        self.mark_failed(turn, root.path());
                    }
                    return None;
                };
                if let (Some(turn), Some(tree)) = (turn, tree) {
                    self.touch(turn, root.path(), &changes, &tree).await;
                }
                Some(Shown { root: root.path().to_path_buf(), shown: root.shown, changes })
            }))
            .await;
        changes::merge(compared.into_iter().flatten().collect())
    }

    /// Makes sure that the turn `turn` has its `pre` snapshot of each of `roots`, before
    /// a file tool writes into one of them.
    pub async fn before_write(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        roots: Vec<Root>,
        limits: Limits,
    ) {
        let roots = self.canonical(roots).await;
        let missing: Vec<Root> =
            roots.into_iter().filter(|root| !self.in_turn(turn, root.path())).collect();
        futures::future::join_all(missing.into_iter().map(|root| async move {
            let snap = self.snap(root.path(), &limits, true).await;
            self.start_root(conversation, turn, &root, snap, &limits).await;
        }))
        .await;
    }

    /// Adds the files that a file tool of the turn `turn` wrote (absolute paths) to the
    /// turn's `touched` paths of the roots that hold them, and moves the turn's `last`
    /// ref of each such root to a snapshot taken now. So an efrd stop before the turn's
    /// end keeps the write and its path, and `efr undo --force` takes it back.
    pub async fn after_write(&self, turn: TurnId, written: Vec<PathBuf>, limits: Limits) {
        let first = written.first().cloned().unwrap_or_default();
        let resolved = self
            .runner()
            .blocking(&first, move || written.iter().map(|path| resolve_parent(path)).collect())
            .await;
        let resolved: Vec<PathBuf> = match resolved {
            Ok(resolved) => resolved,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), "the paths of a file tool's write could not be read");
                return;
            }
        };
        let roots = self.touch_paths(turn, &resolved);
        futures::future::join_all(roots.into_iter().map(|root| async move {
            match self.snap(&root, &limits, false).await {
                Ok(snap) => self.touch(turn, &root, &[], &snap.tree).await,
                Err(_) => self.mark_failed(turn, &root),
            }
        }))
        .await;
    }

    pub(crate) fn lock_turns(&self) -> MutexGuard<'_, HashMap<TurnId, TurnRecord>> {
        self.inner.turns.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The lock of the store of the canonical `root`.
    pub(crate) fn slot(&self, root: &Path) -> Arc<Slot> {
        let mut slots = self.inner.slots.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(slots.entry(root.to_path_buf()).or_default())
    }

    /// `roots` with their canonical paths, without duplicates and without a root that
    /// is not a directory.
    pub(crate) async fn canonical(&self, roots: Vec<Root>) -> Vec<Root> {
        let first = roots.first().map(|root| root.path().to_path_buf()).unwrap_or_default();
        let resolved = self
            .runner()
            .blocking(&first, move || {
                let mut out: Vec<Root> = Vec::new();
                for root in roots {
                    let Ok(path) = std::fs::canonicalize(root.path()) else { continue };
                    if path.is_dir() && !out.iter().any(|known| known.path == path) {
                        out.push(Root { path, ..root });
                    }
                }
                out
            })
            .await;
        match resolved {
            Ok(roots) => roots,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), "the roots of a snapshot could not be read");
                Vec::new()
            }
        }
    }

    /// One snapshot of the canonical `root`, or why there is none, with a line in the
    /// log.
    pub(crate) async fn snap(
        &self,
        root: &Path,
        limits: &Limits,
        ignored: bool,
    ) -> Result<Snap, Missed> {
        let slot = self.slot(root);
        let _turn = slot.enter().await;
        let now = self.runner().clock().now();
        if slot.skipped(now) {
            tracing::debug!(root = %root.display(), "no snapshot of this root: it was skipped a short time ago");
            return Err(Missed::TooLarge);
        }
        let store = Store::new(self.dir(), root);
        let watch = Stopwatch::start();
        let cached = slot.tree();
        let taken: Result<Result<String, &str>, SnapshotError> = async {
            let captured = capture::stage(
                self.runner(),
                &self.inner.reported,
                &store,
                limits,
                ignored,
                cached.is_none(),
                &slot.ready,
            )
            .await?;
            slot.note(&captured);
            match captured.staged {
                Staged::Skipped(reason) => Ok(Err(reason)),
                Staged::Unchanged if cached.is_some() => Ok(Ok(cached.clone().unwrap_or_default())),
                Staged::Unchanged | Staged::Changed => {
                    capture::write_tree(self.runner(), &store).await.map(Ok)
                }
            }
        }
        .await;
        tracing::debug!(phase = "snapshot", root = %root.display(), ignored, elapsed_ms = %watch, "phase=snapshot elapsed_ms={}", watch);
        match taken {
            Ok(Ok(tree)) => {
                slot.set_tree(Some(tree.clone()));
                let files = store.index_entries().unwrap_or(0);
                Ok(Snap {
                    tree,
                    files: u32::try_from(files).unwrap_or(u32::MAX),
                    ms: u64::try_from(watch.elapsed().as_millis()).unwrap_or(u64::MAX),
                    left_out: slot.left_out(),
                    modes: slot.modes(),
                })
            }
            Ok(Err(reason)) => {
                slot.skip(now);
                tracing::debug!(root = %root.display(), reason, "no snapshot of this root");
                Err(Missed::TooLarge)
            }
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %root.display(), "a snapshot failed");
                Err(Missed::Failed)
            }
        }
    }

    /// The changes of the canonical `root` since the tree `before`: the index takes
    /// the root's changes and is compared with `before` directly. Without `tree`, the
    /// new tree is written only when a later snapshot needs it, which saves a git run
    /// in each call that changes files; with it, a call that changed files writes its
    /// tree at once and returns it. `None` with a line in the log when nothing could
    /// be compared.
    async fn changes_since(
        &self,
        root: &Path,
        before: &str,
        limits: &Limits,
        tree: bool,
    ) -> Option<(Vec<RootChange>, Option<String>)> {
        let slot = self.slot(root);
        let _turn = slot.enter().await;
        let now = self.runner().clock().now();
        if slot.skipped(now) {
            tracing::debug!(root = %root.display(), "no snapshot of this root: it was skipped a short time ago");
            return None;
        }
        let store = Store::new(self.dir(), root);
        let watch = Stopwatch::start();
        let cached = slot.tree();
        let compared: Result<Result<_, &str>, SnapshotError> = async {
            let captured = capture::stage(
                self.runner(),
                &self.inner.reported,
                &store,
                limits,
                false,
                cached.is_none(),
                &slot.ready,
            )
            .await?;
            slot.note(&captured);
            let staged = captured.staged;
            match staged {
                Staged::Skipped(reason) => Ok(Err(reason)),
                Staged::Unchanged if cached.as_deref() == Some(before) => {
                    Ok(Ok((Vec::new(), None)))
                }
                Staged::Unchanged | Staged::Changed => {
                    if staged == Staged::Changed {
                        slot.set_tree(None);
                    }
                    let mut args: Vec<&str> = changes::DIFF_INDEX.to_vec();
                    args.push(before);
                    let out =
                        self.runner().checked(&store, "diff-index", &args, Run::default()).await?;
                    let changes = self.read_changes(&store, &out).await?;
                    let written = if tree && !changes.is_empty() {
                        let written = capture::write_tree(self.runner(), &store).await?;
                        slot.set_tree(Some(written.clone()));
                        Some(written)
                    } else {
                        None
                    };
                    Ok(Ok((changes, written)))
                }
            }
        }
        .await;
        tracing::debug!(phase = "snapshot_after", root = %root.display(), elapsed_ms = %watch, "phase=snapshot_after elapsed_ms={}", watch);
        match compared {
            Ok(Ok(changes)) => Some(changes),
            Ok(Err(reason)) => {
                slot.skip(now);
                tracing::debug!(root = %root.display(), reason, "no snapshot of this root");
                None
            }
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %root.display(), "a snapshot failed");
                None
            }
        }
    }

    /// The changes from tree `from` to tree `to` in the store of `root`, or `None` with
    /// a line in the log.
    pub(crate) async fn compare(
        &self,
        root: &Path,
        from: &str,
        to: &str,
    ) -> Option<Vec<RootChange>> {
        let store = Store::new(self.dir(), root);
        match self.diff_trees(&store, from, to).await {
            Ok(changes) => Some(changes),
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %root.display(), "two snapshots could not be compared");
                None
            }
        }
    }

    pub(crate) async fn diff_trees(
        &self,
        store: &Store,
        from: &str,
        to: &str,
    ) -> Result<Vec<RootChange>, SnapshotError> {
        let mut args: Vec<&str> = changes::DIFF_TREE.to_vec();
        args.extend([from, to]);
        let out = self.runner().checked(store, "diff-tree", &args, Run::default()).await?;
        self.read_changes(store, &out).await
    }

    /// The changes in `out`, the output of [`changes::DIFF_TREE`] or
    /// [`changes::DIFF_INDEX`] in `store`, with the files that the snapshot left out
    /// for their size marked ([`changes::mark_left_out`]).
    async fn read_changes(
        &self,
        store: &Store,
        out: &[u8],
    ) -> Result<Vec<RootChange>, SnapshotError> {
        let mut found = changes::parse(out);
        if !found.iter().any(|change| change.kind == efr_protocol::ChangeKind::Deleted) {
            return Ok(found);
        }
        let root = store.root().to_path_buf();
        self.runner()
            .blocking(&root.clone(), move || {
                changes::mark_left_out(&root, &mut found);
                found
            })
            .await
    }

    /// The paths of `changes` (and where a renamed file was) that the ignore rules of
    /// `root` name now: a snapshot took them as small ignored files, such as `.env`,
    /// and `turn_diff` shows them without their content. Empty, with a line in the
    /// log, when git cannot tell.
    pub(crate) async fn ignored(&self, root: &Path, changes: &[RootChange]) -> Vec<String> {
        let store = Store::new(self.dir(), root);
        let paths: Vec<String> = changes
            .iter()
            .flat_map(|change| std::iter::once(change.path.clone()).chain(change.from.clone()))
            .collect();
        match capture::split_ignored(self.runner(), &store, root, paths).await {
            Ok((ignored, _)) => ignored,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %root.display(), "the ignored files of a turn could not be found");
                Vec::new()
            }
        }
    }

    /// Writes a commit of `tree` with `message`, at `time` (`<seconds> +0000`).
    pub(crate) async fn commit(
        &self,
        store: &Store,
        tree: &str,
        parent: Option<&str>,
        message: String,
        time: &str,
    ) -> Result<String, SnapshotError> {
        let mut args = vec!["commit-tree", tree];
        if let Some(parent) = parent {
            args.extend(["-p", parent]);
        }
        let out = self
            .runner()
            .checked(
                store,
                "commit-tree",
                &args,
                Run {
                    stdin: Some(message.into_bytes()),
                    commit_time: Some(time.to_owned()),
                    ..Run::default()
                },
            )
            .await?;
        let commit = String::from_utf8(out)
            .map(|commit| commit.trim().to_owned())
            .map_err(|_| SnapshotError::BadOutput { command: "commit-tree" })?;
        if commit.is_empty() || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(SnapshotError::BadOutput { command: "commit-tree" });
        }
        Ok(commit)
    }
}

/// `path` with its parent directory resolved, so it compares with the canonical roots;
/// `path` itself when the parent is gone.
fn resolve_parent(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => {
            std::fs::canonicalize(parent).map_or_else(|_| path.to_path_buf(), |dir| dir.join(name))
        }
        _ => path.to_path_buf(),
    }
}
