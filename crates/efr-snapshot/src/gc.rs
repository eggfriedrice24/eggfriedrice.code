//! The collector of the stores: three bounds keep them small.
//!
//! - Count: each conversation keeps its newest `keep_turns` turns, counted across every
//!   store, the home store with them, so a turn stays or goes whole: its refs in each
//!   root store, its mirror, its home copies and the refs of its undos. Then `git
//!   prune` removes the loose objects that no ref and no index entry reaches any more
//!   (older than an hour, so a snapshot that is being written keeps its objects), and
//!   `git gc --auto` packs them at each collection once there are many. The trees that
//!   a running turn or call still needs reach no ref yet, so they are pinned under
//!   `refs/efr-live/<tree>` while the prune and the packing run.
//! - Mirror: the objects that only the mirrors of a root store reach stay under four
//!   times `snapshot.mirror_max_mib`; above it, the mirrors of the oldest turns go
//!   first (only the mirrors; their turns' refs are then not covered).
//! - Age: a store whose last snapshot is older than `max_age` (the root file's time,
//!   which each snapshot touches at most once an hour) is deleted whole: its
//!   repository, its index and its root file. The home store has its own stamp,
//!   `home.root`; a home store without one gets one, and is never deleted for it.
//!
//! Only `<16 hex>.git` is a root store, and `home.git` the home store; another
//! repository in the directory is no business of this collector.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::{Duration, SystemTime};

use crate::SnapshotError;
use crate::capture::Limits;
use crate::history::stores;
use crate::runner::Run;
use crate::snapshots::Snapshots;
use crate::store::Store;

/// The refs that pin the trees of running turns and calls during a collection.
const LIVE_REFS: &str = "refs/efr-live/";

/// The refs of the turns: `pre`, `last`, `post`, `undone` and the home copies.
const TURN_REFS: &str = "refs/efr/";

/// The mirrors of the project's refs, which go with their turn.
const MIRROR_REFS: &str = "refs/efr-mirror/";

/// The refs of the undos, which go with the turn that they undid.
const UNDO_REFS: &str = "refs/efr-undo/";

/// How many times `mirror_max_bytes` the mirrors of one store may hold in all.
const MIRROR_CAP_FACTOR: u64 = 4;

/// `git gc --auto`: it packs once there are about 6700 loose objects (git's own
/// default, which the store's `gc.auto=0` turns off for every other command) and
/// prunes what nothing reaches and is older than an hour.
const GC_AUTO: &[&str] =
    &["-c", "gc.auto=6700", "-c", "gc.pruneExpire=1.hour.ago", "gc", "--auto", "--quiet"];

/// What one collection did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Stores deleted for their age.
    pub stores_deleted: usize,
    /// Refs deleted beyond the newest turns of a conversation, or for the mirror cap.
    pub refs_deleted: usize,
}

/// A conversation and a turn, as their refs name them.
type TurnKey = (String, String);

/// What the first pass saw of one store.
#[derive(Debug, Default)]
struct Seen {
    /// The turns with refs of their own (`refs/efr/` or `refs/efr-mirror/`).
    turns: BTreeSet<TurnKey>,
    /// The turn of each undo, by conversation and undo id; `None` when no commit of
    /// the store names it.
    undos: BTreeMap<TurnKey, Option<String>>,
}

impl Snapshots {
    /// Deletes the refs of all but the newest `keep_turns` turns of each conversation,
    /// counted across every store, and every store without a snapshot for `max_age`.
    /// The mirrors of a store stay under four times the default `mirror_max_bytes`. A
    /// running turn has no refs yet, so it loses nothing.
    pub async fn gc(&self, keep_turns: usize, max_age: Duration) -> GcReport {
        self.gc_capped(keep_turns, max_age, Limits::default().mirror_max_bytes).await
    }

