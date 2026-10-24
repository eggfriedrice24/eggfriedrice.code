//! `conversation.delete`: delete a conversation and everything that efr keeps of it
//! (`efr conversation delete`, Ctrl+D in the `,resume` picker).
//!
//! The method needs `terminal`, which a phone does not hold by default and a
//! model-side peer never holds: a delete removes the record that undo and the reviewer
//! of `auto` read, and nothing takes it back.
//!
//! When another open terminal holds the conversation (a slot whose shell runs or is not
//! known, or a terminal that follows it), efrd deletes nothing and answers `held` with
//! that terminal, unless the params say `force`; the client asks the user and sends a
//! new command with `force`. A `held` answer keeps no receipt. The terminal that asks
//! may hold the conversation itself.
//!
//! A running turn, a queued prompt or a compaction makes the delete `busy`, unless the
//! params say `stop`: then efrd stops the actor, which drops them. An undo at work is
//! `busy` also with `stop`, because an undo that stops half way leaves the files of the
//! project half restored.
//!
//! Then, in this order: the actor stops; every running background job of the
//! conversation stops (the user's stop) and efrd waits for each end; the hidden shell
//! closes, with its screen; `conversation_deleted` commits with the receipt, and from
//! then on the conversation shows nowhere and the store refuses its events; the slots
//! of every terminal let it go; whatever raced the commit (an actor, a job, a shell)
//! stops again; and `deletes::finish` removes the rest. A retried command id answers
//! from its receipt.

use efr_protocol::{
    ConversationDelete, ConversationDeleteResult, ConversationId, Event, JobStopBy,
};
use efr_store::receipts::NewReceipt;
use efr_store::{Batch, StoreError};
use efr_transport::{ConnectionContext, Responder};
use serde_json::Value;

use crate::conversations;
use crate::state::State;
use crate::{DaemonError, deletes, receipts};

const METHOD: &str = "conversation.delete";

#[tracing::instrument(skip_all, fields(conversation_id = %params.conversation_id, force = params.force, stop = params.stop))]
pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: ConversationDelete,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    let answer = match receipts::lookup(state, command_id).await? {
        Some(receipt) => receipts::replay(METHOD, receipt)?,
        None => match delete(state, context, params).await {
            Ok(answer) => answer,
            Err(error @ DaemonError::Rejected { .. }) => return Err(error),
            Err(error) => return Err(receipts::refuse(state, command_id, METHOD, error).await),
        },
    };
    responder.item(&answer).await?;
    Ok(())
}

async fn delete(
    state: &State,
    context: &ConnectionContext,
    params: ConversationDelete,
) -> Result<Value, DaemonError> {
    let ConversationDelete { command_id, conversation_id, force, stop } = params;
    let summary = state
        .readers
        .with(move |conn| efr_store::conversations::get(conn, conversation_id))
        .await?
        .ok_or(DaemonError::ConversationNotFound { conversation_id })?;
    let asking = state.connections.tty(context.conn_id());
    if !force && let Some(held) = conversations::held(state, conversation_id, asking.as_deref()) {
        let held =
            ConversationDeleteResult::Held { tty: held.tty().to_owned(), title: summary.title };
        return encode(&held);
    }
    if let Some(handle) = state.conversations.live(conversation_id) {
        // NOTE: an actor that stopped while it was asked is at work no more.
        if let Ok(now) = handle.state().await {
            let working = now.running.is_some() || !now.queued.is_empty() || now.compacting;
            if now.undoing || (working && !stop) {
                return Err(DaemonError::ConversationBusy { conversation_id, undo: now.undoing });
            }
        }
    }
    state.conversations.set_deleting(conversation_id, true);
    let mut stopped = stop_all(state, conversation_id).await;
    let deleted = |jobs_stopped: usize, not_removed| ConversationDeleteResult::Deleted {
        conversation_id,
        title: summary.title.clone(),
        jobs_stopped: u32::try_from(jobs_stopped).unwrap_or(u32::MAX),
        not_removed,
    };
    let value = encode(&deleted(stopped, Vec::new()))?;
    let tombstone = Event::ConversationDeleted { origin: context.surface() };
    let batch = Batch::new()
        .event(conversation_id, tombstone)
        .receipt(NewReceipt::accepted(command_id, METHOD, value));
    // NOTE: a receipt of this command or an unknown conversation means that another
    // delete recorded it, so the mark stays.
    match state.writer.append(batch).await {
        Ok(_) => {}
        Err(StoreError::DuplicateCommand { receipt }) => return receipts::replay(METHOD, *receipt),
        Err(StoreError::UnknownConversation { .. }) => {
            return Err(DaemonError::ConversationNotFound { conversation_id });
        }
        Err(source) => {
            // NOTE: nothing was recorded; the conversation goes on, without the jobs and
            // the shell that the stops ended.
            state.conversations.set_deleting(conversation_id, false);
            return Err(DaemonError::Store { source });
        }
    }
    tracing::info!(%conversation_id, force, stop, jobs = stopped, "the user deleted a conversation");
    state.conversations.forget_slots(conversation_id);
    state.presence.forget(conversation_id);
    // NOTE: a prompt that reached the conversation between the stops and the commit
    // started an actor, a job or a shell; their events fail now, and they stop here.
    stopped += stop_all(state, conversation_id).await;
    let finished = deletes::finish(state, conversation_id).await;
    encode(&deleted(stopped, finished.not_removed))
}

/// Stops the actor, every running job and the hidden shell of `conversation_id`, in
/// that order; the number of jobs that it stopped.
async fn stop_all(state: &State, conversation_id: ConversationId) -> usize {
    state.conversations.forget(conversation_id).await;
    let jobs = state.jobs.stop_conversation(conversation_id, JobStopBy::User).await;
    if let Err(error) = state.shells.close(conversation_id).await {
        tracing::warn!(%conversation_id, %error, "the hidden shell of a deleted conversation could not be closed");
    }
    jobs.len()
}

fn encode(result: &ConversationDeleteResult) -> Result<Value, DaemonError> {
    serde_json::to_value(result)
        .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })
}
