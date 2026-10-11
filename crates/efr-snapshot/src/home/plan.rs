//! The home part of a plan: one step per home path of the turn, from the state before
//! the turn's first copy of the path to the state after its last copy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use efr_protocol::{ConversationId, TurnId, UndoReason};

use crate::SnapshotError;
use crate::home::{HomePlan, HomeState, HomeStep, Item, Kind, Manifest};
use crate::object::ObjectId;
use crate::snapshots::Snapshots;
use crate::store::Store;
use crate::undo::{Expect, RootPlan, Scan, TurnHas, UndoPlan, undo_ref};

/// What a path's last copy in a turn says about it after its call.
#[derive(Debug, Clone)]
enum After {
    /// The call did not finish: the copy has no "after".
    Unfinished,
    /// The "after" of the copy; `None` when it does not list the path.
    Done(Option<Item>),
}

/// One home path of a turn.
#[derive(Debug, Clone)]
struct PathCopies {
    first: Item,
    last: After,
    floor: bool,
}

impl Snapshots {
    /// The home steps of the undo of `turn`. A path below a root of `roots` belongs to
    /// the root's plan. A copy whose call did not finish leaves its paths as they are
    /// ([`UndoReason::CallUnfinished`]), unless `force`, which takes what it finds
    /// ([`Expect::Any`]).
    pub(crate) async fn plan_home_undo(
        &self,
        _conversation: ConversationId,
        turn: TurnId,
        scan: &Scan,
        roots: &[RootPlan],
        force: bool,
    ) -> Result<HomePlan, SnapshotError> {
        let Some((store, held)) = &scan.home else { return Ok(HomePlan::default()) };
        let Some(has) = held.turns.get(&turn) else { return Ok(HomePlan::default()) };
        let copies = self.turn_copies(store, has).await?;
        let mut steps = Vec::new();
        for (path, copied) in copies {
            if in_roots(&path, roots) {
                continue;
            }
            let (expect, after, unfinished) = match &copied.last {
                After::Unfinished if force => (Expect::Any, None, true),
                After::Unfinished => (Expect::Unknown, None, true),
                After::Done(None) => (Expect::Unknown, None, false),
                After::Done(Some(item)) => (self.expect_of(store, item).await?, Some(item), false),
            };
            if after.is_some_and(|after| after.same_state(&copied.first)) {
                continue;
            }
            let mut write = self.state_of(store, &copied.first).await?;
            if unfinished && !force {
                write = HomeState::NotCovered(UndoReason::CallUnfinished);
            } else if let Some(reason) = after.and_then(not_covered) {
                write = HomeState::NotCovered(reason);
            } else if expect == Expect::Unknown && !unfinished {
                write = HomeState::NotCovered(UndoReason::Unknown);
            }
            steps.push(HomeStep {
                path,
                blob: blob_of(&copied.first),
                trash: after.and_then(trash_of),
                write,
                expect,
                floor: copied.floor,
            });
        }
        Ok(HomePlan { steps })
    }

    /// The home steps of the redo of the undo `undo` of `turn`: each path that the
    /// undo's copy holds goes back to that copy's state, when it still has the state
    /// that the undo gave it.
    pub(crate) async fn plan_home_redo(
        &self,
        _conversation: ConversationId,
        undo: &str,
        turn: TurnId,
        scan: &Scan,
        roots: &[RootPlan],
    ) -> Result<HomePlan, SnapshotError> {
        let Some((store, held)) = &scan.home else { return Ok(HomePlan::default()) };
        let Some(copy) = held.undos.get(undo).and_then(|has| has.home.clone()) else {
            return Ok(HomePlan::default());
        };
        let before_undo = self.manifest_of(store, &copy).await?;
        let copies = match held.turns.get(&turn) {
            Some(has) => self.turn_copies(store, has).await?,
            None => BTreeMap::new(),
        };
        let mut steps = Vec::new();
        for item in &before_undo.entries {
            let path = PathBuf::from(&item.path);
            if in_roots(&path, roots) {
                continue;
            }
            let Some(target) = copies.get(&path).map(|copied| &copied.first) else { continue };
            if target.same_state(item) {
                continue;
            }
            let mut write = self.state_of(store, item).await?;
            let expect = self.expect_of(store, target).await?;
            if expect == Expect::Unknown && !matches!(write, HomeState::NotCovered(_)) {
                write = HomeState::NotCovered(UndoReason::Unknown);
            }
            steps.push(HomeStep {
                path,
                write,
                blob: blob_of(item),
                expect,
                trash: trash_of(target),
                floor: item.floor || target.floor,
            });
        }
        Ok(HomePlan { steps })
    }

