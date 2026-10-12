//! The end of a background job (efr's auto spec, section 3.18, phase C): the files that
//! the job changed join the turn that started it.
//!
//! The job's start call takes a [`CallSnapshot`] of the job's roots right after the
//! start, and [`Snapshots::after_job`] compares each root with it at the job's end. A
//! path that another call or an undo changed in the same root while the job ran stays
//! out: the `touched` paths of every other turn, and the paths of every undo, whose
//! newest commit came at or after the job's start, so a later turn's write stays that
//! turn's. The rest joins the start turn:
//!
//! - While the start turn still runs, the paths join its `touched` paths, and its
//!   `last` ref moves, as after a call.
//! - Once it ended, its record after the turn is no longer whole: `post` goes, and
//!   `last` names a tree of `post` with the job's paths in their state at the job's end,
//!   with the job's paths in `touched` and the record marked not complete. The undo
//!   planner then reads `last`, so an undo of the start turn takes the job's writes
//!   back, and only with `force`, as for a turn with no record after it.

use std::collections::BTreeSet;
use std::path::Path;

use efr_protocol::{ConversationId, FileChanges, TurnId};
use jiff::Timestamp;

use crate::SnapshotError;
use crate::capture::Limits;
use crate::changes::{self, RootChange, Shown};
use crate::meta::Meta;
use crate::runner::Run;
use crate::snapshots::{CallSnapshot, Snapshots};
use crate::store::Store;
use crate::turn::{set_touched, turn_ref};

/// The ref kinds of a turn whose `touched` paths a later write may hold.
const TURN_KINDS: &[&str] = &["last", "post"];

/// The ref kinds of an undo whose `touched` paths it wrote.
const UNDO_KINDS: &[&str] = &["before", "after"];

impl Snapshots {
    /// Compares each root of `call`, the snapshot of a job's roots right after its
    /// start, with the root at the job's end, and adds what the job changed to the turn
    /// that started it. `since` is the job's start: a path that another turn or an
    /// undo changed since then stays out. Returns what joined the turn; `None` when the
    /// job changed nothing of its own.
    pub async fn after_job(
        &self,
        call: CallSnapshot,
        since: Timestamp,
        limits: Limits,
    ) -> Option<FileChanges> {
        let mut call = call;
        call.owner = None;
        let (Some(turn), Some(conversation)) = (call.turn, call.conversation) else {
            return None;
        };
        let before = std::mem::take(&mut call.before);
        let joined = futures::future::join_all(before.into_iter().map(|(root, before, pin)| {
            async move {
                let path = root.path().to_path_buf();
                let compared = self.changes_since(&path, &before, &limits, true).await;
                drop(pin);
                let (changes, tree) = compared?;
                let tree = tree?;
                let skip = self.touched_since(&path, conversation, turn, since).await;
                let own: Vec<RootChange> =
                    changes.into_iter().filter(|change| !skipped(change, &skip)).collect();
                if own.is_empty() {
                    return None;
                }
                if self.lock_turns().contains_key(&turn) {
                    // NOTE: the turn still runs: the job is a call that ran on.
                    self.mark_unfinished(turn, &path);
                    self.touch(turn, &path, &own, &tree).await;
                } else if let Err(error) =
                    self.join_ended(conversation, turn, &path, &own, &tree).await
                {
                    tracing::warn!(error = %efr_stdx::with_causes(&error), root = %path.display(), "the files of a job could not join the turn that started it");
                    return None;
                }
                Some(Shown { root: path, shown: root.shown().to_owned(), changes: own })
            }
        }))
        .await;
        changes::merge(joined.into_iter().flatten().collect())
    }

