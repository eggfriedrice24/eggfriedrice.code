//! The end of a conversation delete, after its tombstone (`conversation.delete`).
//!
//! The method records `conversation_deleted` first; from that commit on the
//! conversation shows nowhere and the store refuses its events. [`finish`] then
//! removes the rest in an order that a stop of efrd may break at any point, because
//! the tombstone keeps what each step needs and every step can run again:
//!
//! 1. the recording writer of each of its PTYs stops for good, so no late byte makes a
//!    file again;
//! 2. its rows, in short writer jobs (`WriterHandle::delete_step`): its events with
//!    their search docs and receipts, then its saved messages, its search title, its
//!    outbox rows and the index of its recordings;
//! 3. its snapshot refs in every store, and each root store that no other
//!    conversation uses, such as the store of its `$SCRATCH`
//!    (`Snapshots::forget_conversation`);
//! 4. its files: the recordings of its PTYs, each `$SCRATCH` dir whose marker names it
//!    (with the job outputs inside), its sandbox state dir (the private `/tmp`, the
//!    cache layers, the quarantine), its runtime dir of the hidden shell, its legacy
//!    file backups, and the undo trash of each of its undos;
//! 5. the mark that the delete is done.
//!
//! The method answers after the tombstone and puts the delete in the [`Queue`]. One
//! task, [`serve`], ends the deletes one at a time, so the answer never waits for the
//! purge of the search docs, which takes seconds for a long conversation. The task
//! starts after the socket opens and ends first the deletes that a stop of efrd broke
//! off ([`pending`]), because the tombstone already hides each of them. A stop of efrd
//! between two steps leaves the delete open.
//!
//! A path that cannot go is logged, and the delete stays open: the next start of efrd
//! tries again. Nothing here follows a link: a dir is removed with its links, never
//! their targets.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{ConversationId, PtyId, UndoId};
use efr_stdx::paths::Dirs;
use efr_store::Readers;
use efr_store::deletes::DeleteStep;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::attachments::ATTACHMENTS_DIR;
use crate::backups::FILE_BACKUPS_DIR;
use crate::home::UNDO_TRASH_DIR;
use crate::sandbox::plan::{SANDBOX_DIR, SHELL_DIR};
use crate::state::{SCRATCH_DIR, State};

/// Removes everything of the deleted conversation `conversation_id` after its
/// tombstone, as the module says, and marks the delete done when nothing is left.
async fn finish(state: &State, conversation_id: ConversationId) {
    let tombstone = match state
        .readers
        .with(move |conn| efr_store::deletes::get(conn, conversation_id))
        .await
    {
        Ok(Some(tombstone)) => tombstone,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%conversation_id, %error, "the tombstone of a deleted conversation could not be read");
            return;
        }
    };
    for pty in &tombstone.ptys {
        state.recording.forget(*pty).await;
    }
    state.attachment_cache.remove_conversation(conversation_id);
    let rows = remove_rows(state, conversation_id).await;
    let snapshots = state.snapshots.forget_conversation(conversation_id).await;
    let undos: Vec<UndoId> = tombstone
        .undos
        .iter()
        .copied()
        .chain(snapshots.undos.iter().filter_map(|undo| undo.parse().ok()))
        .collect();
    let paths =
        paths(&state.dirs, state.recordings.root(), conversation_id, &tombstone.ptys, &undos);
    let wanted = paths.clone();
    let not_removed =
        tokio::task::spawn_blocking(move || remove_all(paths)).await.unwrap_or(wanted);
    let mut done = rows && snapshots.failed == 0 && not_removed.is_empty();
    if done {
        match state.writer.delete_done(conversation_id).await {
            Ok(marked) => done = marked,
            Err(error) => {
                tracing::warn!(%conversation_id, %error, "the end of a delete could not be recorded");
                done = false;
            }
        }
    }
    if done {
        tracing::info!(%conversation_id, refs = snapshots.refs_deleted, stores = snapshots.stores_deleted, "a deleted conversation is gone");
    } else {
        tracing::warn!(%conversation_id, rows, snapshot_stores_failed = snapshots.failed, not_removed = not_removed.len(), "a deleted conversation is not gone yet; the next start of efrd tries again");
    }
}

/// The deletes that a stop of efrd broke off, oldest first. The start reads them before
/// the socket opens.
pub(crate) async fn pending(readers: &Readers) -> Vec<ConversationId> {
    match readers.with(efr_store::deletes::pending).await {
        Ok(pending) => pending.into_iter().map(|tombstone| tombstone.conversation_id).collect(),
        Err(error) => {
            tracing::warn!(%error, "the open deletes of conversations could not be read");
            Vec::new()
        }
    }
}

