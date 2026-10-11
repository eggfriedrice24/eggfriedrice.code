//! Undo and redo of a turn, planned on the store's trees: [`Snapshots::plan_undo`] moves
//! each root from the state after the turn back to `pre`, and [`Snapshots::plan_redo`]
//! moves it from the state after an undo back to the state before it. Both act only on
//! the paths that the turn's calls changed (`touched`), or that the undo wrote, so a
//! change between two calls stays.
//!
//! Planning is pure: it reads refs, trees and the files of the roots, and writes
//! nothing. The daemon applies a plan. Before it applies, [`Snapshots::capture_before_undo`]
//! takes the "before" snapshot of each root that undo writes, puts the bytes of each
//! file that undo overwrites or removes into the store (for the undo trash), and writes
//! the journal ref `started`. [`Snapshots::finish_undo`] takes the "after" snapshot and
//! marks the turn `undone` (or, for a redo, not undone). An undo with `started` and no
//! end resumes at the next [`Snapshots::plan_undo`] of its turn: a path that already
//! has its target state is skipped.
//!
//! Refs, per conversation and undo id:
//!
//! ```text
//! refs/efr/<conversation>/<turn>/undone           marker: the turn is undone (its line names the undo)
//! refs/efr-undo/<conversation>/<undo>/before       the root before the undo
//! refs/efr-undo/<conversation>/<undo>/started      the journal: the undo began to write
//! refs/efr-undo/<conversation>/<undo>/after        the root after the undo
//! refs/efr-undo/<conversation>/<undo>/redone       marker: a redo took the undo back
//! refs/efr-undo/<conversation>/<undo>/mirror/...   the project's refs that the undo moves
//! refs/efr-undo/<conversation>/<undo>/home         the home paths that the undo changes
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use efr_protocol::{ConversationId, TurnId, UndoReason};

use crate::SnapshotError;
use crate::capture::{self, LeftOutReason, Limits};
use crate::cover::recorded;
use crate::home::HomePlan;
use crate::meta::{MAX_TOUCHED, Meta};
use crate::object::ObjectId;
use crate::project_git::{GitDirs, Pins};
use crate::runner::Run;
use crate::snapshots::{Root, Snapshots};
use crate::store::Store;
use crate::turn::turn_ref;

mod current;
mod decide;
mod held;
mod refs;

pub(crate) use current::Current;
pub(crate) use held::{CommitInfo, Held, TurnHas, UndoHas, undo_ref, valid_undo_id};

use decide::{Facts, Side};

/// What undo expects a path to be just before it writes it. The applier reads the path
/// again and writes or removes it only when it still is this.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Expect {
    /// The path does not exist.
    Missing,
    /// A regular file with this blob id ([`blob_id`](crate::blob_id)).
    Blob(ObjectId),
    /// A symbolic link to this target.
    Link(Vec<u8>),
    /// No fact says what the path is (a directory, a special file, a parent that is a
    /// link, a failed read): undo never writes or removes it, also with `force`.
    Unknown,
    /// Only for a home path with `force`, whose call did not finish: no copy says what
    /// it was after the turn, so the applier takes what it finds. Its bytes come from
    /// the copy that [`Snapshots::home_undo_before`] takes.
    Any,
}

/// What undo does with one path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlanAction {
    /// Writes the blob `blob` of the store. `mode` (`& 0o777`) is for a file that
    /// undo makes again: the mode that the snapshots recorded, else 0600 for an ignored
    /// file, else git's 0644 or 0755.
    Write {
        /// The blob.
        blob: ObjectId,
        /// The mode of a new file.
        mode: u32,
    },
    /// Makes a symbolic link to `target`.
    Link {
        /// The link's target.
        target: Vec<u8>,
    },
    /// Removes the path (to the undo trash).
    Delete,
    /// Leaves the path as it is, for this reason.
    Skip(UndoReason),
    /// No snapshot holds the path, for this reason; undo never touches it.
    NotCovered(LeftOutReason),
}

/// One path of a root in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    /// The path relative to the root.
    pub path: String,
    /// What undo does.
    pub action: PlanAction,
    /// What the path must be just before undo writes it.
    pub expect: Expect,
    /// For each write over an existing path and each removal: the blob of what is there
    /// now and its mode (`0o100000 | mode & 0o777` for a file, `0o120000` for a link),
    /// which go to the undo trash first. [`Snapshots::capture_before_undo`] puts the
    /// blob into the store.
    pub trash: Option<(ObjectId, u32)>,
    /// What `force` overrides for this path: the change after the turn, the later turn,
    /// the missing record after the turn, or the replaced root.
    pub forced: Option<UndoReason>,
}

