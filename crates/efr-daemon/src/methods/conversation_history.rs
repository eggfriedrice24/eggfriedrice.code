//! `conversation.history`: a conversation's events, newest page first, each page
//! oldest first.

use efr_protocol::{ConversationHistory, ConversationHistoryResult, ConversationId};
use efr_transport::Responder;

use crate::DaemonError;
use crate::methods::{cursor, page_size, parse_cursor};
use crate::state::State;

const DEFAULT_PAGE: u32 = 100;
const MAX_PAGE: u32 = 500;

pub(crate) async fn handle(
    state: &State,
    params: ConversationHistory,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let conversation_id = params.conversation_id;
    let limit = page_size(params.limit, DEFAULT_PAGE, MAX_PAGE);
    let before = params.cursor.as_ref().map(parse_cursor).transpose()?;
    let page = state
        .readers
        .with(move |conn| {
            let exists = efr_store::conversations::get(conn, conversation_id)?.is_some();
            let events = efr_store::events::read_conversation_before(
                conn,
                conversation_id,
                before,
                limit + 1,
            )?;
            Ok(exists.then_some(events))
        })
        .await?;
    let Some(mut events) = page else {
        return Err(not_found(conversation_id));
    };
    // The extra, oldest event only tells whether an older page exists.
    let next_cursor = if events.len() > limit as usize {
        events.remove(0);
        events.first().map(|first| cursor(first.seq))
    } else {
        None
    };
    responder.item(&ConversationHistoryResult { events, next_cursor }).await?;
    Ok(())
}

fn not_found(conversation_id: ConversationId) -> DaemonError {
    DaemonError::ConversationNotFound { conversation_id }
}
