//! `conversation.diff`: what a turn changed in files, from efr's own snapshot store.

use efr_protocol::{ConversationDiff, ConversationDiffResult, MAX_TURN_DIFF_LINES};
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::state::State;

/// Answers with the changes of the named turn, else of the conversation's newest
/// finished turn. A turn without snapshots changed no files, so its list is empty.
pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: ConversationDiff,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let named = match params.turn_id {
        Some(turn_id) => Some(
            state
                .readers
                .with(move |conn| efr_store::conversations::turn(conn, turn_id))
                .await?
                .filter(|turn| params.conversation_id.is_none_or(|id| id == turn.conversation_id))
                .ok_or(DaemonError::TurnNotFound { turn_id })?,
        ),
        None => None,
    };
    let conversation_id = match (params.conversation_id, &named) {
        (Some(conversation_id), _) => conversation_id,
        (None, Some(turn)) => turn.conversation_id,
        (None, None) => state
            .connections
            .tty(context.conn_id())
            .and_then(|tty| state.conversations.active(&tty))
            .map(|active| active.conversation_id)
            .ok_or(DaemonError::NoActiveConversation)?,
    };
    let turn_id = match named {
        Some(turn) => turn.id,
        None => {
            let exists = state
                .readers
                .with(move |conn| efr_store::conversations::get(conn, conversation_id))
                .await?
                .is_some();
            if !exists {
                return Err(DaemonError::ConversationNotFound { conversation_id });
            }
            let turns = state
                .readers
                .with(move |conn| efr_store::conversations::turns(conn, conversation_id))
                .await?;
            turns
                .iter()
                .rev()
                .find(|turn| turn.status.is_finished())
                .map(|turn| turn.id)
                .ok_or(DaemonError::NoFinishedTurn { conversation_id })?
        }
    };
    let found = state
        .snapshots
        .turn_diff(conversation_id, Some(turn_id), !params.stat, MAX_TURN_DIFF_LINES)
        .await
        .map_err(|source| DaemonError::Snapshot { source })?;
    let result = match found {
        Some(found) => ConversationDiffResult {
            turn_id: found.turn_id,
            changes: found.changes,
            diff: found.diff,
        },
        None => ConversationDiffResult {
            turn_id,
            changes: efr_protocol::FileChanges::default(),
            diff: (!params.stat).then(String::new),
        },
    };
    responder.item(&result).await?;
    Ok(())
}
