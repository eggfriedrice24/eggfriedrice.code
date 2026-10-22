//! `conversations.search`: the conversations whose titles or text match some words, one
//! row per conversation, best first, for the picker of `,resume`, `efr search` and the
//! phone.
//!
//! An empty query is the list of `conversations.recent` in its order (the scope of
//! `pwd` first, newest first), and only it reads the place of `pwd`; a query never does,
//! so a key of the picker never loads and writes the registry files. The store searches
//! and ranks on a search lane (`methods/search.rs` has the caps and the stop); efrd adds
//! what only it knows, which terminal holds each row and its project's name. Every row
//! shows a title with its secrets replaced: the search title, else the projection's title
//! after the index redaction. The debug line of a search holds counts and times, never
//! the query or a text.

use std::collections::HashMap;

use efr_protocol::{
    ConversationId, ConversationsSearch, ConversationsSearchResult, RecentConversation, RouteScope,
    SearchIndexState, SearchPartial, SearchRow, SearchStats,
};
use efr_scope::Registry;
use efr_store::conversations::{self as stored, RecentRow};
use efr_store::search::{self, FoundRow};
use efr_transport::Responder;

use crate::DaemonError;
use crate::engine::load_registry;
use crate::methods::page_size;
use crate::methods::prompt_send::read_place;
use crate::methods::recent_rows::recent_rows;
use crate::methods::search::{
    HARD_CAP, Ran, Run, partial_name, search_error, shown_title, wire_hit,
};
use crate::routing;
use crate::state::State;

/// Rows when the request names no limit: what the picker asks for.
const DEFAULT_ROWS: u32 = 50;
/// The most rows of one answer: 200 rows of at most 400 characters stay far below the
/// largest frame.
const MAX_ROWS: u32 = 200;

/// What one search found and measured, for its answer and its debug line.
struct Searched {
    result: ConversationsSearchResult,
    terms: usize,
    rows_scanned: u32,
    conversations_scored: u32,
    titles_scored: u32,
}

pub(crate) async fn handle(
    state: &State,
    params: ConversationsSearch,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let ConversationsSearch { query, tty, pwd, limit, stats } = params;
    let limit = page_size(limit, DEFAULT_ROWS, MAX_ROWS);
    let query_chars = query.chars().count();
    let mut run = Run::start(state);
    let searched = if query.trim().is_empty() {
        recent(state, &mut run, tty, pwd, limit).await?
    } else {
        found(state, &mut run, query, tty, limit).await?
    };
    let Searched { mut result, terms, rows_scanned, conversations_scored, titles_scored } =
        searched;
    let elapsed = run.elapsed();
    tracing::debug!(
        query_chars,
        terms,
        rows_scanned,
        rows = result.rows.len(),
        elapsed_ms = elapsed.as_secs_f64() * 1000.0,
        partial = partial_name(result.partial),
        "searched the conversations"
    );
    if stats {
        result.stats = Some(SearchStats {
            rows_scanned,
            conversations_scored,
            titles_scored,
            elapsed_us: u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
        });
    }
    responder.item(&result).await?;
    Ok(())
}

/// The terminal's current conversation.
fn current(state: &State, tty: Option<&str>) -> Option<ConversationId> {
    tty.and_then(|tty| state.conversations.current(tty)).map(|(_, slot)| slot.conversation_id)
}

/// The empty query: the rows of `conversations.recent`, with the titles of search.
async fn recent(
    state: &State,
    run: &mut Run,
    tty: Option<String>,
    pwd: Option<std::path::PathBuf>,
    limit: u32,
) -> Result<Searched, DaemonError> {
    let (registry, resolved) = read_place(state, pwd.clone()).await;
    let key = pwd.as_deref().map(|pwd| routing::route_key(&registry, pwd, resolved.as_deref()));
    let scope =
        pwd.as_deref().zip(key.as_ref()).map(|(pwd, key)| routing::label(&registry, key, pwd));
    let stop = run.stop();
    let started = run.started();
    let work = state.search_lanes.with(&stop, HARD_CAP, move |conn| {
        started.now();
        let rows = stored::recent(conn, key.as_ref(), limit)?;
        let ids: Vec<ConversationId> = rows.iter().map(|row| row.summary.id).collect();
        let titles = search::titles(conn, &ids)?;
        let short_ids = stored::short_ids(conn, &ids)?;
        let percent = search::index_percent(conn)?;
        Ok((rows, titles, short_ids, percent))
    });
    let current = current(state, tty.as_deref());
    let mut result = ConversationsSearchResult { scope, current, ..Default::default() };
    match run.finish(work).await? {
        Ran::TimeCap => result.partial = Some(SearchPartial::TimeCap),
        Ran::Done((rows, titles, short_ids, percent)) => {
            let conversations = recent_rows(state, rows, &registry, tty.as_deref());
            result.rows = conversations
                .into_iter()
                .zip(titles)
                .zip(short_ids)
                .map(|((conversation, title), short_id)| SearchRow {
                    conversation: titled(conversation, title),
                    short_id,
                    title_ranges: Vec::new(),
                    hit: None,
                })
                .collect();
            result.index = percent.map(|percent| SearchIndexState { percent });
        }
    }
    Ok(Searched { result, terms: 0, rows_scanned: 0, conversations_scored: 0, titles_scored: 0 })
}

