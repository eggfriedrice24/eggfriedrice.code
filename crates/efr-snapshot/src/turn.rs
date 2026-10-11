//! The refs of a running turn in each store: `refs/efr/<conversation>/<turn>/pre` at
//! the turn's first snapshot of a root, `/last` after each call that changed files in
//! it, and `/post` at the turn's end ([`Snapshots::turn_end`]). Each commit's
//! `efr-meta:` line carries the paths that the turn's calls changed (`touched`), so an
//! undo acts on those paths only, and a change between two calls stays.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use efr_protocol::{ConversationId, FileChanges, TurnId};

use crate::SnapshotError;
use crate::capture::{LeftOut, LeftOutReason, Limits};
use crate::changes::{self, RootChange, Shown};
use crate::meta::{GitMeta, MAX_TOUCHED, Meta};
use crate::mirror::{MIRROR_FETCH_FAILED, Mirror};
use crate::project_git::{GitDirs, Pins, ProjectGit, ProjectRefs};
use crate::runner::Run;
use crate::snapshots::{Missed, Pin, Root, Snap, Snapshots};
use crate::store::{self, Store};

/// Which snapshot of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Phase {
    /// The turn's first snapshot of a root.
    Pre,
    /// The turn's last snapshot of a root, at its end.
    Post,
}

/// One kept snapshot of a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInfo {
    /// The root.
    pub root: PathBuf,
    /// Which snapshot.
    pub phase: Phase,
    /// The commit in the root's store.
    pub commit: String,
    /// The files in its tree.
    pub files: u32,
    /// The paths that it left out.
    pub skipped: u32,
    /// How long it took.
    pub ms: u64,
}

/// What [`Snapshots::turn_end`] found at the end of a turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnEnd {
    /// What the turn changed in files; `None` when it changed nothing.
    pub changes: Option<FileChanges>,
    /// The `pre` and `post` snapshot of each root that kept them.
    pub snapshots: Vec<SnapshotInfo>,
    /// What the `pre` or `post` snapshot of a root left out, as absolute paths, each
    /// once; a root with too many files is itself in the list.
    pub left_out: Vec<(PathBuf, LeftOutReason)>,
    /// The roots whose record of the turn is not complete, and why:
    /// [`NOT_COVERED_FAILED`] or [`NOT_COVERED_STORE_CHANGED`].
    pub not_covered: Vec<(PathBuf, String)>,
    /// True when at least one root kept a complete record of the turn.
    pub kept: bool,
}

/// The reason in [`TurnEnd::not_covered`] for a root whose snapshot failed in the turn.
pub const NOT_COVERED_FAILED: &str = "a snapshot failed";
/// The reason in [`TurnEnd::not_covered`] for a root whose `pre` ref is gone or moved,
/// as when an approved command outside the sandbox changed efr's store.
pub const NOT_COVERED_STORE_CHANGED: &str = "the snapshot store changed";
/// The start of the reason in [`TurnEnd::not_covered`] for a git root whose refs the
/// turn's mirror does not hold; why follows, such as `refs: shallow`,
/// `refs: alternates`, `refs: over snapshot.mirror_max_mib`, `refs: mirror over
/// budget` or `refs: the git dir is not trusted`.
pub const NOT_COVERED_REFS: &str = "refs: ";

/// What the `mirror` field of a `pre` line says when the store holds the mirror.
const MIRROR_TAKEN: &str = "taken";

/// One root of a running turn.
#[derive(Debug, Clone)]
pub(crate) struct TurnRoot {
    pub(crate) root: Root,
    pub(crate) state: State,
}

/// What a running turn has of one root.
#[derive(Debug, Clone)]
pub(crate) enum State {
    /// The first snapshot, its commit and what the turn's calls changed since.
    Kept(Box<Kept>),
    /// The root has more files than `snapshot.max_files`.
    TooLarge,
    /// The first snapshot or its ref failed, so no later snapshot becomes `pre`.
    Failed,
}

/// The turn's `pre` of one root, and what the turn's calls changed.
#[derive(Debug, Clone)]
pub(crate) struct Kept {
    pub(crate) pre: Snap,
    pub(crate) commit: String,
    /// The `efr-meta:` line of `pre`, the base of `last` and `post`.
    pub(crate) meta: Meta,
    pub(crate) touched: BTreeSet<String>,
    /// True when a later snapshot of the root failed in the turn.
    pub(crate) failed: bool,
    /// What the turn's first snapshot took of the project's refs; `None` for a root
    /// that is no git work tree.
    pub(crate) mirror: Option<Mirror>,
    _pin: Pin,
}