    /// [`Snapshots::gc`] with the mirrors of each store under four times
    /// `mirror_max_bytes` (`snapshot.mirror_max_mib`).
    pub async fn gc_capped(
        &self,
        keep_turns: usize,
        max_age: Duration,
        mirror_max_bytes: u64,
    ) -> GcReport {
        let dir = self.dir().to_path_buf();
        let roots = match self.runner().blocking(&dir.clone(), move || stores(&dir)).await {
            Ok(all) => all,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), "the snapshot stores could not be listed");
                return GcReport::default();
            }
        };
        let now = SystemTime::from(self.runner().clock().now());
        let mut report = GcReport::default();
        let mut kept: Vec<(Store, bool)> = Vec::new();
        for store in roots {
            if self.drop_if_idle(&store, now, max_age, false).await {
                report.stores_deleted += 1;
            } else {
                kept.push((store, false));
            }
        }
        let home = self.home_store();
        if home.exists() {
            if self.drop_if_idle(&home, now, max_age, true).await {
                report.stores_deleted += 1;
            } else {
                kept.push((home, true));
            }
        }
        let mut seen = Vec::with_capacity(kept.len());
        for (store, _) in &kept {
            match self.seen_in(store).await {
                Ok(found) => seen.push(found),
                Err(error) => {
                    tracing::warn!(error = %efr_stdx::with_causes(&error), root = %store.root().display(), "the refs of a snapshot store could not be listed");
                    seen.push(Seen::default());
                }
            }
        }
        let doomed = doomed_turns(seen.iter().flat_map(|found| &found.turns), keep_turns);
        let mut undos: BTreeMap<TurnKey, Option<String>> = BTreeMap::new();
        for (key, turn) in seen.iter().flat_map(|found| &found.undos) {
            let known = undos.entry(key.clone()).or_default();
            if known.is_none() {
                known.clone_from(turn);
            }
        }
        for (store, is_home) in &kept {
            let cap = (!is_home).then(|| mirror_max_bytes.saturating_mul(MIRROR_CAP_FACTOR));
            match self.trim_refs(store, &doomed, &undos, cap).await {
                Ok(deleted) => report.refs_deleted += deleted,
                Err(error) => {
                    tracing::warn!(error = %efr_stdx::with_causes(&error), root = %store.root().display(), "old snapshots could not be deleted");
                }
            }
        }
        report
    }

    /// Deletes `store` when its stamp is older than `max_age`; true when it went. A
    /// home store without a stamp gets one now.
    async fn drop_if_idle(
        &self,
        store: &Store,
        now: SystemTime,
        max_age: Duration,
        home: bool,
    ) -> bool {
        let slot = self.slot(store.root());
        let _turn = slot.enter().await;
        let file = store.root_file();
        let modified = fs::metadata(&file).and_then(|metadata| metadata.modified());
        let idle = match modified {
            Ok(modified) => now.duration_since(modified).unwrap_or_default(),
            Err(_) if home => {
                let stamped = store.clone();
                let _ = self
                    .runner()
                    .blocking(store.dir(), move || {
                        let _ = efr_stdx::fs::write_atomic(&stamped.root_file(), b"home");
                        stamped.stamp(now);
                    })
                    .await;
                Duration::ZERO
            }
            Err(_) => max_age,
        };
        if idle < max_age {
            return false;
        }
        let removed = store.clone();
        let gone = self
            .runner()
            .blocking(&store.git_dir(), move || remove_store(&removed))
            .await
            .unwrap_or(false);
        if gone {
            slot.forget_store();
            tracing::info!(root = %store.root().display(), "deleted an idle snapshot store");
        }
        gone
    }

    /// The turns and the undos that `store` holds refs of.
    async fn seen_in(&self, store: &Store) -> Result<Seen, SnapshotError> {
        let names = self.ref_names(store).await?;
        let mut seen = Seen::default();
        let mut undo_commits: BTreeMap<TurnKey, Vec<&str>> = BTreeMap::new();
        for (name, id) in &names {
            if let Some(key) = turn_key(name) {
                // NOTE: the marker `undone` of a turn names the undo that undid it, also
                // for an undo whose own commits in this store name no turn.
                if name.ends_with("/undone")
                    && let Ok(info) = self.commit_info(store, id).await
                    && let Some(undo) = info.meta.undo
                {
                    seen.undos.insert((key.0.clone(), undo), Some(key.1.clone()));
                }
                seen.turns.insert(key);
            } else if let Some((key, kind)) = undo_key(name) {
                let commits = undo_commits.entry(key).or_default();
                if !kind.starts_with("mirror/") {
                    commits.push(id);
                }
            }
        }
        for (key, commits) in undo_commits {
            let mut turn = None;
            for commit in commits {
                let named = self.commit_info(store, commit).await.ok().map(|info| info.meta.turn);
                if let Some(named) = named.filter(|named| !named.is_empty()) {
                    turn = Some(named);
                    break;
                }
            }
            let known = seen.undos.entry(key).or_default();
            if known.is_none() {
                *known = turn;
            }
        }
        Ok(seen)
    }

    /// Every ref of the collector's namespaces in `store`, with its object id.
    async fn ref_names(&self, store: &Store) -> Result<Vec<(String, String)>, SnapshotError> {
        let out = self
            .runner()
            .checked(
                store,
                "for-each-ref",
                &[
                    "for-each-ref",
                    "--format=%(refname) %(objectname)",
                    TURN_REFS,
                    MIRROR_REFS,
                    UNDO_REFS,
                    LIVE_REFS,
                ],
                Run::default(),
            )
            .await?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(name, id)| (name.to_owned(), id.to_owned()))
            .collect())
    }

    /// Deletes the refs of the `doomed` turns in `store` and of the undos of those
    /// turns (`undos` names the turn of each undo that any store knows), keeps its
    /// mirrors under `cap`, and then the objects that nothing reaches any more except
    /// the trees of running turns and calls.
    async fn trim_refs(
        &self,
        store: &Store,
        doomed: &BTreeSet<TurnKey>,
        undos: &BTreeMap<TurnKey, Option<String>>,
        cap: Option<u64>,
    ) -> Result<usize, SnapshotError> {
        let slot = self.slot(store.root());
        let _turn = slot.enter().await;
        let unlocked = store.clone();
        if self
            .runner()
            .blocking(&store.git_dir(), move || unlocked.clear_leftover_locks())
            .await
            .unwrap_or(false)
        {
            tracing::warn!(root = %store.root().display(), "removed a lock that a killed git left in a snapshot store");
        }
        let live = slot.live_trees();
        let names = self.ref_names(store).await?;
        let pins: Vec<String> = live.iter().map(|tree| format!("{LIVE_REFS}{tree}")).collect();
        let mut gone: Vec<&str> = Vec::new();
        for (name, _) in &names {
            let drop = if name.starts_with(LIVE_REFS) {
                // NOTE: a pin that a stop of efrd left behind goes with the next
                // collection.
                !pins.iter().any(|pin| pin == name)
            } else if let Some(key) = turn_key(name) {
                doomed.contains(&key)
            } else if let Some((key, _)) = undo_key(name) {
                match undos.get(&key) {
                    Some(Some(turn)) => doomed.contains(&(key.0.clone(), turn.clone())),
                    // NOTE: an undo that no commit names a turn for has no use.
                    Some(None) => true,
                    // NOTE: an undo that began after the first pass stays.
                    None => false,
                }
            } else {
                false
            };
            if drop {
                gone.push(name);
            }
        }
        let doomed_count = gone.iter().filter(|name| !name.starts_with(LIVE_REFS)).count();
        let mut commands: String = gone.iter().map(|name| format!("delete {name}\n")).collect();
        for (pin, tree) in pins.iter().zip(&live) {
            commands.push_str(&format!("update {pin} {tree}\n"));
        }
        if !commands.is_empty() {
            self.update_refs_held(store, commands).await?;
        }
        let capped = match cap {
            Some(cap) => {
                let left: Vec<&(String, String)> =
                    names.iter().filter(|(name, _)| !gone.contains(&name.as_str())).collect();
                self.cap_mirrors(store, &left, cap).await
            }
            None => 0,
        };
        // NOTE: prune and gc keep every object that the persistent index names, because
        // GIT_INDEX_FILE points at it.
        if doomed_count + capped > 0 {
            self.runner()
                .checked(store, "prune", &["prune", "--expire=1.hour.ago"], Run::default())
                .await?;
        }
        // NOTE: the store's hardening has gc.auto=0, which makes `gc --auto` do nothing;
        // a later -c wins, so the collector packs once there are many loose objects.
        self.runner().run(store, GC_AUTO, Run::default()).await?;
        if !pins.is_empty() {
            let unpin: String = pins.iter().map(|pin| format!("delete {pin}\n")).collect();
            self.update_refs_held(store, unpin).await?;
        }
        Ok(doomed_count + capped)
    }

    /// Deletes the mirrors of the oldest turns of `store` while the objects that only
    /// the mirrors reach take more than `cap` bytes; returns the refs it deleted.
    async fn cap_mirrors(&self, store: &Store, names: &[&(String, String)], cap: u64) -> usize {
        let mut by_turn: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, _) in names {
            if let Some((conversation, turn)) = mirror_key(name) {
                by_turn.entry(format!("{turn}/{conversation}")).or_default().push(name.clone());
            }
        }
        let others: Vec<&str> = names
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| name.starts_with(TURN_REFS))
            .collect();
        let mut deleted = 0;
        loop {
            let mirrors: Vec<&String> = by_turn.values().flatten().collect();
            let undo_mirrors = names
                .iter()
                .map(|(name, _)| name)
                .filter(|name| undo_key(name).is_some_and(|(_, kind)| kind.starts_with("mirror/")));
            let all: Vec<&String> = mirrors.into_iter().chain(undo_mirrors).collect();
            if all.is_empty() {
                return deleted;
            }
            let Some(used) = self.only_reached(store, &all, &others).await else { return deleted };
            if used <= cap {
                return deleted;
            }
            // NOTE: turn ids are UUIDv7, so their text sorts by time.
            let Some((_, oldest)) = by_turn.pop_first() else { return deleted };
            let commands: String = oldest.iter().map(|name| format!("delete {name}\n")).collect();
            if let Err(error) = self.update_refs_held(store, commands).await {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %store.root().display(), "the mirror of an old turn could not be deleted");
                return deleted;
            }
            tracing::info!(root = %store.root().display(), "deleted the mirror of an old turn to keep the store within its mirror limit");
            deleted += oldest.len();
        }
    }

    /// The bytes on disk of the objects that `refs` reach and `others` do not; `None`
    /// when git cannot tell.
    async fn only_reached(&self, store: &Store, refs: &[&String], others: &[&str]) -> Option<u64> {
        let mut input = String::new();
        for name in refs {
            input.push_str(name);
            input.push('\n');
        }
        for name in others {
            input.push('^');
            input.push_str(name);
            input.push('\n');
        }
        let out = self
            .runner()
            .checked(
                store,
                "rev-list",
                &["rev-list", "--objects", "--disk-usage", "--stdin"],
                Run { stdin: Some(input.into_bytes()), ..Run::default() },
            )
            .await;
        match out {
            Ok(out) => String::from_utf8_lossy(&out).trim().parse().ok(),
            Err(error) => {
                tracing::debug!(error = %efr_stdx::with_causes(&error), "the size of a store's mirrors could not be read");
                None
            }
        }
    }

    /// `update-ref --stdin` with `commands` in `store`, whose gate the caller holds.
    async fn update_refs_held(&self, store: &Store, commands: String) -> Result<(), SnapshotError> {
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

/// The turns of `seen` beyond the newest `keep_turns` of each conversation.
fn doomed_turns<'a>(
    seen: impl Iterator<Item = &'a TurnKey>,
    keep_turns: usize,
) -> BTreeSet<TurnKey> {
    let mut by_conversation: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (conversation, turn) in seen {
        by_conversation.entry(conversation).or_default().insert(turn);
    }
    let mut doomed = BTreeSet::new();
    for (conversation, turns) in by_conversation {
        // NOTE: turn ids are UUIDv7, so their text sorts by time.
        let old = turns.len().saturating_sub(keep_turns);
        for turn in turns.into_iter().take(old) {
            doomed.insert((conversation.to_owned(), turn.to_owned()));
        }
    }
    doomed
}