impl PlanEntry {
    /// True when undo writes or removes the path.
    pub fn acts(&self) -> bool {
        matches!(
            self.action,
            PlanAction::Write { .. } | PlanAction::Link { .. } | PlanAction::Delete
        )
    }
}

/// One ref of a project that undo moves, by compare and swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefPlan {
    /// The ref, such as `refs/heads/main`, or `HEAD`.
    pub name: String,
    /// The object id that undo gives it; `None` deletes it.
    pub to: Option<ObjectId>,
    /// The object id that it must still have; `None`: it must be absent.
    pub expect: Option<ObjectId>,
    /// For `HEAD`: the branch that undo makes it name again.
    pub symbolic: Option<String>,
    /// For `HEAD`: the branch that it must still name; `None` with `expect`: detached.
    pub expect_symbolic: Option<String>,
    /// True when the store no longer holds the objects of `to`.
    pub objects_missing: bool,
    /// Why undo leaves the ref as it is; `None` when it moves it.
    pub reason: Option<UndoReason>,
    pub(crate) pins: Option<Pins>,
}

/// One root in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootPlan {
    /// The root's canonical path.
    pub root: PathBuf,
    /// The prefix of its paths, as [`Root::shown`] says.
    pub shown: String,
    /// The root's `(dev, ino)` when the turn's first snapshot took it.
    pub dev_ino: (u64, u64),
    /// Each path that differs between the trees, sorted.
    pub entries: Vec<PlanEntry>,
    /// The project's refs that differ.
    pub refs: Vec<RefPlan>,
    /// True when the turn's calls changed more paths than its record names; undo acts
    /// on the paths beyond the record only with `force`.
    pub touched_more: bool,
    pub(crate) git: Option<(GitDirs, Pins)>,
}

/// The refs that the stores hold of one turn of a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRefs {
    /// The turn.
    pub turn: TurnId,
    /// The roots whose stores hold the turn's `pre`.
    pub roots: Vec<PathBuf>,
    /// True when each of them also holds `post`: the turn ended in efrd.
    pub has_post: bool,
    /// True when an undo undid the turn and no redo took it back.
    pub undone: bool,
    /// True when the home store holds copies of the turn.
    pub home: bool,
    /// An undo of the turn that began to write and did not end: the next undo of the
    /// turn resumes it.
    pub started_undo: Option<String>,
}

/// What an undo or a redo of one turn does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoPlan {
    /// The undo's id: the name of its refs and of its undo trash directory.
    pub undo_id: String,
    /// The conversation.
    pub conversation: ConversationId,
    /// The turn that undo undoes, or that redo brings back.
    pub turn: TurnId,
    /// True for a redo.
    pub redo: bool,
    /// True when the plan continues (undo) or takes back (redo) an undo that stopped
    /// half way.
    pub resumed: bool,
    /// Each root that the turn's snapshots hold.
    pub roots: Vec<RootPlan>,
    /// The home paths.
    pub home: HomePlan,
}

/// The newest turn after `turn` with a kept snapshot that no undo undid: undo of
/// `turn` waits for it (undo is a stack), unless `force`. `turns` comes from
/// [`Snapshots::turns_with_refs`].
pub fn stack_blocker(turns: &[TurnRefs], turn: TurnId) -> Option<TurnId> {
    turns
        .iter()
        .filter(|later| later.turn > turn && !later.undone)
        .filter(|later| !later.roots.is_empty() || later.home)
        .map(|later| later.turn)
        .max()
}

/// The stores that hold refs of one conversation.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub(crate) roots: Vec<(Store, Held)>,
    pub(crate) home: Option<(Store, Held)>,
}

impl Scan {
    fn all(&self) -> impl Iterator<Item = &(Store, Held)> {
        self.roots.iter().chain(self.home.iter())
    }
}

/// A path's state in a tree listing (`ls-tree -r`): git's mode and the object id.
pub(crate) type TreeMap = BTreeMap<String, (u32, String)>;

