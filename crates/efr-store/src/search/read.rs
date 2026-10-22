//! The reads of conversation search: the list of conversations that match a query
//! ([`search`]), the hits in one conversation for the preview ([`search_in`]), the
//! search titles ([`titles`]) and how far the index is ([`index_percent`]).
//!
//! They run on a search lane (`SearchLanes::with`) and check the [`SearchStop`] between
//! rows. The list reads at most [`ROW_CAP`] rows of the full text index, newest first
//! with no sort, and at most [`TITLE_CAP`] titles of the newest conversations; it ranks
//! in Rust (the `rank` module) and never with `ORDER BY rank`, which would score and
//! sort every match before the first row. Its snippets come from the docs by sequence
//! number (the `snippet` module), so it runs no FTS5 statement per row. The preview runs
//! one FTS5 statement over the range of the conversation's events, with `highlight()`.

use std::collections::HashMap;
use std::ops::Range;

use efr_protocol::{ConversationId, SearchKind, SearchLine, SearchPartial, Seq, TextRange, TurnId};
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, params};

use super::fuzzy::{FuzzyMatch, Scorer};
use super::lane::VOCABULARY;
use super::query::{self, Query, QueryError};
use super::snippet::{self, Needle};
use super::{SearchStop, marks, rank, state};
use crate::{StoreError, sql};

/// The most rows of the full text index that one search reads, newest first.
pub const ROW_CAP: u32 = 10_000;

/// The most titles that one search scores: those of the newest conversations.
pub const TITLE_CAP: u32 = 10_000;

/// The events after the cursor that [`index_percent`] still calls a complete index:
/// the events of the newest turns, which the indexer takes within a second of a turn's
/// end, or of a turn that still runs.
pub const LIVE_LAG: u64 = 5_000;

/// The most words that a prefix expands to; a prefix of more words stays a prefix.
pub const EXPAND_WORDS: u32 = 64;

/// The newest prompts that the preview of an empty query shows.
const EMPTY_PROMPTS: u32 = 3;

/// The rows of the full text index, newest first, with what the rank needs. FTS5 takes
/// `ORDER BY rowid DESC` and the `LIMIT` natively, so there is no sort.
const FTS_SQL: &str = "SELECT rowid, conversation_id, kind FROM search_fts \
                       WHERE search_fts MATCH ?1 ORDER BY rowid DESC LIMIT ?2";

/// The `bm25()` of the same rows in the same order. `bm25()` counts every match of
/// each term before its first row, so it runs only when that is cheap (the plan's DQ21
/// fallback, widened): a query below the row cap with no long prefix left after
/// [`expression`]. A long prefix makes FTS5 merge the doclists of every word that
/// starts with it, once for the rows and again for each count of `bm25()`. Without
/// `bm25()` a hit counts by its kind, and recency decides.
const BM25_SQL: &str = "SELECT bm25(search_fts) FROM search_fts \
                        WHERE search_fts MATCH ?1 ORDER BY rowid DESC LIMIT ?2";

/// The titles of the newest conversations, through `conversations_updated`, so no sort.
const TITLES_SQL: &str = "SELECT t.conversation_id, t.title FROM conversations AS c \
                          JOIN search_titles AS t ON t.conversation_id = c.id \
                          ORDER BY c.updated_at DESC, c.last_seq DESC LIMIT ?1";

/// The hits of one conversation for the preview: one FTS5 scan of the range of its
/// events, with the matches marked.
const PREVIEW_SQL: &str = "SELECT rowid, kind, highlight(search_fts, 0, char(2), char(3)) \
                           FROM search_fts WHERE search_fts MATCH ?1 \
                           AND rowid BETWEEN ?2 AND ?3 AND conversation_id = ?4 \
                           ORDER BY rowid DESC LIMIT ?5";