/// The deletes whose end waits for [`serve`], and the number of deletes that are not
/// ended yet.
#[derive(Debug, Clone)]
pub(crate) struct Queue {
    sender: mpsc::Sender<ConversationId>,
    open: watch::Sender<usize>,
}

impl Queue {
    /// A queue that counts `pending` deletes from the last run of efrd as open, and the
    /// receiver for [`serve`].
    pub(crate) fn new(pending: usize) -> (Queue, mpsc::Receiver<ConversationId>) {
        let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
        (Queue { sender, open: watch::Sender::new(pending) }, receiver)
    }

    /// Puts the delete of `conversation_id`, whose tombstone committed, in the queue.
    pub(crate) async fn push(&self, conversation_id: ConversationId) {
        self.open.send_modify(|open| *open += 1);
        if self.sender.send(conversation_id).await.is_err() {
            // NOTE: efrd stops, and its next start ends the delete.
            self.ended();
        }
    }

    /// The number of deletes that are not ended yet.
    pub(crate) fn open(&self) -> watch::Receiver<usize> {
        self.open.subscribe()
    }

    fn ended(&self) {
        self.open.send_modify(|open| *open = open.saturating_sub(1));
    }
}

/// The deletes that can wait in the [`Queue`] before a push waits.
const QUEUE_CAPACITY: usize = 64;

/// Ends the deletes of `pending` (from the last run of efrd), then each delete of the
/// queue, one at a time, until `stop`. A stop drops the delete at work between two
/// steps; every step can run again, so the next start ends it.
pub(crate) async fn serve(
    state: Arc<State>,
    pending: Vec<ConversationId>,
    mut queue: mpsc::Receiver<ConversationId>,
    stop: CancellationToken,
) {
    let mut pending = pending.into_iter();
    loop {
        let conversation_id = match pending.next() {
            Some(conversation_id) => {
                tracing::info!(%conversation_id, "finishing the delete of a conversation");
                conversation_id
            }
            None => tokio::select! {
                () = stop.cancelled() => return,
                next = queue.recv() => match next {
                    Some(conversation_id) => conversation_id,
                    None => return,
                },
            },
        };
        tokio::select! {
            () = stop.cancelled() => return,
            () = finish(&state, conversation_id) => state.deletes.ended(),
        }
    }
}

/// Removes the rows of the deleted conversation step by step; true when they are gone.
async fn remove_rows(state: &State, conversation_id: ConversationId) -> bool {
    loop {
        match state.writer.delete_step(conversation_id).await {
            Ok(DeleteStep::Removed(_)) => {}
            Ok(DeleteStep::RowsDone) => return true,
            Ok(DeleteStep::NotDeleted) => return false,
            Err(error) => {
                tracing::warn!(%conversation_id, %error, "the rows of a deleted conversation could not be removed");
                return false;
            }
        }
    }
}

/// Every file and dir of the conversation outside the database and the snapshot
/// stores. Each part of a path is an id that efrd made, never text from a file.
fn paths(
    dirs: &Dirs,
    recordings: &Path,
    conversation_id: ConversationId,
    ptys: &[PtyId],
    undos: &[UndoId],
) -> Vec<PathBuf> {
    let id = conversation_id.to_string();
    let mut paths: Vec<PathBuf> = ptys.iter().map(|pty| recordings.join(pty.to_string())).collect();
    paths.extend(efr_conversation::scratch_dirs(&dirs.data().join(SCRATCH_DIR), conversation_id));
    paths.push(dirs.state().join(SANDBOX_DIR).join(&id));
    paths.push(dirs.runtime().join(SHELL_DIR).join(&id));
    paths.push(dirs.data().join(FILE_BACKUPS_DIR).join(&id));
    paths.push(dirs.data().join(ATTACHMENTS_DIR).join(&id));
    let trash = dirs.data().join(UNDO_TRASH_DIR);
    paths.extend(undos.iter().map(|undo| trash.join(undo.to_string())));
    paths.sort();
    paths.dedup();
    paths
}

/// Removes each of `paths` that is there, a dir with all below it and a link as a
/// link; the paths that stay. It blocks.
fn remove_all(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut left = Vec::new();
    for path in paths {
        let Ok(meta) = path.symlink_metadata() else { continue };
        let removed = if meta.is_dir() {
            // NOTE: an overlay's work dir and a dir that a command made read-only hold
            // entries that cannot go before their dir is writable again.
            crate::sandbox::gc::make_removable(&path);
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(error) = removed {
            tracing::warn!(path = %path.display(), %error, "a file of a deleted conversation could not be removed");
            left.push(path);
        }
    }
    left
}

#[cfg(test)]
mod tests;
