//! `conversations.recent`: the recent conversations for `,resume` and `efr resume`.
//!
//! The conversations whose scope key is the key of `pwd` come first, then all others,
//! each part newest first by its last activity (`routing.rs` gives the key, as for a
//! prompt). Each row says whether the asking terminal holds it (`here`) and which other
//! open terminal holds it (`open_in`: a slot whose shell runs, or a terminal that
//! follows it; a phone never holds one). A client without a terminal, such as the
//! phone, sends no `tty`.

use efr_protocol::{ConversationsRecent, ConversationsRecentResult};
use efr_transport::Responder;

use crate::DaemonError;
use crate::methods::page_size;
use crate::methods::prompt_send::read_place;
use crate::methods::recent_rows::recent_rows;
use crate::routing;
use crate::state::State;

/// Rows when the request names no limit.
const DEFAULT_ROWS: u32 = 10;
/// The most rows of one answer.
const MAX_ROWS: u32 = 50;

pub(crate) async fn handle(
    state: &State,
    params: ConversationsRecent,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let ConversationsRecent { tty, pwd, limit } = params;
    let limit = page_size(limit, DEFAULT_ROWS, MAX_ROWS);
    let (registry, resolved) = read_place(state, pwd.clone()).await;
    let key = pwd.as_deref().map(|pwd| routing::route_key(&registry, pwd, resolved.as_deref()));
    let scope =
        pwd.as_deref().zip(key.as_ref()).map(|(pwd, key)| routing::label(&registry, key, pwd));
    let first = key.clone();
    let rows = state
        .readers
        .with(move |conn| efr_store::conversations::recent(conn, first.as_ref(), limit))
        .await?;
    let current = tty
        .as_deref()
        .and_then(|tty| state.conversations.current(tty))
        .map(|(_, slot)| slot.conversation_id);
    let conversations = recent_rows(state, rows, &registry, tty.as_deref());
    responder.item(&ConversationsRecentResult { scope, current, conversations }).await?;
    Ok(())
}
