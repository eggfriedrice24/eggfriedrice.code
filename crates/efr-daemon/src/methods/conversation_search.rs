//! `conversation.search`: the places in one conversation that match some words, newest
//! first, for the preview of the picker of `,resume` and for the phone.
//!
//! The store runs one statement over the conversation's range of the index, with the
//! matches marked, on a search lane under the caps of `methods/search.rs`. An empty
//! query shows the newest prompts and the start of the newest answer. The title is the
//! search title, absent before the indexer reached the conversation. `not_found` for a
//! conversation that does not exist.

use efr_protocol::{ConversationSearch, ConversationSearchResult, SearchPartial};
use efr_store::conversations as stored;
use efr_store::search;
use efr_transport::Responder;

use crate::DaemonError;
use crate::methods::page_size;
use crate::methods::search::{HARD_CAP, Ran, Run, partial_name, search_error, wire_hit};
use crate::state::State;

/// Hits when the request names no limit.
const DEFAULT_HITS: u32 = 20;
/// The most hits of one answer.
const MAX_HITS: u32 = 50;

pub(crate) async fn handle(
    state: &State,
    params: ConversationSearch,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let ConversationSearch { conversation_id, query, limit } = params;
    let limit = page_size(limit, DEFAULT_HITS, MAX_HITS);
    let query_chars = query.chars().count();
    let mut run = Run::start(state);
    // NOTE: the limits of a query fail before the search takes a lane. A panic message
    // could quote the query; inside `quiet` efrd's hook shows none.
    let terms = efr_stdx::panic::quiet(|| {
        search::check_query(&query)?;
        Ok(search::query::parse(&query).map_or(0, |parsed| parsed.terms.len()))
    })
    .map_err(search_error)?;
    let stop = run.stop();
    let started = run.started();
    let lane_stop = stop.clone();
    let work = state.search_lanes.with(&stop, HARD_CAP, move |conn| {
        started.now();
        if stored::get(conn, conversation_id)?.is_none() {
            return Ok(None);
        }
        search::search_in(conn, conversation_id, &query, limit, &lane_stop).map(Some)
    });
    let result = match run.finish(work).await? {
        Ran::TimeCap => {
            ConversationSearchResult { partial: Some(SearchPartial::TimeCap), ..Default::default() }
        }
        Ran::Done(None) => return Err(DaemonError::ConversationNotFound { conversation_id }),
        Ran::Done(Some(found)) => ConversationSearchResult {
            title: found.title,
            hits: found.hits.into_iter().map(wire_hit).collect(),
            partial: found.partial,
        },
    };
    tracing::debug!(
        query_chars,
        terms,
        hits = result.hits.len(),
        elapsed_ms = run.elapsed().as_secs_f64() * 1000.0,
        partial = partial_name(result.partial),
        "searched a conversation"
    );
    responder.item(&result).await?;
    Ok(())
}
