//! An undo of a turn (`turn.undo`): the actor's one job between turns, like a manual
//! compaction.
//!
//! The actor starts it only while no turn, compaction or other undo runs, and while no
//! call of the conversation still runs after its tool answered; else it refuses with
//! `UndoBusy`. It runs in a task of its own, so the actor keeps answering; a prompt that
//! arrives meanwhile waits in the queue and starts after it. The toolbox does the work
//! ([`Toolbox::undo`](crate::Toolbox::undo)). Only an undo that changed files or refs
//! is recorded, also one that stopped half way at a write that failed: `undo_applied`
//! with the command's receipt, in one batch. A dry run and a list record nothing.

use std::sync::Arc;

use efr_protocol::{
    Event, Origin, TurnUndo, TurnUndoResult, UndoAction, UndoEntry, UndoId, UndoReason,
};
use efr_stdx::id::uuid_v7;
use efr_store::Batch;

use super::receipt;
use crate::turn::Shared;
use crate::{ConversationError, UndoCandidate, UndoRequest};

/// The method of the receipt.
pub(super) const TURN_UNDO: &str = "turn.undo";

/// Runs the undo `params` of the conversation of `shared` over `candidates`: asks the
/// toolbox, and records `undo_applied` with the receipt when the undo changed files or
/// refs, also when it stopped half way.
pub(super) async fn run(
    shared: Arc<Shared>,
    params: TurnUndo,
    candidates: Vec<UndoCandidate>,
    origin: Origin,
) -> Result<TurnUndoResult, ConversationError> {
    let conversation_id = shared.conversation_id;
    let undo_id = UndoId::from_uuid(uuid_v7(&*shared.deps.clock, &*shared.deps.rng));
    let dry_run = params.dry_run || params.list;
    let command_id = params.command_id;
    let redo = params.redo;
    let request = UndoRequest { conversation_id, candidates, params, undo_id, origin };
    let result = shared.deps.toolbox.undo(request).await.map_err(ConversationError::UndoRefused)?;
    if dry_run || !(result.applied || result.stopped) {
        return Ok(result);
    }
    let Some(event) = applied(&result, redo, origin) else {
        tracing::warn!(%undo_id, "an applied undo named no turn, so it was not recorded");
        return Ok(result);
    };
    let receipt = receipt(command_id, TURN_UNDO, &result)?;
    let batch = Batch::new().event(conversation_id, event).receipt(receipt);
    shared.deps.writer.append(batch).await.map_err(ConversationError::from_store)?;
    Ok(result)
}

/// The `undo_applied` event of an undo that changed files or refs; `None` when the
/// result names no turn.
fn applied(result: &TurnUndoResult, redo: bool, origin: Origin) -> Option<Event> {
    let undone_turn_id = result.turn_id?;
    let undo_id = result.undo_id?;
    let count = |wanted: fn(&UndoEntry) -> bool| {
        u32::try_from(result.entries.iter().filter(|entry| wanted(entry)).count())
            .unwrap_or(u32::MAX)
    };
    Some(Event::UndoApplied {
        conversation_id: result.conversation_id,
        undone_turn_id,
        undo_id,
        redo,
        origin,
        restored: count(|entry| matches!(entry.action, UndoAction::Restore | UndoAction::Command)),
        trashed: count(|entry| entry.action == UndoAction::Trash),
        deleted: count(|entry| entry.action == UndoAction::Delete),
        conflicts: count(|entry| {
            entry.action == UndoAction::Skip && entry.reason == Some(UndoReason::ChangedAfterTurn)
        }),
        not_covered: count(|entry| entry.action == UndoAction::NotCovered),
        refs: result.refs.clone(),
        trash: result.trash.clone(),
        stopped: result.stopped,
    })
}
