//! What the daemon does with work that was in flight when the last one stopped.
//!
//! Nothing continues on its own after a restart. Before the socket opens:
//!
//! - a running turn is cancelled (`turn_cancelled`): its provider stream and tool
//!   calls died with the process;
//! - a pending approval expires (`approval_expired`): the turn that waited for it is
//!   gone, so the answer could not resume anything;
//! - a tool call of that turn without a result gets an error result
//!   (`tool_call_completed`), so the log answers every call it started, as a provider
//!   requires of the history;
//! - a queued prompt is recorded as not run (`turn_cancelled` without a
//!   `turn_started`), and its terminal gets a notice that names it and says to send it
//!   again: running it by surprise, maybe hours later in another directory, would be
//!   worse than asking. A prompt that an earlier daemon held (`prompt_held`) is settled
//!   the same way;
//! - a hidden shell that was running is recorded as exited (`shell_exited`): shells are
//!   children of the daemon at milestone 1 and end with it;
//! - process-bound outbox items are cancelled, and claimed replay-safe items return to
//!   the queue.
//!
//! [`plan`] decides the events from the projections without touching the store, so the
//! table is testable on its own; [`reconcile`] reads, plans and writes one batch per
//! conversation, then the notices.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use efr_protocol::{CallId, ConversationId, Event, EventEnvelope, TurnId};
use efr_store::approvals::PendingApproval;
use efr_store::conversations::{Turn, TurnStatus};
use efr_store::shells::Shell;
use efr_store::{Batch, Readers, WriterHandle};

use crate::{DaemonError, notices};

/// What the model reads for a call that was running or waiting for approval when the
/// daemon stopped.
pub(crate) const STOPPED_CALL: &str = "The call did not finish: the daemon stopped while it \
                                       ran or waited for approval; it may or may not have \
                                       run.";

/// How many events one read of a turn's events asks for.
const PAGE: u32 = 512;

/// What the reconciliation did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Reconciled {
    pub(crate) turns_cancelled: usize,
    pub(crate) approvals_expired: usize,
    pub(crate) calls_closed: usize,
    pub(crate) prompts_not_run: usize,
    pub(crate) shells_exited: usize,
    pub(crate) outbox_cancelled: u64,
    pub(crate) outbox_requeued: u64,
}

/// The events that settle what was in flight, per conversation, in log order: expired
/// approvals first, then the results of the calls in `open_calls` (the calls of each
/// running turn without a result), then the cancelled turn, then the prompts that did
/// not run.
pub(crate) fn plan(
    turns: &[Turn],
    approvals: &[PendingApproval],
    open_calls: &HashMap<TurnId, Vec<CallId>>,
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
        let settled = events.entry(turn.conversation_id).or_default();
        for call_id in open_calls.get(&turn.id).into_iter().flatten() {
            settled.push(Event::ToolCallCompleted {
                turn_id: turn.id,
                call_id: *call_id,
                output: STOPPED_CALL.to_owned(),
                truncated: false,
                is_error: true,
                exit_code: None,
                sandbox: None,
            });
        }
        settled.push(Event::TurnCancelled { turn_id: turn.id });
    }
    for turn in turns.iter().filter(|turn| not_run(turn)) {
        events
            .entry(turn.conversation_id)
            .or_default()
            .push(Event::TurnCancelled { turn_id: turn.id });
    }
    for shell in shells {
        events
            .entry(shell.conversation_id)
            .or_default()
            .push(Event::ShellExited { pty_id: shell.pty_id, exit_code: None });
    }
    events
}

/// True for a turn that waited when the daemon stopped: it never ran and never will.
fn not_run(turn: &Turn) -> bool {
    matches!(turn.status, TurnStatus::Queued | TurnStatus::Held)
}