impl Snapshots {
    /// Every turn of `conversation` whose snapshots a store holds, newest first.
    pub async fn turns_with_refs(&self, conversation: ConversationId) -> Vec<TurnRefs> {
        match self.scan(conversation).await {
            Ok(scan) => self.turns_of(&scan).await,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), "the snapshots of a conversation could not be listed");
                Vec::new()
            }
        }
    }

    /// Plans the undo of `turn` of `conversation` on each root whose store holds the
    /// turn's `pre` (`roots` gives the git dirs and the shown prefix of the known
    /// ones), and on the turn's home copies. Pure: reads refs, trees and files, writes
    /// nothing. An undo of the turn that stopped half way is resumed: the plan keeps
    /// its id and `resumed` is true. A turn with no snapshot gives a plan with no root.
    pub async fn plan_undo(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        roots: &[Root],
        force: bool,
        undo_id: &str,
        _limits: Limits,
    ) -> Result<UndoPlan, SnapshotError> {
        let scan = self.scan(conversation).await?;
        let open = self.open_undos(&scan).await;
        let (undo_id, resumed) = match open.iter().find(|(_, of)| *of == turn) {
            Some((undo, _)) => (undo.clone(), true),
            None if valid_undo_id(undo_id) => (undo_id.to_owned(), false),
            None => return Err(SnapshotError::InvalidUndoId { id: undo_id.to_owned() }),
        };
        let known = self.canonical(roots.to_vec()).await;
        let mut planned = Vec::new();
        for (store, held) in &scan.roots {
            if held.turns.get(&turn).is_none_or(|has| has.pre.is_none()) {
                continue;
            }
            let root = known.iter().find(|root| root.path() == store.root());
            planned.push(self.plan_root_undo(store, held, turn, root, force).await?);
        }
        let home = self.plan_home_undo(conversation, turn, &scan, &planned, force).await?;
        Ok(UndoPlan { undo_id, conversation, turn, redo: false, resumed, roots: planned, home })
    }

    /// Plans the redo of the newest undo of `conversation`: an undo that stopped half
    /// way, else the newest undo that no redo took back. It moves each path that the
    /// undo wrote from the state after the undo back to the state before it.
    /// [`SnapshotError::NoRedo`] when there is none.
    pub async fn plan_redo(
        &self,
        conversation: ConversationId,
        roots: &[Root],
        force: bool,
        _limits: Limits,
    ) -> Result<UndoPlan, SnapshotError> {
        let scan = self.scan(conversation).await?;
        let open = self.open_undos(&scan).await;
        let (undo_id, turn, finished) = match open.into_iter().max() {
            Some((undo, turn)) => (undo, turn, false),
            None => match self.done_undos(&scan).await.into_iter().max() {
                Some((undo, turn)) => (undo, turn, true),
                None => return Err(SnapshotError::NoRedo),
            },
        };
        let known = self.canonical(roots.to_vec()).await;
        let mut planned = Vec::new();
        for (store, held) in &scan.roots {
            let Some(undo) = held.undos.get(&undo_id) else { continue };
            let Some(before) = undo.before.as_deref() else { continue };
            let expected = if finished {
                undo.after.clone()
            } else {
                held.turns.get(&turn).and_then(|has| has.pre.clone())
            };
            let Some(expected) = expected else { continue };
            let root = known.iter().find(|root| root.path() == store.root());
            let plan = self.plan_root_redo(store, before, &expected, root, force).await?;
            planned.push(plan);
        }
        let home = self.plan_home_redo(conversation, &undo_id, turn, &scan, &planned).await?;
        Ok(UndoPlan {
            undo_id,
            conversation,
            turn,
            redo: true,
            resumed: !finished,
            roots: planned,
            home,
        })
    }

    /// Prepares the apply of `plan`. For an undo: the "before" snapshot of each root
    /// that it writes (`refs/efr-undo/<conversation>/<undo>/before`), the project's refs
    /// that it moves, then the journal ref `started`. For every plan: the bytes of each
    /// path that goes to the undo trash, into the store. A resumed undo keeps its first
    /// "before".
    pub async fn capture_before_undo(
        &self,
        plan: &UndoPlan,
        limits: Limits,
    ) -> Result<(), SnapshotError> {
        for root in &plan.roots {
            let store = Store::new(self.dir(), &root.root);
            let acting: Vec<String> =
                root.entries.iter().filter(|entry| entry.acts()).map(|e| e.path.clone()).collect();
            let moving = root.refs.iter().any(|plan| plan.reason.is_none());
            if acting.is_empty() && !moving {
                continue;
            }
            self.keep_trash(&store, &root.root, &root.entries).await?;
            if plan.redo || plan.resumed {
                continue;
            }
            let before = self.undo_before(plan, root, &store, &acting, &limits).await?;
            self.mirror_moved_refs(plan, root, &store).await?;
            let started = undo_ref(plan.conversation, &plan.undo_id, "started");
            self.update_refs(&store, format!("create {started} {before}\n")).await?;
        }
        if !plan.redo && !plan.resumed && plan.home.steps.iter().any(|step| step.acts()) {
            self.home_started(plan).await?;
        }
        Ok(())
    }

    /// Ends the apply of `plan`. For an undo: the "after" snapshot of each root that it
    /// wrote, the marker `undone` of the turn in each store that holds it, and the end
    /// of the journal. For a redo: the turn's `undone` markers go, and `redone` marks
    /// the undo as taken back.
    pub async fn finish_undo(&self, plan: &UndoPlan, limits: Limits) -> Result<(), SnapshotError> {
        let scan = self.scan(plan.conversation).await?;
        for (store, held) in &scan.roots {
            let turn = held.turns.get(&plan.turn);
            let undo = held.undos.get(&plan.undo_id);
            if turn.is_none() && undo.is_none() {
                continue;
            }
            if plan.redo {
                self.end_redo(plan, store, turn, undo).await?;
            } else {
                let root = plan.roots.iter().find(|root| root.root == store.root());
                self.end_undo(plan, store, root, undo, &limits).await?;
            }
        }
        if let Some((store, held)) = &scan.home {
            let turn = held.turns.get(&plan.turn);
            let undo = held.undos.get(&plan.undo_id);
            if turn.is_some_and(|has| !has.home.is_empty()) || undo.is_some() {
                if plan.redo {
                    self.end_redo(plan, store, turn, undo).await?;
                } else {
                    self.end_undo(plan, store, None, undo, &limits).await?;
                }
            }
        }
        Ok(())
    }

    /// The bytes of the blob `id` in the store of the canonical `root`.
    pub async fn blob(&self, root: &Path, id: &ObjectId) -> Result<Vec<u8>, SnapshotError> {
        let store = Store::new(self.dir(), root);
        self.blob_in(&store, id).await
    }

    pub(crate) async fn blob_in(
        &self,
        store: &Store,
        id: &ObjectId,
    ) -> Result<Vec<u8>, SnapshotError> {
        self.runner()
            .checked(store, "cat-file", &["cat-file", "blob", id.as_str()], Run::default())
            .await
    }

    /// The root stores and the home store with refs of `conversation`.
    pub(crate) async fn scan(&self, conversation: ConversationId) -> Result<Scan, SnapshotError> {
        let roots = self.held_roots(conversation).await?;
        let home = self.home_store();
        let held = self.held_in(&home, conversation).await?;
        let home = (!held.turns.is_empty() || !held.undos.is_empty()).then_some((home, held));
        Ok(Scan { roots, home })
    }

    async fn turns_of(&self, scan: &Scan) -> Vec<TurnRefs> {
        let mut turns: BTreeMap<TurnId, TurnRefs> = BTreeMap::new();
        for (store, held) in &scan.roots {
            for (turn, has) in &held.turns {
                let refs = refs_of(&mut turns, *turn);
                if has.pre.is_some() {
                    refs.roots.push(store.root().to_path_buf());
                    refs.has_post &= has.post.is_some();
                }
                refs.undone |= has.undone.is_some();
            }
        }
        if let Some((_, held)) = &scan.home {
            for (turn, has) in &held.turns {
                let refs = refs_of(&mut turns, *turn);
                refs.home |= !has.home.is_empty();
                refs.undone |= has.undone.is_some();
            }
        }
        for (undo, turn) in self.open_undos(scan).await {
            if let Some(refs) = turns.get_mut(&turn) {
                refs.started_undo = Some(undo);
            }
        }
        let mut all: Vec<TurnRefs> =
            turns.into_values().filter(|refs| !refs.roots.is_empty() || refs.home).collect();
        all.sort_by_key(|refs| std::cmp::Reverse(refs.turn));
        all
    }

    /// The undos that began to write and did not end, with their turn.
    pub(crate) async fn open_undos(&self, scan: &Scan) -> Vec<(String, TurnId)> {
        let mut open: BTreeMap<String, TurnId> = BTreeMap::new();
        for (store, held) in scan.all() {
            for (undo, has) in &held.undos {
                let Some(started) = has.started.as_deref() else { continue };
                if has.after.is_some() || has.redone.is_some() || open.contains_key(undo) {
                    continue;
                }
                if let Some(turn) = self.turn_of(store, started).await {
                    open.insert(undo.clone(), turn);
                }
            }
        }
        open.into_iter().collect()
    }

    /// The undos that ended and that no redo took back: each `undone` marker names one.
    async fn done_undos(&self, scan: &Scan) -> Vec<(String, TurnId)> {
        let mut done: BTreeMap<String, TurnId> = BTreeMap::new();
        for (store, held) in scan.all() {
            for (turn, has) in &held.turns {
                let Some(marker) = has.undone.as_deref() else { continue };
                let Ok(info) = self.commit_info(store, marker).await else { continue };
                if let Some(undo) = info.meta.undo {
                    done.insert(undo, *turn);
                }
            }
        }
        done.into_iter().collect()
    }

    /// The turn that the commit `commit` of an undo names.
    pub(crate) async fn turn_of(&self, store: &Store, commit: &str) -> Option<TurnId> {
        let info = self.commit_info(store, commit).await.ok()?;
        info.meta.turn.parse().ok()
    }

    async fn plan_root_undo(
        &self,
        store: &Store,
        held: &Held,
        turn: TurnId,
        root: Option<&Root>,
        force: bool,
    ) -> Result<RootPlan, SnapshotError> {
        let has = held.turns.get(&turn).cloned().unwrap_or_default();
        let Some(pre) = has.pre.as_deref() else {
            return Err(SnapshotError::BadOutput { command: "for-each-ref" });
        };
        let pre = self.commit_info(store, pre).await?;
        let source = match has.post.as_deref().or(has.last.as_deref()) {
            Some(source) => self.commit_info(store, source).await?,
            None => pre.clone(),
        };
        let mut later = BTreeSet::new();
        for other_has in held.turns.range(turn..).skip(1).map(|(_, has)| has) {
            if other_has.undone.is_some() || other_has.pre.is_none() {
                continue;
            }
            let newest = other_has.post.as_deref().or(other_has.last.as_deref());
            if let Some(newest) = newest
                && let Ok(info) = self.commit_info(store, newest).await
            {
                later.extend(info.meta.touched);
            }
        }
        let touched: BTreeSet<String> = source.meta.touched.iter().cloned().collect();
        let maybe = if source.meta.failed {
            Some(UndoReason::SnapshotFailed)
        } else if source.meta.touched_more {
            Some(UndoReason::OutsideTurnCalls)
        } else {
            None
        };
        let frame = Frame {
            store,
            target: &pre,
            source: &source,
            has_post: has.post.is_some(),
            touched,
            only_touched: false,
            maybe,
            later,
            force,
        };
        let entries = self.plan_paths(&frame).await?;
        let mut plan = self.root_plan(store, &pre, root, entries);
        plan.touched_more = source.meta.touched_more;
        plan.refs = self.plan_refs(&plan, &pre.meta, &source.meta, has.post.is_some(), force).await;
        Ok(plan)
    }

    async fn plan_root_redo(
        &self,
        store: &Store,
        before: &str,
        expected: &str,
        root: Option<&Root>,
        force: bool,
    ) -> Result<RootPlan, SnapshotError> {
        let before = self.commit_info(store, before).await?;
        let expected = self.commit_info(store, expected).await?;
        let frame = Frame {
            store,
            target: &before,
            source: &expected,
            has_post: true,
            touched: before.meta.touched.iter().cloned().collect(),
            only_touched: true,
            maybe: None,
            later: BTreeSet::new(),
            force,
        };
        let entries = self.plan_paths(&frame).await?;
        let mut plan = self.root_plan(store, &before, root, entries);
        plan.refs = self.plan_refs(&plan, &before.meta, &expected.meta, true, force).await;
        Ok(plan)
    }

    fn root_plan(
        &self,
        store: &Store,
        target: &CommitInfo,
        root: Option<&Root>,
        entries: Vec<PlanEntry>,
    ) -> RootPlan {
        let git = target.meta.git.as_ref().map(|git| {
            (
                GitDirs {
                    git_dir: PathBuf::from(&git.git_dir),
                    common_dir: PathBuf::from(&git.common_dir),
                },
                Pins { git_dir: git.git_dir_dev_ino, common_dir: git.common_dir_dev_ino },
            )
        });
        RootPlan {
            root: store.root().to_path_buf(),
            shown: root.map_or_else(|| target.meta.shown.clone(), |root| root.shown().to_owned()),
            dev_ino: target.meta.dev_ino.unwrap_or_default(),
            entries,
            refs: Vec::new(),
            touched_more: false,
            git,
        }
    }

    /// The entries of the paths of `frame`, sorted.
    async fn plan_paths(&self, frame: &Frame<'_>) -> Result<Vec<PlanEntry>, SnapshotError> {
        let store = frame.store;
        let target_map = self.tree_map(store, &frame.target.tree).await?;
        let source_map = if frame.source.tree == frame.target.tree {
            target_map.clone()
        } else {
            self.tree_map(store, &frame.source.tree).await?
        };
        let paths: Vec<String> = if frame.only_touched {
            frame
                .touched
                .iter()
                .filter(|path| target_map.get(*path) != source_map.get(*path))
                .cloned()
                .collect()
        } else {
            let all: BTreeSet<&String> = target_map.keys().chain(source_map.keys()).collect();
            all.into_iter()
                .filter(|path| target_map.get(*path) != source_map.get(*path))
                .cloned()
                .collect()
        };
        let root = store.root().to_path_buf();
        let wanted = paths.clone();
        let base = root.clone();
        let expected_root = frame.target.meta.dev_ino;
        let (currents, replaced) = self
            .runner()
            .blocking(&root, move || {
                let currents: Vec<Current> =
                    wanted.iter().map(|path| current::read(&base, path)).collect();
                let now = current::dev_ino(&base);
                (currents, expected_root.is_some_and(|expected| now != Some(expected)))
            })
            .await?;
        let mut entries = Vec::with_capacity(paths.len());
        for (path, current) in paths.iter().zip(&currents) {
            let target = side(&target_map, &frame.target.meta, path);
            let source = side(&source_map, &frame.source.meta, path);
            let link = match &target {
                Side::Link { id } => Some(self.blob_in(store, id).await?),
                _ => None,
            };
            let facts = Facts {
                touched: frame.touched.contains(path),
                maybe_touched: frame.maybe,
                has_post: frame.has_post,
                replaced,
                later: frame.later.contains(path),
                force: frame.force,
            };
            entries.push(decide::decide(path, &target, &source, current, link.as_deref(), facts));
        }
        self.set_new_modes(store, &root, &frame.target.meta, &mut entries, &currents).await;
        Ok(entries)
    }

    /// Gives each file that undo makes again the mode that a snapshot recorded, else
    /// 0600 when the ignore rules name it (a secret such as `.env`), else git's mode.
    async fn set_new_modes(
        &self,
        store: &Store,
        root: &Path,
        meta: &Meta,
        entries: &mut [PlanEntry],
        currents: &[Current],
    ) {
        let mut unrecorded = Vec::new();
        for (entry, current) in entries.iter_mut().zip(currents) {
            let PlanAction::Write { mode, .. } = &mut entry.action else { continue };
            if *current != Current::Missing {
                continue;
            }
            match meta.modes.get(&entry.path) {
                Some(recorded) => *mode = recorded & 0o777,
                None => unrecorded.push(entry.path.clone()),
            }
        }
        if unrecorded.is_empty() {
            return;
        }
        let ignored = match capture::split_ignored(self.runner(), store, root, unrecorded).await {
            Ok((ignored, _)) => ignored,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %root.display(), "the ignored files of an undo could not be found");
                return;
            }
        };
        for entry in entries.iter_mut() {
            if let PlanAction::Write { mode, .. } = &mut entry.action
                && ignored.contains(&entry.path)
            {
                *mode = 0o600;
            }
        }
    }

    /// The blobs, links and gitlinks of `tree` in `store`: path to git's mode and id.
    pub(crate) async fn tree_map(
        &self,
        store: &Store,
        tree: &str,
    ) -> Result<TreeMap, SnapshotError> {
        let out = self
            .runner()
            .checked(
                store,
                "ls-tree",
                &["ls-tree", "-r", "-z", "--full-tree", tree],
                Run::default(),
            )
            .await?;
        let mut map = TreeMap::new();
        for record in out.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
            let Ok(text) = std::str::from_utf8(record) else { continue };
            let Some((head, path)) = text.split_once('\t') else { continue };
            let mut parts = head.split(' ');
            let (Some(mode), Some(_), Some(id)) = (parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            let Ok(mode) = u32::from_str_radix(mode, 8) else { continue };
            map.insert(path.to_owned(), (mode, id.to_owned()));
        }
        Ok(map)
    }

    /// Puts the bytes of each path of `entries` that goes to the undo trash into
    /// `store`, read from the root when the store lacks them.
    async fn keep_trash(
        &self,
        store: &Store,
        root: &Path,
        entries: &[PlanEntry],
    ) -> Result<(), SnapshotError> {
        let wanted: Vec<(String, ObjectId)> = entries
            .iter()
            .filter(|entry| entry.acts())
            .filter_map(|entry| {
                entry.trash.as_ref().map(|(id, _)| (entry.path.clone(), id.clone()))
            })
            .collect();
        if wanted.is_empty() {
            return Ok(());
        }
        let ids: Vec<ObjectId> = wanted.iter().map(|(_, id)| id.clone()).collect();
        let missing = self.missing_objects(store, &ids).await?;
        for (path, id) in wanted.into_iter().filter(|(_, id)| missing.contains(id)) {
            let base = root.to_path_buf();
            let looked = id.clone();
            let bytes = self
                .runner()
                .blocking(root, move || current::bytes_of(&base, &path, &looked))
                .await?;
            // NOTE: a path that changed since the plan fails the applier's own check,
            // so it is never written and needs no trash copy.
            let Some(bytes) = bytes else { continue };
            self.runner()
                .checked(
                    store,
                    "hash-object",
                    &["hash-object", "-w", "--stdin"],
                    Run { stdin: Some(bytes), ..Run::default() },
                )
                .await?;
        }
        Ok(())
    }

    /// The ids of `ids` that `store` does not hold.
    pub(crate) async fn missing_objects(
        &self,
        store: &Store,
        ids: &[ObjectId],
    ) -> Result<BTreeSet<ObjectId>, SnapshotError> {
        if ids.is_empty() {
            return Ok(BTreeSet::new());
        }
        let input: String = ids.iter().map(|id| format!("{id}\n")).collect();
        let out = self
            .runner()
            .checked(
                store,
                "cat-file",
                &["cat-file", "--batch-check=%(objectname)"],
                Run { stdin: Some(input.into_bytes()), ..Run::default() },
            )
            .await?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|line| line.strip_suffix(" missing"))
            .filter_map(ObjectId::parse)
            .collect())
    }

    /// The "before" snapshot of `root` for `plan`, its commit and its ref.
    async fn undo_before(
        &self,
        plan: &UndoPlan,
        root: &RootPlan,
        store: &Store,
        acting: &[String],
        limits: &Limits,
    ) -> Result<String, SnapshotError> {
        let snap = self
            .snap(&root.root, limits, true)
            .await
            .map_err(|_| SnapshotError::GitFailed { command: "add", store: store.git_dir() })?;
        let mut meta = self.undo_meta(plan, root, "before");
        meta.files = snap.files;
        meta.set_left_out(&snap.left_out);
        meta.set_modes(&snap.modes);
        meta.touched = acting.iter().take(MAX_TOUCHED).cloned().collect();
        meta.refs = self.project_refs_now(root).await;
        let time = self.commit_time();
        let commit = self.commit_locked(store, &snap.tree, None, meta, &time).await?;
        let name = undo_ref(plan.conversation, &plan.undo_id, "before");
        self.update_refs(store, format!("create {name} {commit}\n")).await?;
        Ok(commit)
    }

    async fn end_undo(
        &self,
        plan: &UndoPlan,
        store: &Store,
        root: Option<&RootPlan>,
        undo: Option<&UndoHas>,
        limits: &Limits,
    ) -> Result<(), SnapshotError> {
        let mut commands = String::new();
        let mut parent = None;
        if let (Some(root), Some(before)) = (root, undo.and_then(|has| has.before.as_deref())) {
            let snap = self
                .snap(&root.root, limits, true)
                .await
                .map_err(|_| SnapshotError::GitFailed { command: "add", store: store.git_dir() })?;
            let info = self.commit_info(store, before).await?;
            let mut meta = self.undo_meta(plan, root, "after");
            meta.files = snap.files;
            meta.set_left_out(&snap.left_out);
            meta.set_modes(&snap.modes);
            meta.touched = info.meta.touched;
            meta.refs = self.project_refs_now(root).await;
            let time = self.commit_time();
            let after = self.commit_locked(store, &snap.tree, Some(before), meta, &time).await?;
            let name = undo_ref(plan.conversation, &plan.undo_id, "after");
            commands.push_str(&format!("update {name} {after}\n"));
            parent = Some(after);
        }
        let marker = self.marker(store, plan, "undone", parent.as_deref()).await?;
        let undone = turn_ref(plan.conversation, plan.turn, "undone");
        commands.push_str(&format!("update {undone} {marker}\n"));
        if let Some(started) = undo.and_then(|has| has.started.as_deref()) {
            let name = undo_ref(plan.conversation, &plan.undo_id, "started");
            commands.push_str(&format!("delete {name} {started}\n"));
        }
        self.update_refs(store, commands).await
    }

    async fn end_redo(
        &self,
        plan: &UndoPlan,
        store: &Store,
        turn: Option<&TurnHas>,
        undo: Option<&UndoHas>,
    ) -> Result<(), SnapshotError> {
        let mut commands = String::new();
        if let Some(undone) = turn.and_then(|has| has.undone.as_deref()) {
            let name = turn_ref(plan.conversation, plan.turn, "undone");
            commands.push_str(&format!("delete {name} {undone}\n"));
        }
        if let Some(undo) = undo {
            if let Some(started) = undo.started.as_deref() {
                let name = undo_ref(plan.conversation, &plan.undo_id, "started");
                commands.push_str(&format!("delete {name} {started}\n"));
            }
            let parent = undo.after.as_deref().or(undo.before.as_deref());
            let marker = self.marker(store, plan, "redone", parent).await?;
            let name = undo_ref(plan.conversation, &plan.undo_id, "redone");
            commands.push_str(&format!("update {name} {marker}\n"));
        }
        if commands.is_empty() {
            return Ok(());
        }
        self.update_refs(store, commands).await
    }

    /// The `efr-meta:` line of a commit of `plan` in the store of `root`.
    fn undo_meta(&self, plan: &UndoPlan, root: &RootPlan, phase: &str) -> Meta {
        Meta {
            conversation: plan.conversation.to_string(),
            turn: plan.turn.to_string(),
            phase: phase.to_owned(),
            root: root.root.to_string_lossy().into_owned(),
            shown: root.shown.clone(),
            at: self.runner().clock().now().to_string(),
            dev_ino: current::dev_ino(&root.root),
            undo: Some(plan.undo_id.clone()),
            ..Meta::default()
        }
    }

    /// A commit of the empty tree that marks a step of `plan` in `store`.
    pub(crate) async fn marker(
        &self,
        store: &Store,
        plan: &UndoPlan,
        phase: &str,
        parent: Option<&str>,
    ) -> Result<String, SnapshotError> {
        let meta = Meta {
            conversation: plan.conversation.to_string(),
            turn: plan.turn.to_string(),
            phase: phase.to_owned(),
            at: self.runner().clock().now().to_string(),
            undo: Some(plan.undo_id.clone()),
            ..Meta::default()
        };
        let tree = self.empty_tree(store).await?;
        let time = self.commit_time();
        self.commit_locked(store, &tree, parent, meta, &time).await
    }

    pub(crate) async fn empty_tree(&self, store: &Store) -> Result<String, SnapshotError> {
        let out = self
            .runner()
            .checked(
                store,
                "mktree",
                &["mktree"],
                Run { stdin: Some(Vec::new()), ..Run::default() },
            )
            .await?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    /// The time of a new commit, in git's `<seconds> +0000` form.
    pub(crate) fn commit_time(&self) -> String {
        format!("{} +0000", self.runner().clock().now().as_second())
    }

    /// A commit of `tree` with `meta`, under the gate of `store`.
    pub(crate) async fn commit_locked(
        &self,
        store: &Store,
        tree: &str,
        parent: Option<&str>,
        meta: Meta,
        time: &str,
    ) -> Result<String, SnapshotError> {
        let slot = self.slot(store.root());
        let _turn = slot.enter().await;
        self.commit(store, tree, parent, meta.message(), time).await
    }

    /// Runs `update-ref --stdin` with `commands` in `store`, under its gate.
    pub(crate) async fn update_refs(
        &self,
        store: &Store,
        commands: String,
    ) -> Result<(), SnapshotError> {
        let slot = self.slot(store.root());
        let _turn = slot.enter().await;
        self.runner()
            .checked(
                store,
                "update-ref",
                &["update-ref", "--stdin"],
                Run { stdin: Some(commands.into_bytes()), ..Run::default() },
            )
            .await?;
        Ok(())
    }
}

