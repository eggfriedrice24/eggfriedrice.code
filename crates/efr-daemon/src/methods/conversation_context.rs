//! `conversation.context`: what fills the model's context of a conversation (`efr
//! context`, `,context`).
//!
//! The conversation's actor builds the request that the next model call would send and
//! measures it part by part, also while a turn runs; it records nothing. A conversation
//! without a live actor gets one, which reads the history from the log; a conversation
//! that does not exist is `not_found`. While the model catalog has no list yet, it
//! first waits for one fetch, as a prompt does, so the window is the model's own. The
//! rules of the context are in the README of efr-conversation, section "Context".

use efr_protocol::ConversationContext;
use efr_transport::Responder;

use crate::DaemonError;
use crate::methods::prompt_send::ensure_exists;
use crate::state::State;

const METHOD: &str = "conversation.context";

pub(crate) async fn handle(
    state: &State,
    params: ConversationContext,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let conversation_id = params.conversation_id;
    state.providers.models().ready().await;
    let handle = match state.conversations.live(conversation_id) {
        Some(handle) => handle,
        None => {
            ensure_exists(state, conversation_id).await?;
            state.conversations.open(conversation_id)
        }
    };
    let result = handle.context(params).await?;
    let answer = serde_json::to_value(result)
        .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?;
    responder.item(&answer).await?;
    Ok(())
}
