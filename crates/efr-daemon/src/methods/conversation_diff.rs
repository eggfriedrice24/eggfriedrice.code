//! `conversation.diff`: what a turn changed in files, from efr's own snapshot store.

use efr_protocol::{ConversationDiff, ConversationDiffResult, MAX_TURN_DIFF_LINES};
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: ConversationDiff,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let conversation_id = match params.conversation_id {
        Some(conversation_id) => conversation_id,
        None => state
            .connections
            .tty(context.conn_id())
            .and_then(|tty| state.conversations.active(&tty))
            .map(|active| active.conversation_id)
            .ok_or(DaemonError::NoActiveConversation)?,
    };
    let exists = state
        .readers
        .with(move |conn| efr_store::conversations::get(conn, conversation_id))
        .await?
        .is_some();
    if !exists {
        return Err(DaemonError::ConversationNotFound { conversation_id });
    }
    let found = state
        .snapshots
        .turn_diff(conversation_id, params.turn_id, !params.stat, MAX_TURN_DIFF_LINES)
        .await
        .map_err(|source| DaemonError::Snapshot { source })?;
    let Some(found) = found else {
        return Err(DaemonError::NoSnapshot { conversation_id });
    };
    let result =
        ConversationDiffResult { turn_id: found.turn_id, changes: found.changes, diff: found.diff };
    responder.item(&result).await?;
    Ok(())
}