/// The roots of a running turn.
#[derive(Debug, Clone)]
pub(crate) struct TurnRecord {
    pub(crate) conversation: ConversationId,
    pub(crate) roots: Vec<TurnRoot>,
}

/// `refs/efr/<conversation>/<turn>/<name>`.
pub(crate) fn turn_ref(conversation: ConversationId, turn: TurnId, name: &str) -> String {
    format!("refs/efr/{conversation}/{turn}/{name}")
}

impl Snapshots {
    /// True when the running turn `turn` has a snapshot of `root`, or tried one.
    pub(crate) fn in_turn(&self, turn: TurnId, root: &Path) -> bool {
        self.lock_turns()
            .get(&turn)
            .is_some_and(|record| record.roots.iter().any(|known| known.root.path() == root))
    }

    /// True when the running turn `turn` kept a `pre` of `root`.
    pub(crate) fn keeps(&self, turn: TurnId, root: &Path) -> bool {
        self.lock_turns().get(&turn).is_some_and(|record| {
            record
                .roots
                .iter()
                .any(|known| known.root.path() == root && matches!(known.state, State::Kept(_)))
        })
    }

    /// Makes `snap`, the turn's first snapshot of `root`, its `pre`: writes its commit
    /// and ref at once. A snapshot or a ref that failed marks the root for the turn, so
    /// no later snapshot becomes `pre`. The snapshot comes back when the call can use
    /// it.
    pub(crate) async fn start_root(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        root: &Root,
        snap: Result<Snap, Missed>,
        limits: &Limits,
    ) -> Option<Snap> {
        let state = match &snap {
            Ok(snap) => match self.write_pre(conversation, turn, root, snap, limits).await {
                Ok((commit, meta, mirror)) => State::Kept(Box::new(Kept {
                    pre: snap.clone(),
                    commit,
                    meta,
                    touched: BTreeSet::new(),
                    failed: false,
                    mirror,
                    _pin: Pin::new(self.slot(root.path()), &snap.tree),
                })),
                Err(error) => {
                    tracing::warn!(error = %efr_stdx::with_causes(&error), root = %root.path().display(), "the first snapshot of a turn could not be kept");
                    State::Failed
                }
            },
            Err(Missed::TooLarge) => State::TooLarge,
            Err(Missed::Failed) => State::Failed,
        };
        let usable = matches!(state, State::Kept(_));
        let mut turns = self.lock_turns();
        let record =
            turns.entry(turn).or_insert_with(|| TurnRecord { conversation, roots: Vec::new() });
        if record.roots.iter().any(|known| known.root.path() == root.path()) {
            return snap.ok();
        }
        record.roots.push(TurnRoot { root: root.clone(), state });
        if usable { snap.ok() } else { None }
    }

    /// Marks the kept `pre` of `root` in the turn `turn` as not complete: a later
    /// snapshot failed, so the turn's calls may have changed paths that `touched` lacks.
    pub(crate) fn mark_failed(&self, turn: TurnId, root: &Path) {
        let mut turns = self.lock_turns();
        let Some(record) = turns.get_mut(&turn) else { return };
        for known in &mut record.roots {
            if known.root.path() == root
                && let State::Kept(kept) = &mut known.state
            {
                kept.failed = true;
            }
        }
    }

    /// Adds the paths of `changes` to the turn's `touched` paths of `root`, and moves
    /// its `last` ref to `tree`.
    pub(crate) async fn touch(
        &self,
        turn: TurnId,
        root: &Path,
        changes: &[RootChange],
        tree: &str,
    ) {
        let meta = {
            let mut turns = self.lock_turns();
            let Some(record) = turns.get_mut(&turn) else { return };
            let conversation = record.conversation;
            let Some(kept) = record.roots.iter_mut().find_map(|known| match &mut known.state {
                State::Kept(kept) if known.root.path() == root => Some(kept),
                _ => None,
            }) else {
                return;
            };
            for change in changes {
                kept.touched.insert(change.path.clone());
                if let Some(from) = &change.from {
                    kept.touched.insert(from.clone());
                }
            }
            let mut meta = kept.meta.clone();
            meta.phase = "last".to_owned();
            set_touched(&mut meta, &kept.touched);
            meta.failed = kept.failed;
            (conversation, meta)
        };
        let (conversation, mut meta) = meta;
        let slot = self.slot(root);
        meta.set_left_out(&slot.left_out());
        meta.set_modes(&slot.modes());
        if let Err(error) = self.move_last(conversation, turn, root, tree, meta).await {
            tracing::warn!(error = %efr_stdx::with_causes(&error), root = %root.display(), "the last snapshot of a call could not be kept");
            self.mark_failed(turn, root);
        }
    }

