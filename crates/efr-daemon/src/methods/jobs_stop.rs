//! `jobs.stop`: stop one background job (`efr jobs stop`, the stop key of the view).
//!
//! The conversation is the one that the params name, else the active conversation of
//! the terminal that the connection's `hello` named. The method needs `operate`, which a
//! model-side peer never holds: the model stops its jobs with the `job_stop` tool.
//! Until the job runtime exists, no job ever starts, so every job is `not_found`.

use efr_protocol::JobsStop;
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::methods::prompt_send::ensure_exists;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: JobsStop,
    _responder: &Responder,
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
    ensure_exists(state, conversation_id).await?;
    Err(DaemonError::JobNotFound { conversation_id, job_id: params.job_id })
}
