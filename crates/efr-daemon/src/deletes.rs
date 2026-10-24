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
//! A path that cannot go is logged and named in the answer, and the delete stays open:
//! [`finish_pending`] tries again at the next start of efrd, before the socket opens.
//! Nothing here follows a link: a dir is removed with its links, never their targets.

use std::path::{Path, PathBuf};

use efr_protocol::{ConversationId, PtyId, UndoId};
use efr_stdx::paths::Dirs;
use efr_store::deletes::DeleteStep;

use crate::backups::FILE_BACKUPS_DIR;
use crate::home::UNDO_TRASH_DIR;
use crate::sandbox::plan::{SANDBOX_DIR, SHELL_DIR};
use crate::state::{SCRATCH_DIR, State};

/// What [`finish`] left.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Finished {
    /// The paths that could not be removed.
    pub(crate) not_removed: Vec<PathBuf>,
    /// True when the delete is done; false leaves it for the next start.
    pub(crate) done: bool,
}

/// Removes everything of the deleted conversation `conversation_id` after its
/// tombstone, as the module says, and marks the delete done when nothing is left.
pub(crate) async fn finish(state: &State, conversation_id: ConversationId) -> Finished {
    let tombstone = match state
        .readers
        .with(move |conn| efr_store::deletes::get(conn, conversation_id))
        .await
    {
        Ok(Some(tombstone)) => tombstone,
        Ok(None) => return Finished::default(),
        Err(error) => {
            tracing::warn!(%conversation_id, %error, "the tombstone of a deleted conversation could not be read");
            return Finished::default();
        }
    };
    for pty in &tombstone.ptys {
        state.recording.forget(*pty).await;
    }
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
    Finished { not_removed, done }
}

/// Finishes every delete that a stop of efrd broke off, oldest first. It runs at the
/// start, before the socket opens.
pub(crate) async fn finish_pending(state: &State) {
    let pending = match state.readers.with(efr_store::deletes::pending).await {
        Ok(pending) => pending,
        Err(error) => {
            tracing::warn!(%error, "the open deletes of conversations could not be read");
            return;
        }
    };
    for tombstone in pending {
        tracing::info!(conversation_id = %tombstone.conversation_id, "finishing the delete of a conversation");
        finish(state, tombstone.conversation_id).await;
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
