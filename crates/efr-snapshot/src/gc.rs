//! The collector of the store: two bounds keep it small.
//!
//! - Count: each conversation keeps the refs of its newest `keep_turns` turns in each
//!   store; older refs are deleted, then `git prune` removes the loose objects that no
//!   ref and no index entry reaches any more (older than an hour, so a snapshot that is
//!   being written keeps its objects), and `git gc --auto` packs the rest when there
//!   are many.
//! - Age: a store whose last snapshot is older than `max_age` (the root file's time,
//!   which each snapshot touches at most once an hour) is deleted whole: its
//!   repository, its index and its root file.

use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, SystemTime};

use crate::SnapshotError;
use crate::history::stores;
use crate::runner::Run;
use crate::snapshots::Snapshots;
use crate::store::Store;

/// What one collection did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Stores deleted for their age.
    pub stores_deleted: usize,
    /// Refs deleted beyond the newest turns of a conversation.
    pub refs_deleted: usize,
}

impl Snapshots {
    /// Deletes the refs of all but the newest `keep_turns` turns of each conversation
    /// in each store, and every store without a snapshot for `max_age`. A running turn
    /// has no refs yet, so it loses nothing.
    pub async fn gc(&self, keep_turns: usize, max_age: Duration) -> GcReport {
        let dir = self.dir().to_path_buf();
        let all = match self.runner().blocking(&dir.clone(), move || stores(&dir)).await {
            Ok(all) => all,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), "the snapshot stores could not be listed");
                return GcReport::default();
            }
        };
        let now = SystemTime::from(self.runner().clock().now());
        let mut report = GcReport::default();
        for store in all {
            let slot = self.slot(store.root());
            let _turn = slot.enter().await;
            let file = store.root_file();
            let idle = fs::metadata(&file)
                .and_then(|metadata| metadata.modified())
                .map(|modified| now.duration_since(modified).unwrap_or_default())
                .unwrap_or(max_age);
            if idle >= max_age {
                let removed = store.clone();
                let gone = self
                    .runner()
                    .blocking(&store.git_dir(), move || remove_store(&removed))
                    .await
                    .unwrap_or(false);
                if gone {
                    slot.set_tree(None);
                    report.stores_deleted += 1;
                    tracing::info!(root = %store.root().display(), "deleted an idle snapshot store");
                }
                continue;
            }
            match self.trim_refs(&store, keep_turns).await {
                Ok(deleted) => report.refs_deleted += deleted,
                Err(error) => {
                    tracing::warn!(error = %efr_stdx::with_causes(&error), root = %store.root().display(), "old snapshots could not be deleted");
                }
            }
        }
        report
    }

    /// Deletes the refs of all but the newest `keep_turns` turns of each conversation
    /// in `store`, and the objects that nothing reaches any more.
    async fn trim_refs(&self, store: &Store, keep_turns: usize) -> Result<usize, SnapshotError> {
        let out = self
            .runner()
            .checked(
                store,
                "for-each-ref",
                &["for-each-ref", "--format=%(refname)", "refs/efr/"],
                Run::default(),
            )
            .await?;
        let text = String::from_utf8_lossy(&out);
        // conversation -> turn -> its refs
        let mut by_conversation: BTreeMap<&str, BTreeMap<&str, Vec<&str>>> = BTreeMap::new();
        for name in text.lines() {
            let mut parts = name.splitn(5, '/');
            let (Some("refs"), Some("efr"), Some(conversation), Some(turn)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            by_conversation.entry(conversation).or_default().entry(turn).or_default().push(name);
        }
        let mut doomed: Vec<&str> = Vec::new();
        for turns in by_conversation.values() {
            // NOTE: turn ids are UUIDv7, so their text sorts by time.
            let old = turns.len().saturating_sub(keep_turns);
            for refs in turns.values().take(old) {
                doomed.extend(refs.iter().copied());
            }
        }
        if doomed.is_empty() {
            return Ok(0);
        }
        let commands: String = doomed.iter().map(|name| format!("delete {name}\n")).collect();
        self.runner()
            .checked(
                store,
                "update-ref",
                &["update-ref", "--stdin"],
                Run { stdin: Some(commands.into_bytes()), ..Run::default() },
            )
            .await?;
        // NOTE: prune and gc keep every object that the persistent index names, because
        // GIT_INDEX_FILE points at it.
        self.runner()
            .checked(store, "prune", &["prune", "--expire=1.hour.ago"], Run::default())
            .await?;
        self.runner()
            .run(
                store,
                &["-c", "gc.pruneExpire=1.hour.ago", "gc", "--auto", "--quiet"],
                Run::default(),
            )
            .await?;
        Ok(doomed.len())
    }
}

/// Removes the repository, the index and the root file of `store`; true when the
/// repository is gone.
fn remove_store(store: &Store) -> bool {
    let removed = fs::remove_dir_all(store.git_dir()).is_ok();
    let _ = fs::remove_file(store.index());
    let _ = fs::remove_file(store.root_file());
    removed
}
