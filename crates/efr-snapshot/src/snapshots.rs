//! [`Snapshots`]: the store of every root, the snapshots of each call and turn, and the
//! refs of each turn.

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
use crate::meta::Meta;
use crate::runner::{Run, Runner};
use crate::store::{self, Store};

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
}

impl Root {
    /// The root at `path`, whose changed files show as `shown` and their path, such as
    /// `""`, `"$SCRATCH/"` or `"~/p/app/"` ([`shown_prefix`](crate::shown_prefix)).
    pub fn new(path: impl Into<PathBuf>, shown: impl Into<String>) -> Self {
        Root { path: path.into(), shown: shown.into() }
    }

    /// The root's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The prefix of its changed files.
    pub fn shown(&self) -> &str {
        &self.shown
    }
}

/// The snapshots before one call, which [`Snapshots::after_call`] compares with the
/// snapshots after it.
#[derive(Debug, Clone, Default)]
pub struct CallSnapshot {
    before: Vec<(Root, String, Pin)>,
}

impl CallSnapshot {
    /// True when no root was snapshotted.
    pub fn is_empty(&self) -> bool {
        self.before.is_empty()
    }
}

/// What [`Snapshots::keep_turn`] keeps of a turn in one store.
#[derive(Debug, Clone, Copy)]
struct Kept<'a> {
    conversation: ConversationId,
    turn: TurnId,
    pre: &'a Snap,
    post: &'a Snap,
    hidden: &'a [String],
}

/// One snapshot of a root: its tree and what it left out.
#[derive(Debug, Clone, Default)]
pub(crate) struct Snap {
    pub(crate) tree: String,
    /// The files in the tree.
    pub(crate) files: u32,
    /// What the snapshots of the root left out by now.
    pub(crate) left_out: Vec<LeftOut>,
    /// The modes other than 0644 and 0755 that they saw.
    pub(crate) modes: BTreeMap<String, u32>,
}

/// One root of a running turn and its first snapshot.
#[derive(Debug, Clone)]
struct TurnRoot {
    root: Root,
    pre: Snap,
    _pin: Pin,
}