/// Settles everything the last daemon left in flight, and writes a notice under
/// `notices_dir` for each conversation with prompts that did not run. Runs once, before any actor starts
/// and before the socket opens.
pub(crate) async fn reconcile(
    readers: &Readers,
    writer: &WriterHandle,
    notices_dir: &Path,
) -> Result<Reconciled, DaemonError> {
    let (turns, approvals, open_calls, shells, ttys) = readers
        .with(|conn| {
            let turns = efr_store::conversations::unfinished_turns(conn)?;
            let mut open = HashMap::new();
            for turn in turns.iter().filter(|turn| turn.status == TurnStatus::Running) {
                let mut calls = Vec::new();
                let mut after = turn.queued_seq;
                while after < turn.last_seq {
                    let page = efr_store::events::read_conversation_after(
                        conn,
                        turn.conversation_id,
                        after,
                        PAGE,
                    )?;
                    let Some(last) = page.last() else {
                        break;
                    };
                    after = last.seq;
                    track_calls(&mut calls, turn.id, &page);
                }
                open.insert(turn.id, calls);
            }
            let mut ttys = HashMap::new();
            for turn in turns.iter().filter(|turn| not_run(turn)) {
                if let Some(summary) = efr_store::conversations::get(conn, turn.conversation_id)?
                    && let Some(tty) = summary.tty
                {
                    ttys.insert(turn.conversation_id, tty);
                }
            }
            Ok((
                turns,
                efr_store::approvals::pending(conn, None)?,
                open,
                efr_store::shells::running(conn)?,
                ttys,
            ))
        })
        .await?;
    let mut done = Reconciled {
        turns_cancelled: turns.iter().filter(|turn| turn.status == TurnStatus::Running).count(),
        approvals_expired: approvals.len(),
        calls_closed: open_calls.values().map(Vec::len).sum(),
        prompts_not_run: turns.iter().filter(|turn| not_run(turn)).count(),
        shells_exited: shells.len(),
        ..Reconciled::default()
    };
    for (conversation_id, events) in plan(&turns, &approvals, &open_calls, &shells) {
        let batch = events
            .into_iter()
            .fold(Batch::new(), |batch, event| batch.event(conversation_id, event));
        writer.append(batch).await?;
    }
    let outbox = writer.outbox_cancel_process_bound().await?;
    done.outbox_cancelled = outbox.cancelled;
    done.outbox_requeued = outbox.requeued;
    // NOTE: written after the commit, so a notice never names a prompt that the log
    // still shows as waiting.
    let mut counts: BTreeMap<ConversationId, usize> = BTreeMap::new();
    for turn in turns.iter().filter(|turn| not_run(turn)) {
        *counts.entry(turn.conversation_id).or_default() += 1;
    }
    let not_run: Vec<(String, String)> = counts
        .into_iter()
        .filter_map(|(conversation_id, count)| {
            let tty = ttys.get(&conversation_id)?;
            Some((tty.clone(), notices::not_run(conversation_id, count)))
        })
        .collect();
    let dir = notices_dir.to_path_buf();
    let written = tokio::task::spawn_blocking(move || {
        for (tty, line) in not_run {
            if let Err(error) = notices::append(&dir, &tty, &line) {
                tracing::warn!(error = %error, "the notice of a prompt that did not run could not be written");
            }
        }
    })
    .await;
    if written.is_err() {
        tracing::warn!("writing the notices of prompts that did not run panicked");
    }
    Ok(done)
}

/// Adds the calls of `turn_id` that start in `page` to `open`, and removes those that
/// end there, so `open` keeps the calls without a result in the order they started.
fn track_calls(open: &mut Vec<CallId>, turn_id: TurnId, page: &[EventEnvelope]) {
    for envelope in page {
        match &envelope.event {
            Event::ToolCallStarted { turn_id: turn, call_id, .. } if *turn == turn_id => {
                open.push(*call_id);
            }
            Event::ToolCallCompleted { turn_id: turn, call_id, .. } if *turn == turn_id => {
                open.retain(|open| open != call_id);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