    /// Adds the absolute `paths` to the turn's `touched` paths of each kept root that
    /// holds them.
    pub(crate) fn touch_paths(&self, turn: TurnId, paths: &[PathBuf]) {
        let mut turns = self.lock_turns();
        let Some(record) = turns.get_mut(&turn) else { return };
        for known in &mut record.roots {
            let State::Kept(kept) = &mut known.state else { continue };
            for path in paths {
                let Ok(relative) = path.strip_prefix(known.root.path()) else { continue };
                match relative.to_str() {
                    Some("") => {}
                    Some(relative) => {
                        kept.touched.insert(relative.to_owned());
                    }
                    // NOTE: a snapshot never holds a name that is not UTF-8, so undo
                    // cannot act on it either way.
                    None => {}
                }
            }
        }
    }

    /// Ends the turn `turn`: snapshots each of its roots a last time (`post`), keeps
    /// it as `refs/efr/<conversation>/<turn>/post` (a child of `pre`), and lists what
    /// the turn changed, what its snapshots left out, and the roots whose record is not
    /// complete.
    pub async fn turn_end(&self, turn: TurnId, limits: Limits) -> TurnEnd {
        let Some(record) = self.lock_turns().remove(&turn) else { return TurnEnd::default() };
        let conversation = record.conversation;
        let ended = futures::future::join_all(
            record.roots.into_iter().map(|known| self.end_root(conversation, turn, known, limits)),
        )
        .await;
        let mut end = TurnEnd::default();
        let mut shown = Vec::new();
        for root in ended {
            end.snapshots.extend(root.snapshots);
            for left in root.left_out {
                if !end.left_out.contains(&left) {
                    end.left_out.push(left);
                }
            }
            end.not_covered.extend(root.not_covered);
            end.kept |= root.kept;
            shown.extend(root.changes);
        }
        end.changes = changes::merge(shown);
        end
    }

    /// Ends the turn `turn` as [`turn_end`](Self::turn_end) does, and lists only what
    /// it changed. `None` when the turn took no snapshot or changed nothing.
    pub async fn finish_turn(&self, turn: TurnId, limits: Limits) -> Option<FileChanges> {
        self.turn_end(turn, limits).await.changes
    }