/// The conversation and turn of a ref below `refs/efr/` or `refs/efr-mirror/`.
fn turn_key(name: &str) -> Option<TurnKey> {
    let mut parts = name.splitn(5, '/');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("refs"), Some("efr" | "efr-mirror"), Some(conversation), Some(turn)) => {
            Some((conversation.to_owned(), turn.to_owned()))
        }
        _ => None,
    }
}

/// The conversation and turn of a ref below `refs/efr-mirror/`.
fn mirror_key(name: &str) -> Option<TurnKey> {
    name.starts_with(MIRROR_REFS).then(|| turn_key(name)).flatten()
}

/// The conversation and undo id of a ref below `refs/efr-undo/`, and the rest of its
/// name.
fn undo_key(name: &str) -> Option<(TurnKey, &str)> {
    let rest = name.strip_prefix(UNDO_REFS)?;
    let mut parts = rest.splitn(3, '/');
    let (conversation, undo, kind) = (parts.next()?, parts.next()?, parts.next()?);
    Some(((conversation.to_owned(), undo.to_owned()), kind))
}

/// Removes the repository, the index and the root file of `store`; true when the
/// repository is gone.
fn remove_store(store: &Store) -> bool {
    let removed = fs::remove_dir_all(store.git_dir()).is_ok();
    let _ = fs::remove_file(store.index());
    let _ = fs::remove_file(store.root_file());
    removed
}

#[cfg(test)]
mod tests;
