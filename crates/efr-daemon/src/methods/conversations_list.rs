//! `conversations.list`: the conversations, most recently updated first, a page at a
//! time.

use efr_protocol::{ConversationsList, ConversationsListResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::methods::{cursor, page_size, parse_cursor};
use crate::state::State;

const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 200;

pub(crate) async fn handle(
    state: &State,
    params: ConversationsList,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let limit = page_size(params.limit, DEFAULT_PAGE, MAX_PAGE);
    let before = params.cursor.as_ref().map(parse_cursor).transpose()?;
    // One more than the page tells whether another page follows.
    let mut conversations = state
        .readers
        .with(move |conn| efr_store::conversations::list(conn, before, limit + 1))
        .await?;
    let next_cursor = if conversations.len() > limit as usize {
        conversations.truncate(limit as usize);
        conversations.last().map(|last| cursor(last.last_seq))
    } else {
        None
    };
    responder.item(&ConversationsListResult { conversations, next_cursor }).await?;
    Ok(())
}