/// What [`search`] found: the rows, best first, and the measures of the search.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    /// The conversations, best first.
    pub rows: Vec<FoundRow>,
    /// Why the search stopped before it saw every match; `None` when it saw all.
    pub partial: Option<SearchPartial>,
    /// The rows of the full text index that the search read.
    pub rows_scanned: u32,
    /// The conversations that those rows belong to.
    pub conversations_scored: u32,
    /// The titles that the search compared with the query.
    pub titles_scored: u32,
    /// True when the text hits ranked with `bm25()`; false when the query reached the
    /// row cap or ends with a prefix longer than the prefix index, so each hit counts
    /// by its kind and recency decides.
    pub bm25: bool,
}

/// One conversation that [`search`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundRow {
    /// The conversation.
    pub conversation_id: ConversationId,
    /// Its search title, from the indexed text of its first prompt; `None` before the
    /// indexer reached it.
    pub title: Option<String>,
    /// The parts of `title` that match the query.
    pub title_ranges: Vec<TextRange>,
    /// Its best hit in the text; `None` for a match in the title only.
    pub hit: Option<FoundHit>,
}

/// One doc that matches: a prompt, an answer or a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundHit {
    /// What the doc is. A steer is a prompt.
    pub kind: SearchKind,
    /// The sequence number of its event.
    pub seq: Seq,
    /// The turn of its event.
    pub turn_id: Option<TurnId>,
    /// When its event happened.
    pub at: Timestamp,
    /// One snippet line in the list; the lines of the hit in the preview.
    pub lines: Vec<SearchLine>,
}

/// What [`search_in`] found in one conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoundIn {
    /// The search title with the parts that match; `None` before the indexer reached
    /// the conversation.
    pub title: Option<SearchLine>,
    /// The hits, newest first.
    pub hits: Vec<FoundHit>,
    /// Why the search stopped before it saw every match.
    pub partial: Option<SearchPartial>,
}

/// Checks the limits of a query (the most characters and words) before a search
/// takes a lane.
pub fn check_query(query: &str) -> Result<(), StoreError> {
    parse(query).map(|_| ())
}

fn parse(text: &str) -> Result<Query, StoreError> {
    query::parse(text).map_err(|error| match error {
        QueryError::TooLong { chars } => {
            StoreError::QueryTooLong { chars, limit: query::MAX_CHARS }
        }
        QueryError::TooManyTerms { terms } => {
            StoreError::QueryTooManyTerms { terms, limit: query::MAX_TERMS }
        }
    })
}

/// The best hit of one conversation.
#[derive(Debug, Clone, Copy)]
struct Best {
    score: f64,
    seq: i64,
}