    async fn end_root(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        known: TurnRoot,
        limits: Limits,
    ) -> RootEnd {
        let root = known.root;
        let path = root.path().to_path_buf();
        let mut end = RootEnd::default();
        let kept = match known.state {
            State::Kept(kept) => kept,
            State::TooLarge => {
                end.left_out.push((path, LeftOutReason::RootTooLarge));
                return end;
            }
            State::Failed => {
                end.not_covered.push((path, NOT_COVERED_FAILED.to_owned()));
                return end;
            }
        };
        end.snapshots.push(SnapshotInfo {
            root: path.clone(),
            phase: Phase::Pre,
            commit: kept.commit.clone(),
            files: kept.pre.files,
            skipped: u32::try_from(kept.pre.left_out.len()).unwrap_or(u32::MAX),
            ms: kept.pre.ms,
        });
        end.add_left_out(&path, &kept.pre.left_out);
        if let Some(reason) = kept.mirror.as_ref().and_then(|mirror| mirror.missing) {
            end.not_covered.push((path.clone(), format!("{NOT_COVERED_REFS}{reason}")));
        }
        let post = match self.snap(&path, &limits, true).await {
            Ok(post) => post,
            Err(missed) => {
                let reason = match missed {
                    Missed::TooLarge => LeftOutReason::RootTooLarge,
                    Missed::Failed => {
                        end.not_covered.push((path, NOT_COVERED_FAILED.to_owned()));
                        return end;
                    }
                };
                end.left_out.push((path.clone(), reason));
                end.not_covered.push((path, NOT_COVERED_FAILED.to_owned()));
                return end;
            }
        };
        end.add_left_out(&path, &post.left_out);
        let changes = if post.tree == kept.pre.tree {
            Some(Vec::new())
        } else {
            self.compare(&path, &kept.pre.tree, &post.tree).await
        };
        let hidden = match &changes {
            Some(changes) if !changes.is_empty() => self.ignored(&path, changes).await,
            _ => Vec::new(),
        };
        if let Some(changes) = &changes
            && !changes.is_empty()
        {
            let shown = root.shown().to_owned();
            end.changes = Some(Shown { root: path.clone(), shown, changes: changes.clone() });
        }
        if !self.pre_holds(conversation, turn, &path, &kept.commit).await {
            end.not_covered.push((path, NOT_COVERED_STORE_CHANGED.to_owned()));
            return end;
        }
        let mut meta = kept.meta.clone();
        meta.phase = "post".to_owned();
        meta.hidden = hidden;
        meta.files = post.files;
        meta.failed = kept.failed;
        meta.set_left_out(&post.left_out);
        meta.set_modes(&post.modes);
        set_touched(&mut meta, &kept.touched);
        if let Some((dirs, pins)) = kept.mirror.as_ref().and_then(|mirror| mirror.git.as_ref()) {
            match self.refs_at_end(dirs, *pins).await {
                Ok(refs) => {
                    meta.head = refs.head_ref.clone().or_else(|| refs.head_commit.clone());
                    meta.refs = refs_with_head(&refs);
                }
                Err(reason) => {
                    meta.head = None;
                    meta.refs.clear();
                    let already =
                        kept.mirror.as_ref().is_some_and(|mirror| mirror.missing.is_some());
                    if !already {
                        end.not_covered.push((path.clone(), format!("{NOT_COVERED_REFS}{reason}")));
                    }
                }
            }
        }
        match self.keep_post(conversation, turn, &path, &post.tree, &kept.commit, meta).await {
            Ok(commit) => {
                end.snapshots.push(SnapshotInfo {
                    root: path.clone(),
                    phase: Phase::Post,
                    commit,
                    files: post.files,
                    skipped: u32::try_from(post.left_out.len()).unwrap_or(u32::MAX),
                    ms: post.ms,
                });
                if kept.failed {
                    end.not_covered.push((path.clone(), NOT_COVERED_FAILED.to_owned()));
                } else {
                    end.kept = true;
                }
            }
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %path.display(), "the snapshots of a turn could not be kept");
                end.not_covered.push((path, NOT_COVERED_FAILED.to_owned()));
            }
        }
        end
    }

    /// Writes the commit of the turn's first snapshot `snap` of `root` and the ref
    /// `pre`, which must not exist yet. The `efr-meta:` line holds the root, the prefix
    /// of its paths, the project's `HEAD`, the mode, the root's `(dev, ino)`, what the
    /// snapshot left out and the modes.
    async fn write_pre(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        root: &Root,
        snap: &Snap,
        limits: &Limits,
    ) -> Result<(String, Meta, Option<Mirror>), SnapshotError> {
        let path = root.path().to_path_buf();
        let store = Store::new(self.dir(), &path);
        let slot = self.slot(&path);
        let _turn = slot.enter().await;
        let now = self.runner().clock().now();
        let time = format!("{} +0000", now.as_second());
        let looked = path.clone();
        let (head, dev_ino) = self
            .runner()
            .blocking(&path, move || {
                let dev_ino = std::fs::metadata(&looked)
                    .map(|metadata| (metadata.dev(), metadata.ino()))
                    .map_err(|source| SnapshotError::Io { path: looked.clone(), source });
                (store::project_head(&looked), dev_ino)
            })
            .await?;
        let mut meta = Meta {
            conversation: conversation.to_string(),
            turn: turn.to_string(),
            phase: "pre".to_owned(),
            root: path.to_string_lossy().into_owned(),
            shown: root.shown().to_owned(),
            head,
            at: now.to_string(),
            mode: root.mode().map(str::to_owned),
            dev_ino: Some(dev_ino?),
            files: snap.files,
            ..Meta::default()
        };
        meta.set_left_out(&snap.left_out);
        meta.set_modes(&snap.modes);
        let mirror = self.take_mirror(conversation, turn, root, &store, limits).await;
        if let Some(mirror) = &mirror {
            meta.mirror = Some(mirror.missing.unwrap_or(MIRROR_TAKEN).to_owned());
            if let Some((dirs, pins)) = &mirror.git {
                meta.git = Some(GitMeta {
                    git_dir: dirs.git_dir.to_string_lossy().into_owned(),
                    common_dir: dirs.common_dir.to_string_lossy().into_owned(),
                    git_dir_dev_ino: pins.git_dir,
                    common_dir_dev_ino: pins.common_dir,
                });
                meta.head =
                    mirror.refs.head_ref.clone().or_else(|| mirror.refs.head_commit.clone());
                meta.refs = refs_with_head(&mirror.refs);
            }
        }
        let commit = self.commit(&store, &snap.tree, None, meta.message(), &time).await?;
        let name = turn_ref(conversation, turn, "pre");
        let update = format!("create {name} {commit}\n");
        self.runner()
            .checked(
                &store,
                "update-ref",
                &["update-ref", "--stdin"],
                Run { stdin: Some(update.into_bytes()), ..Run::default() },
            )
            .await?;
        Ok((commit, meta, mirror))
    }

    /// The project's refs of `root` at the end of a turn, from the git dirs that its
    /// first snapshot pinned; `Err` with why not.
    async fn refs_at_end(&self, dirs: &GitDirs, pins: Pins) -> Result<ProjectRefs, &'static str> {
        let runner = self.runner().clone();
        let looked = dirs.clone();
        let opened = self
            .runner()
            .blocking(&dirs.git_dir, move || ProjectGit::open(&runner, &looked, Some(pins)))
            .await;
        let project = match opened {
            Ok(Ok(project)) => project,
            Ok(Err(refusal)) => return Err(refusal.reason()),
            Err(_) => return Err(MIRROR_FETCH_FAILED),
        };
        project.refs().await.map_err(|_| MIRROR_FETCH_FAILED)
    }

    /// Points the turn's `last` ref of `root` at a commit of `tree` with `meta`.
    async fn move_last(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        root: &Path,
        tree: &str,
        meta: Meta,
    ) -> Result<(), SnapshotError> {
        let store = Store::new(self.dir(), root);
        let slot = self.slot(root);
        let _turn = slot.enter().await;
        let time = format!("{} +0000", self.runner().clock().now().as_second());
        let commit = self.commit(&store, tree, None, meta.message(), &time).await?;
        let name = turn_ref(conversation, turn, "last");
        self.runner()
            .checked(
                &store,
                "update-ref",
                &["update-ref", name.as_str(), commit.as_str()],
                Run::default(),
            )
            .await?;
        Ok(())
    }

    /// Writes the turn's `post` of `root`, a commit of `tree` whose parent is `pre`,
    /// and removes its `last`.
    async fn keep_post(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        root: &Path,
        tree: &str,
        pre: &str,
        meta: Meta,
    ) -> Result<String, SnapshotError> {
        let store = Store::new(self.dir(), root);
        let slot = self.slot(root);
        let _turn = slot.enter().await;
        let time = format!("{} +0000", self.runner().clock().now().as_second());
        let commit = self.commit(&store, tree, Some(pre), meta.message(), &time).await?;
        let name = turn_ref(conversation, turn, "post");
        // NOTE: `post` holds all that `last` held, so `last` goes in the same step.
        let last = turn_ref(conversation, turn, "last");
        let update = format!("create {name} {commit}\ndelete {last}\n");
        self.runner()
            .checked(
                &store,
                "update-ref",
                &["update-ref", "--stdin"],
                Run { stdin: Some(update.into_bytes()), ..Run::default() },
            )
            .await?;
        Ok(commit)
    }

    /// True when the turn's `pre` ref of `root` still names `commit`.
    async fn pre_holds(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        root: &Path,
        commit: &str,
    ) -> bool {
        let store = Store::new(self.dir(), root);
        let name = turn_ref(conversation, turn, "pre");
        let found = self
            .runner()
            .run(&store, &["rev-parse", "--verify", "--quiet", name.as_str()], Run::default())
            .await;
        matches!(found, Ok(output) if output.success && String::from_utf8_lossy(&output.stdout).trim() == commit)
    }
}

