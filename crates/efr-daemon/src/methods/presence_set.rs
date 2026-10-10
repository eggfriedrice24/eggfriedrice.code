//! `presence.set`: the user is away from a conversation, or back (`efr away`, `,away`).
//!
//! With a conversation id it sets that conversation. Without one it sets each
//! conversation that the presence table knows: away for those that are present now,
//! present for all of them. Each change is one `presence_changed` with the source
//! `user`, recorded with the receipt, so a retried command answers from it. The method
//! needs `operate`, which a model-side peer never holds: sandboxed code cannot make
//! questions expire.

use efr_protocol::{
    ConversationId, Event, Presence, PresenceSet, PresenceSetResult, PresenceSource,
};
use efr_store::StoreError;
use efr_store::receipts::NewReceipt;
use efr_transport::Responder;

use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "presence.set";

pub(crate) async fn handle(
    state: &State,
    params: PresenceSet,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let PresenceSet { command_id, conversation_id, presence } = params;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(METHOD, receipt)?).await?;
        return Ok(());
    }
    let conversations = match conversation_id {
        Some(conversation_id) => {
            if let Err(error) = ensure_exists(state, conversation_id).await {
                return Err(receipts::refuse(state, command_id, METHOD, error).await);
            }
            vec![conversation_id]
        }
        None if presence == Presence::Away => state.presence.present(),
        None => state.presence.known(),
    };
    let changes = state.presence.set(&conversations, presence);
    let changed: Vec<ConversationId> = changes
        .iter()
        .filter(|(_, event)| {
            matches!(event, Event::PresenceChanged { source: PresenceSource::User, .. })
        })
        .map(|(conversation_id, _)| *conversation_id)
        .collect();
    tracing::info!(presence = presence.as_str(), changed = changed.len(), "presence set");
    let stored = PresenceSetResult { presence, changed, seq: None };
    let value = serde_json::to_value(&stored)
        .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?;
    let batch = crate::presence::batch(changes)
        .unwrap_or_default()
        .receipt(NewReceipt::accepted(command_id, METHOD, value));
    let answer = match state.writer.append(batch).await {
        Ok(committed) => {
            let seq = committed.events().last().map(|envelope| envelope.seq);
            serde_json::to_value(PresenceSetResult { seq, ..stored })
                .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?
        }
        Err(StoreError::DuplicateCommand { receipt }) => receipts::replay(METHOD, *receipt)?,
        Err(source) => return Err(DaemonError::Store { source }),
    };
    responder.item(&answer).await?;
    Ok(())
}