/// The roots of a running turn.
#[derive(Debug, Clone)]
struct TurnRecord {
    conversation: ConversationId,
    roots: Vec<TurnRoot>,
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
    /// first snapshot of a root in a turn is the turn's `pre` and takes the small
    /// ignored files too. A root that cannot be snapshotted is left out, with a line in
    /// the log.
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
            let snap = self.snap(root.path(), &limits, first).await?;
            if first {
                self.add_to_turn(conversation, turn, &root, &snap);
            }
            let pin = Pin::new(self.slot(root.path()), &snap.tree);
            Some((root, snap.tree, pin))
        }))
        .await;
        CallSnapshot { before: taken.into_iter().flatten().collect() }
    }

    /// Snapshots the roots of `call` again after the call and lists what changed.
    /// `None` when nothing changed or nothing could be compared.
    pub async fn after_call(&self, call: CallSnapshot, limits: Limits) -> Option<FileChanges> {
        let compared = futures::future::join_all(call.before.into_iter().map(
            |(root, before, pin)| async move {
                let changes = self.changes_since(root.path(), &before, &limits).await;
                drop(pin);
                let changes = changes?;
                Some(Shown { root: root.path().to_path_buf(), shown: root.shown, changes })
            },
        ))
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
            if let Some(snap) = self.snap(root.path(), &limits, true).await {
                self.add_to_turn(conversation, turn, &root, &snap);
            }
        }))
        .await;
    }

    /// Ends the turn `turn`: snapshots each of its roots a last time (`post`), keeps
    /// both trees as `refs/efr/<conversation>/<turn>/pre` and `/post`, and lists what
    /// the turn changed. `None` when the turn took no snapshot or changed nothing.
    pub async fn finish_turn(&self, turn: TurnId, limits: Limits) -> Option<FileChanges> {
        let record = self.lock_turns().remove(&turn)?;
        let conversation = record.conversation;
        let compared = futures::future::join_all(record.roots.into_iter().map(|turn_root| {
            async move {
                let root = turn_root.root;
                let post = self.snap(root.path(), &limits, true).await?;
                let changes = if post.tree == turn_root.pre.tree {
                    Some(Vec::new())
                } else {
                    self.compare(root.path(), &turn_root.pre.tree, &post.tree).await
                };
                let hidden = match &changes {
                    Some(changes) if !changes.is_empty() => self.ignored(root.path(), changes).await,
                    _ => Vec::new(),
                };
                let kept = Kept { conversation, turn, pre: &turn_root.pre, post: &post, hidden: &hidden };
                if let Err(error) = self.keep_turn(root.path(), root.shown(), kept).await {
                    tracing::warn!(error = %efr_stdx::with_causes(&error), root = %root.path().display(), "the snapshots of a turn could not be kept");
                }
                let changes = changes?;
                if changes.is_empty() {
                    return None;
                }
                Some(Shown { root: root.path().to_path_buf(), shown: root.shown, changes })
            }
        }))
        .await;
        changes::merge(compared.into_iter().flatten().collect())
    }

    /// True when the running turn `turn` has a snapshot of `root`.
    fn in_turn(&self, turn: TurnId, root: &Path) -> bool {
        self.lock_turns()
            .get(&turn)
            .is_some_and(|record| record.roots.iter().any(|known| known.root.path() == root))
    }

    fn add_to_turn(&self, conversation: ConversationId, turn: TurnId, root: &Root, snap: &Snap) {
        let mut turns = self.lock_turns();
        let record =
            turns.entry(turn).or_insert_with(|| TurnRecord { conversation, roots: Vec::new() });
        if !record.roots.iter().any(|known| known.root.path() == root.path()) {
            let pin = Pin::new(self.slot(root.path()), &snap.tree);
            record.roots.push(TurnRoot { root: root.clone(), pre: snap.clone(), _pin: pin });
        }
    }

    fn lock_turns(&self) -> MutexGuard<'_, HashMap<TurnId, TurnRecord>> {
        self.inner.turns.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The lock of the store of the canonical `root`.
    pub(crate) fn slot(&self, root: &Path) -> Arc<Slot> {
        let mut slots = self.inner.slots.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(slots.entry(root.to_path_buf()).or_default())
    }

    /// `roots` with their canonical paths, without duplicates and without a root that
    /// is not a directory.
    async fn canonical(&self, roots: Vec<Root>) -> Vec<Root> {
        let first = roots.first().map(|root| root.path().to_path_buf()).unwrap_or_default();
        let resolved = self
            .runner()
            .blocking(&first, move || {
                let mut out: Vec<Root> = Vec::new();
                for root in roots {
                    let Ok(path) = std::fs::canonicalize(root.path()) else { continue };
                    if path.is_dir() && !out.iter().any(|known| known.path == path) {
                        out.push(Root { path, shown: root.shown });
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

    /// One snapshot of the canonical `root`, or `None` with a line in the log.
    async fn snap(&self, root: &Path, limits: &Limits, ignored: bool) -> Option<Snap> {
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
                Some(Snap {
                    tree,
                    files: u32::try_from(files).unwrap_or(u32::MAX),
                    left_out: slot.left_out(),
                    modes: slot.modes(),
                })
            }
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

    /// The changes of the canonical `root` since the tree `before`: the index takes
    /// the root's changes and is compared with `before` directly. The new tree is
    /// written only when a later snapshot needs it, which saves a git run in each call
    /// that changes files. `None` with a line in the log when nothing could be
    /// compared.
    async fn changes_since(
        &self,
        root: &Path,
        before: &str,
        limits: &Limits,
    ) -> Option<Vec<RootChange>> {
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
        let compared: Result<Result<Vec<RootChange>, &str>, SnapshotError> = async {
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
                Staged::Unchanged if cached.as_deref() == Some(before) => Ok(Ok(Vec::new())),
                Staged::Unchanged | Staged::Changed => {
                    if staged == Staged::Changed {
                        slot.set_tree(None);
                    }
                    let mut args: Vec<&str> = changes::DIFF_INDEX.to_vec();
                    args.push(before);
                    let out =
                        self.runner().checked(&store, "diff-index", &args, Run::default()).await?;
                    self.read_changes(&store, &out).await.map(Ok)
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
    async fn compare(&self, root: &Path, from: &str, to: &str) -> Option<Vec<RootChange>> {
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
    async fn ignored(&self, root: &Path, changes: &[RootChange]) -> Vec<String> {
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

    /// Writes the commits of the turn's first and last trees of `root` and points
    /// `refs/efr/<conversation>/<turn>/pre` and `/post` at them. The `efr-meta:` line
    /// of both holds the root, the prefix of its paths, the project's `HEAD` and the
    /// changed paths that are ignored (`hidden`).
    async fn keep_turn(
        &self,
        root: &Path,
        shown: &str,
        kept: Kept<'_>,
    ) -> Result<(), SnapshotError> {
        let Kept { conversation, turn, pre, post, hidden } = kept;
        let store = Store::new(self.dir(), root);
        let slot = self.slot(root);
        let _turn = slot.enter().await;
        let now = self.runner().clock().now();
        let time = format!("{} +0000", now.as_second());
        let head = {
            let root = root.to_path_buf();
            self.runner().blocking(&root.clone(), move || store::project_head(&root)).await?
        };
        let mut meta = Meta {
            conversation: conversation.to_string(),
            turn: turn.to_string(),
            phase: "pre".to_owned(),
            root: root.to_string_lossy().into_owned(),
            shown: shown.to_owned(),
            head,
            at: now.to_string(),
            hidden: hidden.to_vec(),
            files: pre.files,
            ..Meta::default()
        };
        meta.set_left_out(&pre.left_out);
        meta.set_modes(&pre.modes);
        let pre_commit = self.commit(&store, &pre.tree, None, meta.message(), &time).await?;
        meta.phase = "post".to_owned();
        meta.files = post.files;
        meta.set_left_out(&post.left_out);
        meta.set_modes(&post.modes);
        let post_commit =
            self.commit(&store, &post.tree, Some(&pre_commit), meta.message(), &time).await?;
        let base = format!("refs/efr/{conversation}/{turn}");
        let updates = format!("update {base}/pre {pre_commit}\nupdate {base}/post {post_commit}\n");
        self.runner()
            .checked(
                &store,
                "update-ref",
                &["update-ref", "--stdin"],
                Run { stdin: Some(updates.into_bytes()), ..Run::default() },
            )
            .await?;
        Ok(())
    }

    async fn commit(
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