/// What the end of a turn found in one root.
#[derive(Debug, Default)]
struct RootEnd {
    changes: Option<Shown>,
    snapshots: Vec<SnapshotInfo>,
    left_out: Vec<(PathBuf, LeftOutReason)>,
    not_covered: Vec<(PathBuf, String)>,
    kept: bool,
}

impl RootEnd {
    fn add_left_out(&mut self, root: &Path, left_out: &[LeftOut]) {
        for left in left_out {
            let path =
                if left.path.is_empty() { root.to_path_buf() } else { root.join(&left.path) };
            let entry = (path, left.reason);
            if !self.left_out.contains(&entry) {
                self.left_out.push(entry);
            }
        }
    }
}

/// The refs of `refs`, with `HEAD` and the commit it names.
fn refs_with_head(refs: &ProjectRefs) -> BTreeMap<String, String> {
    let mut all = refs.refs.clone();
    let head = refs
        .head_commit
        .clone()
        .or_else(|| refs.head_ref.as_ref().and_then(|name| refs.refs.get(name).cloned()));
    if let Some(head) = head {
        all.insert("HEAD".to_owned(), head);
    }
    all
}

/// Sets the `touched` paths of `meta`, cut at [`MAX_TOUCHED`].
fn set_touched(meta: &mut Meta, touched: &BTreeSet<String>) {
    meta.touched = touched.iter().take(MAX_TOUCHED).cloned().collect();
    meta.touched_more = touched.len() > MAX_TOUCHED;
}