    /// The paths of the canonical `root` that a turn other than `turn` of
    /// `conversation`, or an undo, changed at or after `since`: the `touched` paths of
    /// each such commit of the root's store. A commit time is whole seconds, so a
    /// commit in the second of `since` counts.
    pub(crate) async fn touched_since(
        &self,
        root: &Path,
        conversation: ConversationId,
        turn: TurnId,
        since: Timestamp,
    ) -> BTreeSet<String> {
        let store = Store::new(self.dir(), root);
        let mut touched = BTreeSet::new();
        if !store.exists() {
            return touched;
        }
        let listed = self
            .runner()
            .checked(
                &store,
                "for-each-ref",
                &[
                    "for-each-ref",
                    "--format=%(objectname) %(committerdate:unix) %(refname)",
                    "refs/efr/",
                    "refs/efr-undo/",
                ],
                Run::default(),
            )
            .await;
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %root.display(), "the turns that changed a job's root could not be listed");
                return touched;
            }
        };
        let own = turn_ref(conversation, turn, "");
        let since = since.as_second();
        for line in String::from_utf8_lossy(&listed).lines() {
            let mut parts = line.splitn(3, ' ');
            let (Some(id), Some(time), Some(name)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if name.starts_with(&own) || time.parse::<i64>().is_ok_and(|time| time < since) {
                continue;
            }
            if !counts(name) {
                continue;
            }
            match self.commit_info(&store, id).await {
                Ok(info) => touched.extend(info.meta.touched),
                Err(error) => {
                    tracing::debug!(error = %efr_stdx::with_causes(&error), name, "a commit of a job's root could not be read");
                }
            }
        }
        touched
    }

    /// Adds `own`, the job's changes of the canonical `root`, to the turn `turn` of
    /// `conversation`, which ended: `last` names a tree of the turn's record after it
    /// with the job's paths as `tree` has them, and `post` goes.
    async fn join_ended(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        root: &Path,
        own: &[RootChange],
        tree: &str,
    ) -> Result<(), SnapshotError> {
        let store = Store::new(self.dir(), root);
        let held = self.held_in(&store, conversation).await?;
        let Some(has) = held.turns.get(&turn).filter(|has| has.pre.is_some()) else {
            tracing::debug!(root = %root.display(), "the turn that started a job kept no snapshot of this root");
            return Ok(());
        };
        let Some(source) = has.post.clone().or_else(|| has.last.clone()) else {
            return Ok(());
        };
        let info = self.commit_info(&store, &source).await?;
        let slot = self.slot(root);
        let _turn = slot.enter().await;
        let paths: Vec<String> = own
            .iter()
            .flat_map(|change| std::iter::once(change.path.clone()).chain(change.from.clone()))
            .collect();
        let built = self.with_paths(&store, &info.tree, tree, &paths).await?;
        let mut meta: Meta = info.meta.clone();
        meta.phase = "last".to_owned();
        meta.failed = true;
        let mut touched: BTreeSet<String> = meta.touched.iter().cloned().collect();
        touched.extend(paths);
        set_touched(&mut meta, &touched);
        let time = format!("{} +0000", self.runner().clock().now().as_second());
        let commit = self.commit(&store, &built, None, meta.message(), &time).await?;
        let last = turn_ref(conversation, turn, "last");
        let mut commands = match &has.last {
            Some(old) => format!("update {last} {commit} {old}\n"),
            None => format!("create {last} {commit}\n"),
        };
        if let Some(post) = &has.post {
            commands.push_str(&format!("delete {} {post}\n", turn_ref(conversation, turn, "post")));
        }
        self.runner()
            .checked(
                &store,
                "update-ref",
                &["update-ref", "--stdin"],
                Run { stdin: Some(commands.into_bytes()), ..Run::default() },
            )
            .await?;
        tracing::info!(root = %root.display(), paths = touched.len(), "a job's files joined the turn that started it");
        Ok(())
    }

    /// The tree `base` with each of `paths` as the tree `from` has it: taken from `from`,
    /// or gone when `from` lacks it. Built in a second index, so the store's persistent
    /// index keeps its stat cache. The caller holds the store's gate.
    async fn with_paths(
        &self,
        store: &Store,
        base: &str,
        from: &str,
        paths: &[String],
    ) -> Result<String, SnapshotError> {
        let index = store.build_index();
        let result = self.build_tree(store, &index, base, from, paths).await;
        let _ = std::fs::remove_file(&index);
        result
    }

    async fn build_tree(
        &self,
        store: &Store,
        index: &Path,
        base: &str,
        from: &str,
        paths: &[String],
    ) -> Result<String, SnapshotError> {
        let _ = std::fs::remove_file(index);
        let built = |stdin: Option<Vec<u8>>, literal: bool| Run {
            stdin,
            literal,
            index: Some(index),
            ..Run::default()
        };
        self.runner().checked(store, "read-tree", &["read-tree", base], built(None, false)).await?;
        let mut args: Vec<&str> = vec!["ls-tree", "-r", "-z", "--full-tree", from, "--"];
        args.extend(paths.iter().map(String::as_str));
        let listed = self.runner().checked(store, "ls-tree", &args, built(None, true)).await?;
        let mut found: BTreeSet<String> = BTreeSet::new();
        let mut info: Vec<u8> = Vec::new();
        let zero = "0".repeat(base.len());
        for record in listed.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
            let text = String::from_utf8_lossy(record);
            if let Some((_, path)) = text.split_once('\t') {
                found.insert(path.to_owned());
            }
        }
        // NOTE: the paths that are gone go first, so a file that became a directory
        // leaves before the files below it come.
        for path in paths.iter().filter(|path| !found.contains(*path)) {
            info.extend_from_slice(format!("0 {zero}\t{path}").as_bytes());
            info.push(0);
        }
        for record in listed.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
            info.extend_from_slice(record);
            info.push(0);
        }
        self.runner()
            .checked(
                store,
                "update-index",
                &["update-index", "-z", "--replace", "--index-info"],
                built(Some(info), false),
            )
            .await?;
        let out =
            self.runner().checked(store, "write-tree", &["write-tree"], built(None, false)).await?;
        let tree = String::from_utf8(out)
            .map(|tree| tree.trim().to_owned())
            .map_err(|_| SnapshotError::BadOutput { command: "write-tree" })?;
        if tree.is_empty() || !tree.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(SnapshotError::BadOutput { command: "write-tree" });
        }
        Ok(tree)
    }
}

/// True when the ref `name` holds paths that a call or an undo changed: a turn's `last`
/// or `post`, an undo's `before` or `after`.
fn counts(name: &str) -> bool {
    let kind = name.rsplit('/').next().unwrap_or_default();
    if name.starts_with("refs/efr-undo/") {
        UNDO_KINDS.contains(&kind)
    } else {
        TURN_KINDS.contains(&kind)
    }
}

/// True when `change` names a path of `skip`.
fn skipped(change: &RootChange, skip: &BTreeSet<String>) -> bool {
    skip.contains(&change.path) || change.from.as_ref().is_some_and(|from| skip.contains(from))
}

#[cfg(test)]
mod tests;
