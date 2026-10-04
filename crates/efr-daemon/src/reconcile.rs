//! What the daemon does with work that was in flight when the last one stopped.
//!
//! Nothing continues on its own after a restart. Before the socket opens:
//!
//! - a running turn is cancelled (`turn_cancelled`): its provider stream and tool
//!   calls died with the process;
//! - a pending approval expires (`approval_expired`): the turn that waited for it is
//!   gone, so the answer could not resume anything;
//! - a queued prompt is held (`prompt_held`) until the user confirms it;
//! - a hidden shell that was running is recorded as exited (`shell_exited`): shells are
//!   children of the daemon at milestone 1 and end with it;
//! - process-bound outbox items are cancelled, and claimed replay-safe items return to
//!   the queue.
//!
//! [`plan`] decides the events from the projections without touching the store, so the
//! table is testable on its own; [`reconcile`] reads, plans and writes one batch per
//! conversation.

use std::collections::BTreeMap;

use efr_protocol::{ConversationId, Event};
use efr_store::approvals::PendingApproval;
use efr_store::conversations::{Turn, TurnStatus};
use efr_store::shells::Shell;
use efr_store::{Batch, Readers, WriterHandle};

use crate::DaemonError;

/// What the reconciliation did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Reconciled {
    pub(crate) turns_cancelled: usize,
    pub(crate) approvals_expired: usize,
    pub(crate) prompts_held: usize,
    pub(crate) shells_exited: usize,
    pub(crate) outbox_cancelled: u64,
    pub(crate) outbox_requeued: u64,
}

/// The events that settle what was in flight, per conversation, in log order: expired
/// approvals first, then the cancelled turn, then the held prompts.
pub(crate) fn plan(
    turns: &[Turn],
    approvals: &[PendingApproval],
    shells: &[Shell],
) -> BTreeMap<ConversationId, Vec<Event>> {
    let mut events: BTreeMap<ConversationId, Vec<Event>> = BTreeMap::new();
    for approval in approvals {
        events
            .entry(approval.conversation_id)
            .or_default()
            .push(Event::ApprovalExpired { turn_id: approval.turn_id, call_id: approval.call_id });
    }
    for turn in turns.iter().filter(|turn| turn.status == TurnStatus::Running) {
        events
            .entry(turn.conversation_id)
            .or_default()
            .push(Event::TurnCancelled { turn_id: turn.id });
    }
    for turn in turns.iter().filter(|turn| turn.status == TurnStatus::Queued) {
        events
            .entry(turn.conversation_id)
            .or_default()
            .push(Event::PromptHeld { turn_id: turn.id });
    }
    for shell in shells {
        events
            .entry(shell.conversation_id)
            .or_default()
            .push(Event::ShellExited { pty_id: shell.pty_id, exit_code: None });
    }
    events
}

/// Settles everything the last daemon left in flight. Runs once, before any actor
/// starts and before the socket opens.
pub(crate) async fn reconcile(
    readers: &Readers,
    writer: &WriterHandle,
) -> Result<Reconciled, DaemonError> {
    let (turns, approvals, shells) = readers
        .with(|conn| {
            Ok((
                efr_store::conversations::unfinished_turns(conn)?,
                efr_store::approvals::pending(conn, None)?,
                efr_store::shells::running(conn)?,
            ))
        })
        .await?;
    let mut done = Reconciled {
        turns_cancelled: turns.iter().filter(|turn| turn.status == TurnStatus::Running).count(),
        approvals_expired: approvals.len(),
        prompts_held: turns.iter().filter(|turn| turn.status == TurnStatus::Queued).count(),
        shells_exited: shells.len(),
        ..Reconciled::default()
    };
    for (conversation_id, events) in plan(&turns, &approvals, &shells) {
        let batch = events
            .into_iter()
            .fold(Batch::new(), |batch, event| batch.event(conversation_id, event));
        writer.append(batch).await?;
    }
    let outbox = writer.outbox_cancel_process_bound().await?;
    done.outbox_cancelled = outbox.cancelled;
    done.outbox_requeued = outbox.requeued;
    Ok(done)
}

#[cfg(test)]
mod tests;