/// One root of a plan, as [`Snapshots::plan_paths`] reads it.
struct Frame<'a> {
    store: &'a Store,
    /// The commit whose tree undo moves the root to (`pre`, or `before` for a redo).
    target: &'a CommitInfo,
    /// The commit whose tree undo expects the root to have now.
    source: &'a CommitInfo,
    has_post: bool,
    touched: BTreeSet<String>,
    /// Plan only the touched paths (a redo).
    only_touched: bool,
    maybe: Option<UndoReason>,
    later: BTreeSet<String>,
    force: bool,
}

/// The entry of `turn` in `turns`, made when it is not there yet.
fn refs_of(turns: &mut BTreeMap<TurnId, TurnRefs>, turn: TurnId) -> &mut TurnRefs {
    turns.entry(turn).or_insert_with(|| TurnRefs {
        turn,
        roots: Vec::new(),
        has_post: true,
        undone: false,
        home: false,
        started_undo: None,
    })
}

/// The state of `path` in a tree listing `map` whose commit carries `meta`.
fn side(map: &TreeMap, meta: &Meta, path: &str) -> Side {
    if let Some((mode, id)) = map.get(path) {
        let Some(id) = ObjectId::parse(id) else { return Side::LeftOut(LeftOutReason::Unreadable) };
        return match mode {
            0o120_000 => Side::Link { id },
            0o160_000 => Side::LeftOut(LeftOutReason::NestedRepo),
            0o100_755 => Side::Blob { id, mode: 0o100_755 },
            _ => Side::Blob { id, mode: 0o100_644 },
        };
    }
    if let Some(reason) = recorded(&meta.left_out, path) {
        return Side::LeftOut(reason);
    }
    // NOTE: the record names at most `MAX_LEFT_OUT` paths; once it is cut, a path that
    // the tree lacks may be one that the snapshot left out, so undo never treats it as
    // missing.
    if meta.left_out_more > 0 {
        return Side::LeftOut(LeftOutReason::IgnoredCap);
    }
    Side::Missing
}

#[cfg(test)]
mod tests;
