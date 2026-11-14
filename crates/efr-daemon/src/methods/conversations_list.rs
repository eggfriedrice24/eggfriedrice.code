//! `conversations.list`: the conversations, most recently updated first, a page at a
//! time. With `tty`, only the conversations that the terminal holds, one per scope
//! key, its current one first, in one page.
//!
//! Each row has the title that a row of `,resume` shows (`search::shown_title`), never
//! the raw title of the projection: `efr resume` prints these rows for an id start
//! that names more than one conversation.

use efr_protocol::{ConversationsList, ConversationsListResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::methods::search::summaries_titled;
use crate::methods::{cursor, page_size, parse_cursor};
use crate::state::State;

const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 200;

pub(crate) async fn handle(
    state: &State,
    params: ConversationsList,
    responder: &Responder,
) -> Result<(), DaemonError> {
    if let Some(tty) = params.tty {
        let (rows, titles) = state
            .readers
            .with(move |conn| {
                let rows = efr_store::conversations::of_tty(conn, &tty)?;
                let ids: Vec<_> = rows.iter().map(|row| row.id).collect();
                Ok((rows, efr_store::search::titles(conn, &ids)?))
            })
            .await?;
        let conversations = summaries_titled(rows, titles);
        responder.item(&ConversationsListResult { conversations, next_cursor: None }).await?;
        return Ok(());
    }
    let limit = page_size(params.limit, DEFAULT_PAGE, MAX_PAGE);
    let before = params.cursor.as_ref().map(parse_cursor).transpose()?;
    // One more than the page tells whether another page follows.
    let (mut rows, mut titles) = state
        .readers
        .with(move |conn| {
            let rows = efr_store::conversations::list(conn, before, limit + 1)?;
            let ids: Vec<_> = rows.iter().map(|row| row.id).collect();
            Ok((rows, efr_store::search::titles(conn, &ids)?))
        })
        .await?;
    let next_cursor = if rows.len() > limit as usize {
        rows.truncate(limit as usize);
        titles.truncate(limit as usize);
        rows.last().map(|last| cursor(last.last_seq))
    } else {
        None
    };
    let conversations = summaries_titled(rows, titles);
    responder.item(&ConversationsListResult { conversations, next_cursor }).await?;
    Ok(())
}
