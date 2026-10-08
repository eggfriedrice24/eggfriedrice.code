//! [`Snapshots`]: the store of every root, the snapshots of each call and turn, and the
//! refs of each turn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use efr_protocol::{ConversationId, FileChanges, TurnId};
use efr_scope::{Git, Home};
use efr_stdx::time::{Clock, Stopwatch};

use crate::SnapshotError;
use crate::capture::{self, Limits, Staged};
use crate::changes::{self, RootChange, Shown};
use crate::runner::{Run, Runner};
use crate::store::{self, Store};

/// How long one git run of a snapshot may take. The first snapshot of a large root
/// hashes every file.
pub const DEFAULT_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a root that a snapshot skipped (too many files) stays skipped before the
/// next snapshot counts its files again. Counting walks the whole root, the cost that
/// the skip saves.
const SKIP_FOR: Duration = Duration::from_secs(10 * 60);

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
    pre: &'a str,
    post: &'a str,
    hidden: &'a [String],
}

/// One root of a running turn and its first tree.
#[derive(Debug, Clone)]
struct TurnRoot {
    root: Root,
    pre: String,
    _pin: Pin,
}

/// The roots of a running turn.
#[derive(Debug, Clone)]
struct TurnRecord {
    conversation: ConversationId,
    roots: Vec<TurnRoot>,
}

/// One store in this process: the gate that lets one snapshot, commit or collection of
/// the store run at a time, the tree of its index at its last snapshot, the trees
/// that running turns and calls still need ([`Pin`]), and until when the root stays
/// skipped.
#[derive(Debug)]
pub(crate) struct Slot {
    gate: tokio::sync::Semaphore,
    tree: Mutex<Option<String>>,
    live: Mutex<Vec<String>>,
    skipped_until: Mutex<Option<jiff::Timestamp>>,
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            gate: tokio::sync::Semaphore::new(1),
            tree: Mutex::new(None),
            live: Mutex::new(Vec::new()),
            skipped_until: Mutex::new(None),
        }
    }
}

/// A tree of a store that a running turn (its first tree) or a running call (its tree
/// before) still needs. No ref names such a tree, and once the index moves on, the
/// index no longer reaches it either, so the collector's prune could remove its
/// objects; the collector pins every live tree under a ref first. Dropping the pin
/// ends the need.
#[derive(Debug)]
pub(crate) struct Pin {
    slot: Arc<Slot>,
    tree: String,
}

impl Pin {
    fn new(slot: Arc<Slot>, tree: &str) -> Pin {
        slot.lock_live().push(tree.to_owned());
        Pin { slot, tree: tree.to_owned() }
    }
}

impl Clone for Pin {
    fn clone(&self) -> Pin {
        Pin::new(Arc::clone(&self.slot), &self.tree)
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        let mut live = self.slot.lock_live();
        if let Some(at) = live.iter().position(|tree| tree == &self.tree) {
            live.swap_remove(at);
        }
    }
}

impl Slot {
    /// Waits for the store's turn. The gate is never closed, so the wait cannot fail;
    /// `None` would only mean that the store runs unguarded once.
    pub(crate) async fn enter(&self) -> Option<tokio::sync::SemaphorePermit<'_>> {
        self.gate.acquire().await.ok()
    }

    pub(crate) fn tree(&self) -> Option<String> {
        self.tree.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub(crate) fn set_tree(&self, tree: Option<String>) {
        *self.tree.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = tree;
    }

    /// The trees that running turns and calls still need, each once, sorted.
    pub(crate) fn live_trees(&self) -> Vec<String> {
        let mut trees = self.lock_live().clone();
        trees.sort();
        trees.dedup();
        trees
    }

    /// True while the root stays skipped at `now`.
    fn skipped(&self, now: jiff::Timestamp) -> bool {
        self.lock_skipped().is_some_and(|until| now < until)
    }

    /// Skips the root from `now` for [`SKIP_FOR`].
    fn skip(&self, now: jiff::Timestamp) {
        *self.lock_skipped() = now.checked_add(SKIP_FOR).ok();
    }

    fn lock_skipped(&self) -> MutexGuard<'_, Option<jiff::Timestamp>> {
        self.skipped_until.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_live(&self) -> MutexGuard<'_, Vec<String>> {
        self.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
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
            let tree = self.snap(root.path(), &limits, first).await?;
            if first {
                self.add_to_turn(conversation, turn, &root, &tree);
            }
            let pin = Pin::new(self.slot(root.path()), &tree);
            Some((root, tree, pin))
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
            if let Some(tree) = self.snap(root.path(), &limits, true).await {
                self.add_to_turn(conversation, turn, &root, &tree);
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
                let changes = if post == turn_root.pre {
                    Some(Vec::new())
                } else {
                    self.compare(root.path(), &turn_root.pre, &post).await
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

    fn add_to_turn(&self, conversation: ConversationId, turn: TurnId, root: &Root, tree: &str) {
        let mut turns = self.lock_turns();
        let record =
            turns.entry(turn).or_insert_with(|| TurnRecord { conversation, roots: Vec::new() });
        if !record.roots.iter().any(|known| known.root.path() == root.path()) {
            let pin = Pin::new(self.slot(root.path()), tree);
            record.roots.push(TurnRoot { root: root.clone(), pre: tree.to_owned(), _pin: pin });
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
    async fn snap(&self, root: &Path, limits: &Limits, ignored: bool) -> Option<String> {
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
            match capture::stage(self.runner(), &store, limits, ignored, cached.is_none()).await? {
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
                Some(tree)
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
            let staged =
                capture::stage(self.runner(), &store, limits, false, cached.is_none()).await?;
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
        let meta = serde_json::json!({
            "conversation": conversation.to_string(),
            "turn": turn.to_string(),
            "root": root.to_string_lossy(),
            "shown": shown,
            "head": head,
            "at": now.to_string(),
            "hidden": hidden,
        });
        let pre_message = format!("efr turn {turn} pre\n\nefr-meta: {meta}\n");
        let pre_commit = self.commit(&store, pre, None, pre_message, &time).await?;
        let post_message = format!("efr turn {turn} post\n\nefr-meta: {meta}\n");
        let post_commit = self.commit(&store, post, Some(&pre_commit), post_message, &time).await?;
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