    /// The journal ref `started` of `plan` in the home store.
    pub(crate) async fn home_started(&self, plan: &UndoPlan) -> Result<(), SnapshotError> {
        let store = self.home_store();
        let marker = self.marker(&store, plan, "started", None).await?;
        let name = undo_ref(plan.conversation, &plan.undo_id, "started");
        self.update_refs(&store, format!("create {name} {marker}\n")).await
    }

    /// Each path of the turn's copies `has`, in the order of the copies.
    async fn turn_copies(
        &self,
        store: &Store,
        has: &TurnHas,
    ) -> Result<BTreeMap<PathBuf, PathCopies>, SnapshotError> {
        let mut paths: BTreeMap<PathBuf, PathCopies> = BTreeMap::new();
        for commit in has.home.values() {
            let info = self.commit_info(store, commit).await?;
            let (before, after) = match (info.meta.phase.as_str(), info.parent.as_deref()) {
                ("after", Some(parent)) => (
                    self.manifest_of(store, parent).await?,
                    Some(self.manifest_of(store, commit).await?),
                ),
                _ => (self.manifest_of(store, commit).await?, None),
            };
            let mut names: Vec<&str> =
                before.entries.iter().map(|item| item.path.as_str()).collect();
            if let Some(after) = &after {
                names.extend(after.entries.iter().map(|item| item.path.as_str()));
            }
            names.sort_unstable();
            names.dedup();
            for name in names {
                let was = before.get(name).cloned().unwrap_or_else(|| missing(name));
                let last = match &after {
                    None => After::Unfinished,
                    Some(after) => After::Done(after.get(name).cloned()),
                };
                let floor = was.floor
                    || after
                        .as_ref()
                        .and_then(|after: &Manifest| after.get(name))
                        .is_some_and(|item| item.floor);
                let path = PathBuf::from(name);
                match paths.get_mut(&path) {
                    Some(known) => {
                        known.last = last;
                        known.floor |= floor;
                    }
                    None => {
                        paths.insert(path, PathCopies { first: was, last, floor });
                    }
                }
            }
        }
        Ok(paths)
    }

    /// The state that `item` names, as a step writes it.
    async fn state_of(&self, store: &Store, item: &Item) -> Result<HomeState, SnapshotError> {
        Ok(match item.kind {
            Kind::File => HomeState::File { mode: item.mode.unwrap_or(0o600) },
            Kind::Missing => HomeState::Missing,
            Kind::State => HomeState::State,
            Kind::NotCovered => HomeState::NotCovered(item.reason.unwrap_or(UndoReason::LeftOut)),
            Kind::Link => match item.blob.as_deref().and_then(ObjectId::parse) {
                Some(id) => HomeState::Link { target: self.blob_in(store, &id).await? },
                None => HomeState::NotCovered(UndoReason::Unknown),
            },
        })
    }

    /// What the path of `item` must be for a step to write it.
    async fn expect_of(&self, store: &Store, item: &Item) -> Result<Expect, SnapshotError> {
        let blob = item.blob.as_deref().and_then(ObjectId::parse);
        Ok(match (item.kind, blob) {
            (Kind::Missing, _) => Expect::Missing,
            (Kind::File | Kind::State, Some(id)) => Expect::Blob(id),
            (Kind::Link, Some(id)) => Expect::Link(self.blob_in(store, &id).await?),
            _ => Expect::Unknown,
        })
    }
}

/// True when `path` lies at or below a root of `roots`.
fn in_roots(path: &Path, roots: &[RootPlan]) -> bool {
    roots.iter().any(|root| path.starts_with(&root.root))
}

fn missing(path: &str) -> Item {
    Item { path: path.to_owned(), kind: Kind::Missing, ..Item::default() }
}

fn not_covered(item: &Item) -> Option<UndoReason> {
    (item.kind == Kind::NotCovered).then(|| item.reason.unwrap_or(UndoReason::LeftOut))
}

/// The blob that a step writes for `item`: a file's or a state's.
fn blob_of(item: &Item) -> Option<ObjectId> {
    matches!(item.kind, Kind::File | Kind::State)
        .then(|| item.blob.as_deref().and_then(ObjectId::parse))
        .flatten()
}

/// The trash blob and mode of the path when it has the state of `item`.
fn trash_of(item: &Item) -> Option<(ObjectId, u32)> {
    let id = item.blob.as_deref().and_then(ObjectId::parse)?;
    match item.kind {
        Kind::File => Some((id, 0o100_000 | (item.mode.unwrap_or(0o600) & 0o777))),
        Kind::State => Some((id, 0o100_600)),
        Kind::Link => Some((id, 0o120_000)),
        Kind::Missing | Kind::NotCovered => None,
    }
}