/// The conversations whose titles or text match `query`, best first, at most `limit`.
/// An empty query finds nothing: the list of recent conversations is
/// `conversations::recent`.
///
/// Every term must match in one doc (the FTS5 expression of the `query` module); the
/// title scorer matches every word of the query in the title. The search reads at most
/// [`ROW_CAP`] rows of the index, newest first, and the titles of the [`TITLE_CAP`]
/// newest conversations. A stop ends the reading at the next row; the rows seen are
/// ranked and `partial` says why it ended.
pub fn search(
    conn: &Connection,
    query: &str,
    limit: u32,
    stop: &SearchStop,
) -> Result<Found, StoreError> {
    let parsed = parse(query)?;
    let mut found = Found::default();
    if parsed.is_empty() || limit == 0 {
        return Ok(found);
    }
    let mut texts: HashMap<String, Best> = HashMap::new();
    if let Some(Expression { text: expression, long_prefix }) = expression(conn, &parsed)? {
        let expression = &expression;
        let (names, rows) = text_rows(conn, expression, stop, &mut found)?;
        let scores = if !long_prefix && found.partial.is_none() && found.rows_scanned < ROW_CAP {
            bm25_scores(conn, expression, stop, rows.len())?
        } else {
            None
        };
        found.bm25 = scores.is_some();
        let mut best: Vec<Option<Best>> = vec![None; names.len()];
        for (index, (seq, conversation, kind)) in rows.into_iter().enumerate() {
            let bm25 = scores.as_ref().and_then(|scores| scores.get(index).copied());
            let score = rank::hit_score(kind, bm25);
            // Rows come newest first, so a tie keeps the newer hit.
            let slot = &mut best[conversation];
            if slot.is_none_or(|best| score > best.score) {
                *slot = Some(Best { score, seq });
            }
        }
        texts.extend(names.into_iter().zip(best).filter_map(|(name, best)| Some((name, best?))));
    }
    found.conversations_scored = u32::try_from(texts.len()).unwrap_or(u32::MAX);

    let scorer = Scorer::new(&parsed.words());
    let mut titled: HashMap<String, (f64, String, FuzzyMatch)> = HashMap::new();
    if found.partial != Some(SearchPartial::TimeCap) {
        let mut stmt = conn.prepare(TITLES_SQL)?;
        let mut rows = stmt.query([TITLE_CAP])?;
        loop {
            if stop.is_stopped() {
                found.partial = Some(SearchPartial::TimeCap);
                break;
            }
            let Some(row) = rows.next()? else { break };
            found.titles_scored += 1;
            let title: String = row.get(1)?;
            if let Some(matched) = scorer.score(&title) {
                let relative = scorer.relative(&matched);
                if relative > 0.0 {
                    titled.insert(row.get(0)?, (relative, title, matched));
                }
            }
        }
    }

    let mut keys: Vec<&String> = texts.keys().chain(titled.keys()).collect();
    keys.sort_unstable();
    keys.dedup();
    let candidates = keys
        .into_iter()
        .map(|key| rank::Candidate {
            key: key.clone(),
            text: texts.get(key).map(|best| best.score),
            title: titled.get(key).map_or(0.0, |(relative, _, _)| *relative),
        })
        .collect();
    let ranked = rank::buckets(candidates);
    let reach = rank::reach(&ranked, limit as usize);
    let mut recency =
        conn.prepare("SELECT updated_at, last_seq FROM conversations WHERE id = ?1")?;
    let mut ordered = Vec::with_capacity(reach);
    for candidate in ranked.into_iter().take(reach) {
        let newest: Option<(i64, i64)> =
            recency.query_row([&candidate.key], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        if let Some(newest) = newest {
            ordered.push((candidate.bucket, newest, candidate.key));
        }
    }
    ordered.sort_by(rank::order);
    ordered.truncate(limit as usize);

    let needles = needles(&parsed);
    let mut docs = Docs::prepare(conn)?;
    for (_, _, key) in ordered {
        let (title, title_ranges) = match titled.remove(&key) {
            Some((_, title, matched)) => (Some(title), matched.ranges),
            None => {
                let title = docs.title(&key)?;
                let ranges = title
                    .as_deref()
                    .and_then(|title| scorer.score(title))
                    .map(|matched| matched.ranges)
                    .unwrap_or_default();
                (title, ranges)
            }
        };
        let hit = match texts.get(&key) {
            Some(best) => docs.snippet_hit(best.seq, &needles)?,
            None => None,
        };
        found.rows.push(FoundRow {
            conversation_id: sql::parse(&key, "search_docs", "conversation_id")?,
            title,
            title_ranges: text_ranges(&title_ranges),
            hit,
        });
    }
    Ok(found)
}

/// The places in the conversation `conversation_id` that match `query`, newest first,
/// at most `limit`, each with the lines that hold a match and one line of context
/// around them. With an empty query: the newest prompts and the start of the newest
/// answer, with no ranges.
pub fn search_in(
    conn: &Connection,
    conversation_id: ConversationId,
    query: &str,
    limit: u32,
    stop: &SearchStop,
) -> Result<FoundIn, StoreError> {
    let parsed = parse(query)?;
    let conversation = conversation_id.to_string();
    let mut docs = Docs::prepare(conn)?;
    let mut found = FoundIn::default();
    if let Some(title) = docs.title(&conversation)? {
        let ranges = if parsed.is_empty() {
            Vec::new()
        } else {
            Scorer::new(&parsed.words())
                .score(&title)
                .map(|matched| matched.ranges)
                .unwrap_or_default()
        };
        found.title = Some(SearchLine { text: title, ranges: text_ranges(&ranges) });
    }
    if limit == 0 {
        return Ok(found);
    }
    if parsed.is_empty() {
        found.hits = newest(conn, &mut docs, &conversation, limit)?;
        return Ok(found);
    }
    let Some(Expression { text: expression, .. }) = expression(conn, &parsed)? else {
        return Ok(found);
    };
    let span: (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT MIN(seq), MAX(seq) FROM events WHERE conversation_id = ?1",
        [&conversation],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let (Some(first), Some(last)) = span else {
        return Ok(found);
    };
    let mut stmt = conn.prepare(PREVIEW_SQL)?;
    let mut rows = stmt.query(params![expression, first, last, conversation, limit])?;
    let mut read = 0;
    loop {
        if stop.is_stopped() {
            found.partial = Some(SearchPartial::TimeCap);
            break;
        }
        let Some(row) = rows.next()? else { break };
        read += 1;
        let seq: i64 = row.get(0)?;
        let kind: i64 = row.get(1)?;
        let marked: String = row.get(2)?;
        let (text, ranges) = marks::strip(&marked);
        let lines = snippet::preview(marks::lines(&text, &ranges));
        found.hits.extend(docs.hit(seq, kind, lines)?);
        stop.row(read);
    }
    Ok(found)
}

/// The FTS5 expression of a query.
struct Expression {
    text: String,
    /// True when it still holds a prefix that no prefix index serves.
    long_prefix: bool,
}

/// The FTS5 expression of `parsed`, with a long prefix expanded to the words of the
/// index that start with it, when the connection has the vocabulary and they are at
/// most [`EXPAND_WORDS`]. FTS5 merges the doclists of every word of a prefix into
/// memory before the first row, which for a common prefix costs megabytes and tens of
/// milliseconds; an `OR` of the same words reads them lazily, newest first, and stops
/// at the row cap. The rows are the same either way. `None` when no term is indexed.
fn expression(conn: &Connection, parsed: &Query) -> Result<Option<Expression>, StoreError> {
    if parsed.fts.is_none() {
        return Ok(None);
    }
    let has_vocabulary = parsed.terms.iter().any(|term| term.long_prefix().is_some())
        && conn
            .query_row("SELECT 1 FROM temp.sqlite_master WHERE name = ?1", [VOCABULARY], |_| Ok(()))
            .optional()?
            .is_some();
    let mut parts = Vec::new();
    let mut long_prefix = false;
    for term in parsed.terms.iter().filter(|term| term.indexed) {
        let words = match term.long_prefix() {
            Some(prefix) if has_vocabulary => prefix_words(conn, &prefix)?,
            Some(_) => None,
            None => {
                parts.push(term.fts());
                continue;
            }
        };
        match words {
            Some(words) if !words.is_empty() => {
                let quoted: Vec<String> = words.iter().map(|word| query::quote(word)).collect();
                parts.push(format!("({})", quoted.join(" OR ")));
            }
            _ => {
                long_prefix = true;
                parts.push(term.fts());
            }
        }
    }
    Ok(Some(Expression { text: parts.join(" AND "), long_prefix }))
}

/// The words of the index that start with `prefix` (ASCII, lower case), when they are
/// at most [`EXPAND_WORDS`].
fn prefix_words(conn: &Connection, prefix: &str) -> Result<Option<Vec<String>>, StoreError> {
    // The first string after every string that starts with the prefix: its last
    // character plus one. The prefix is ASCII letters and digits, so that is ASCII too.
    let mut end = prefix.as_bytes().to_vec();
    if let Some(last) = end.last_mut() {
        *last = last.saturating_add(1);
    }
    let end = String::from_utf8_lossy(&end).into_owned();
    let mut stmt = conn.prepare(&format!(
        "SELECT term FROM temp.{VOCABULARY} WHERE term >= ?1 AND term < ?2 ORDER BY term LIMIT ?3"
    ))?;
    let words: Vec<String> = stmt
        .query_map(params![prefix, end, EXPAND_WORDS + 1], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok((words.len() <= EXPAND_WORDS as usize).then_some(words))
}

/// One row of the full text index: its seq, the index of its conversation in the
/// names of [`text_rows`], and the kind code of its doc.
type TextRow = (i64, usize, i64);

/// The conversations of the rows, and the rows of the full text index for
/// `expression`, newest first, at most [`ROW_CAP`]: (seq, the index of the
/// conversation, kind). A stop ends them at the next row.
fn text_rows(
    conn: &Connection,
    expression: &str,
    stop: &SearchStop,
    found: &mut Found,
) -> Result<(Vec<String>, Vec<TextRow>), StoreError> {
    let mut stmt = conn.prepare(FTS_SQL)?;
    let mut rows = stmt.query(params![expression, ROW_CAP])?;
    let mut out = Vec::new();
    // NOTE: the conversations of the rows repeat; one string per conversation keeps a
    // read of 10,000 rows to a few allocations.
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    loop {
        if stop.is_stopped() {
            found.partial = Some(SearchPartial::TimeCap);
            break;
        }
        let Some(row) = rows.next()? else { break };
        found.rows_scanned += 1;
        let conversation = row.get_ref(1)?.as_str().map_err(rusqlite::Error::from)?;
        let at = match index.get(conversation) {
            Some(&at) => at,
            None => {
                index.insert(conversation.to_owned(), names.len());
                names.push(conversation.to_owned());
                names.len() - 1
            }
        };
        out.push((row.get(0)?, at, row.get(2)?));
        stop.row(found.rows_scanned);
    }
    if found.partial.is_none() && found.rows_scanned >= ROW_CAP {
        found.partial = Some(SearchPartial::RowCap);
    }
    Ok((names, out))
}

/// The `bm25()` of the `count` rows of [`text_rows`], in their order; `None` when a
/// stop ended the read or the count differs.
fn bm25_scores(
    conn: &Connection,
    expression: &str,
    stop: &SearchStop,
    count: usize,
) -> Result<Option<Vec<f64>>, StoreError> {
    let mut stmt = conn.prepare(BM25_SQL)?;
    let mut rows = stmt.query(params![expression, ROW_CAP])?;
    let mut scores = Vec::with_capacity(count);
    loop {
        if stop.is_stopped() {
            return Ok(None);
        }
        let Some(row) = rows.next()? else { break };
        scores.push(row.get(0)?);
    }
    Ok((scores.len() == count).then_some(scores))
}

/// The newest prompts and the newest answer of a conversation, newest first.
fn newest(
    conn: &Connection,
    docs: &mut Docs<'_>,
    conversation: &str,
    limit: u32,
) -> Result<Vec<FoundHit>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT d.seq, d.kind, d.text FROM events AS e JOIN search_docs AS d ON d.seq = e.seq \
         WHERE e.conversation_id = ?1 AND e.kind IN (?2, ?3) ORDER BY e.seq DESC LIMIT ?4",
    )?;
    let mut picked: Vec<(i64, i64, String)> = Vec::new();
    for (kinds, count) in [
        (("prompt_queued", "turn_steered"), EMPTY_PROMPTS),
        (("assistant_message_completed", "assistant_message_completed"), 1),
    ] {
        let rows = stmt.query_map(params![conversation, kinds.0, kinds.1, count], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        for row in rows {
            picked.push(row?);
        }
    }
    picked.sort_by_key(|(seq, _, _)| std::cmp::Reverse(*seq));
    picked.truncate(limit as usize);
    let mut hits = Vec::with_capacity(picked.len());
    for (seq, kind, text) in picked {
        let lines = text.split('\n').map(|line| (line.to_owned(), Vec::new())).collect();
        hits.extend(docs.hit(seq, kind, snippet::preview(lines))?);
    }
    Ok(hits)
}

/// The search title of each conversation of `ids`, in order; `None` for one that the
/// indexer has not reached or that has no title.
pub fn titles(
    conn: &Connection,
    ids: &[ConversationId],
) -> Result<Vec<Option<String>>, StoreError> {
    let mut docs = Docs::prepare(conn)?;
    ids.iter().map(|id| docs.title(&id.to_string())).collect()
}

/// How far the index is, from 0 to 99, while the indexer is more than [`LIVE_LAG`]
/// events behind the log, as after an upgrade or a change of the redaction; `None`
/// when the index is complete or nearly so.
pub fn index_percent(conn: &Connection) -> Result<Option<u8>, StoreError> {
    let current = state(conn)?;
    let (indexed, newest) = (current.indexed_seq.get(), current.newest_seq.get());
    if newest.saturating_sub(indexed) <= LIVE_LAG {
        return Ok(None);
    }
    let percent = u128::from(indexed) * 100 / u128::from(newest.max(1));
    Ok(Some(u8::try_from(percent.min(99)).unwrap_or(99)))
}

/// The needles of the snippet: the terms that the index searched for.
fn needles(parsed: &Query) -> Vec<Needle> {
    parsed
        .terms
        .iter()
        .filter(|term| term.indexed)
        .map(|term| Needle::new(&term.tokens, term.prefix))
        .collect()
}

/// The kind of a hit from the kind code of its doc.
fn search_kind(kind: i64) -> SearchKind {
    match kind {
        0 | 1 => SearchKind::Prompt,
        2 => SearchKind::Answer,
        3 => SearchKind::Command,
        _ => SearchKind::Unknown,
    }
}

fn text_ranges(ranges: &[Range<u32>]) -> Vec<TextRange> {
    ranges.iter().map(|range| TextRange { start: range.start, end: range.end }).collect()
}

/// The primary key reads of a search, prepared once.
struct Docs<'c> {
    title: rusqlite::Statement<'c>,
    doc: rusqlite::Statement<'c>,
    event: rusqlite::Statement<'c>,
}

impl<'c> Docs<'c> {
    fn prepare(conn: &'c Connection) -> Result<Self, StoreError> {
        Ok(Docs {
            title: conn.prepare("SELECT title FROM search_titles WHERE conversation_id = ?1")?,
            doc: conn.prepare("SELECT kind, text FROM search_docs WHERE seq = ?1")?,
            event: conn.prepare("SELECT turn_id, created_at FROM events WHERE seq = ?1")?,
        })
    }

    fn title(&mut self, conversation: &str) -> Result<Option<String>, StoreError> {
        Ok(self.title.query_row([conversation], |row| row.get(0)).optional()?)
    }

    /// The hit of the doc `seq` with its snippet line; `None` when the doc is gone.
    fn snippet_hit(
        &mut self,
        seq: i64,
        needles: &[Needle],
    ) -> Result<Option<FoundHit>, StoreError> {
        let doc: Option<(i64, String)> =
            self.doc.query_row([seq], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        let Some((kind, text)) = doc else {
            return Ok(None);
        };
        let line = snippet::snippet(&text, needles);
        self.hit(seq, kind, vec![line])
    }

    /// A hit of the doc `seq` with `lines`, and its turn and time from its event;
    /// `None` when the log has no such event.
    fn hit(
        &mut self,
        seq: i64,
        kind: i64,
        lines: Vec<(String, Vec<Range<u32>>)>,
    ) -> Result<Option<FoundHit>, StoreError> {
        let event: Option<(Option<String>, i64)> =
            self.event.query_row([seq], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        let Some((turn, at)) = event else {
            return Ok(None);
        };
        Ok(Some(FoundHit {
            kind: search_kind(kind),
            seq: sql::to_seq(seq, "search_docs", "seq")?,
            turn_id: turn.map(|turn| sql::parse(&turn, "events", "turn_id")).transpose()?,
            at: sql::to_timestamp(at, "events", "created_at")?,
            lines: lines
                .into_iter()
                .map(|(text, ranges)| SearchLine { text, ranges: text_ranges(&ranges) })
                .collect(),
        }))
    }
}

#[cfg(test)]
mod tests;