/// A query: the rows that the store found and ranked.
async fn found(
    state: &State,
    run: &mut Run,
    query: String,
    tty: Option<String>,
    limit: u32,
) -> Result<Searched, DaemonError> {
    // NOTE: the limits of a query fail before the search takes a lane.
    search::check_query(&query).map_err(search_error)?;
    let terms = search::query::parse(&query).map_or(0, |parsed| parsed.terms.len());
    let stop = run.stop();
    let started = run.started();
    let lane_stop = stop.clone();
    let work = state.search_lanes.with(&stop, HARD_CAP, move |conn| {
        started.now();
        let found = search::search(conn, &query, limit, &lane_stop)?;
        let ids: Vec<ConversationId> = found.rows.iter().map(|row| row.conversation_id).collect();
        let rows = stored::rows(conn, &ids)?;
        let short_ids = stored::short_ids(conn, &ids)?;
        let percent = search::index_percent(conn)?;
        Ok((found, rows, short_ids, percent))
    });
    let current = current(state, tty.as_deref());
    let mut searched = Searched {
        result: ConversationsSearchResult { current, ..Default::default() },
        terms,
        rows_scanned: 0,
        conversations_scored: 0,
        titles_scored: 0,
    };
    let (found, rows, short_ids, percent) = match run.finish(work).await? {
        Ran::TimeCap => {
            searched.result.partial = Some(SearchPartial::TimeCap);
            return Ok(searched);
        }
        Ran::Done(done) => done,
    };
    searched.rows_scanned = found.rows_scanned;
    searched.conversations_scored = found.conversations_scored;
    searched.titles_scored = found.titles_scored;
    searched.result.partial = found.partial;
    searched.result.index = percent.map(|percent| SearchIndexState { percent });
    let mut stored: HashMap<ConversationId, RecentRow> =
        rows.into_iter().map(|row| (row.summary.id, row)).collect();
    // NOTE: only a project's name needs the registry: one read of the file, with no
    // write of the plugin's copy and no resolve of a directory.
    let registry = if stored.values().any(|row| matches!(row.route, RouteScope::Project(_))) {
        load_registry(&state.engine_parts.registry).await
    } else {
        Registry::empty()
    };
    let (matched, ordered): (Vec<(FoundRow, String)>, Vec<RecentRow>) = found
        .rows
        .into_iter()
        .zip(short_ids)
        .filter_map(|(row, short_id)| {
            stored.remove(&row.conversation_id).map(|stored| ((row, short_id), stored))
        })
        .unzip();
    let conversations = recent_rows(state, ordered, &registry, tty.as_deref());
    searched.result.rows = conversations
        .into_iter()
        .zip(matched)
        .map(|(conversation, (row, short_id))| {
            // NOTE: the ranges point into the search title; a row without one shows
            // the projection's title after the redaction, with no ranges.
            let title_ranges = if row.title.is_some() { row.title_ranges } else { Vec::new() };
            SearchRow {
                conversation: titled(conversation, row.title),
                short_id,
                title_ranges,
                hit: row.hit.map(wire_hit),
            }
        })
        .collect();
    Ok(searched)
}

/// `conversation` with the title that a search row shows.
fn titled(
    mut conversation: RecentConversation,
    search_title: Option<String>,
) -> RecentConversation {
    conversation.summary.title = shown_title(search_title, conversation.summary.title.as_deref());
    conversation
}
