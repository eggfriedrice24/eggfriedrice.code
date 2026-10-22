//! The search index of the conversations (migration 9).
//!
//! efrd's indexer fills it in the background, in rounds: [`pending`] reads the source
//! text after the cursor on a read-only connection, efrd bounds, cleans and redacts
//! that text, and [`WriterHandle::search_apply`](crate::WriterHandle::search_apply)
//! writes the docs, the titles, the drops and the new cursor in one transaction. The
//! store never decides what text goes in: it cannot redact (it may not reach
//! `efr-conversation`), so the migration creates the tables empty and the indexer
//! backfills them from the log.
//!
//! The tables are not projections: a rebuild of the projections leaves them alone.
//! The row id of a doc is the sequence number of its event, and one event gives at
//! most one doc. Indexed are the text of `prompt_queued` (a prompt), of `turn_steered`
//! (a steer), of `assistant_message_completed` (an answer) and the command line of a
//! `tool_call_started` of the tool `shell` (a command). `prompt_withdrawn`,
//! `steering_withdrawn` and the steers that a `prompt_queued` sends again take docs
//! back. Nothing else is indexed: no output, no diff, no file contents.
//!
//! The index is written only by its own writer jobs. The batch that ends a turn writes
//! nothing here, so the index adds no work to a turn's commit. The jobs run no
//! checkpoint of their own: one costs about 4 to 18 ms, longer than the job, so the
//! write-ahead log keeps SQLite's own checkpoint at 1,000 frames, in whatever commit
//! crosses it.

use efr_protocol::{ConversationId, Seq};
use rusqlite::{Connection, OptionalExtension as _, Statement, params};

use crate::conversations::title_from;
use crate::{StoreError, events, sql};

/// The most docs that one page of [`pending`] returns. With [`PAGE_BYTES`], set from
/// the bench of a backfill page on corpus L: a page of 32 docs keeps the writer time of
/// a page at a p95 of about 2 ms; one of 64 docs makes twice the pages that carry
/// SQLite's checkpoint of the write-ahead log.
pub const PAGE_ROWS: u32 = 32;

/// The most text, in bytes, that one page of [`pending`] returns: the page ends after
/// the row that reaches it.
pub const PAGE_BYTES: usize = 32 * 1024;

/// The most events that one page of [`pending`] reads, so a long run of events with no
/// text costs many short reads, not one long one.
pub const PAGE_SCAN: u32 = 20_000;

/// The most characters of one text that leave SQLite: [`pending`] cuts each text in
/// SQL, so a page holds at most [`PAGE_ROWS`] times this much, whatever the event held.
pub const SOURCE_CHARS: u32 = 33_792;

const STATE: &str = "search_state";

/// What a doc of the index holds. Stored as 0 to 3 in `search_docs.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocKind {
    /// The text of a prompt (`prompt_queued`).
    Prompt,
    /// The text of a steer (`turn_steered`).
    Steer,
    /// The text of an answer of the model (`assistant_message_completed`).
    Answer,
    /// The command line of a `shell` call (`tool_call_started`).
    Command,
}

impl DocKind {
    /// The value in the `kind` column.
    pub fn code(self) -> i64 {
        match self {
            DocKind::Prompt => 0,
            DocKind::Steer => 1,
            DocKind::Answer => 2,
            DocKind::Command => 3,
        }
    }

    /// The kind of a `kind` column value; `None` for a value this build does not know.
    pub fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(DocKind::Prompt),
            1 => Some(DocKind::Steer),
            2 => Some(DocKind::Answer),
            3 => Some(DocKind::Command),
            _ => None,
        }
    }
}

/// The text of one event: from the log in [`Pending`], and after efrd's text rules as
/// a doc of a [`SearchPage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRow {
    /// The sequence number of the event, which is the row id of the doc.
    pub seq: Seq,
    /// The conversation of the event.
    pub conversation_id: ConversationId,
    /// What the text is.
    pub kind: DocKind,
    /// The text. In [`Pending`], cut to [`SOURCE_CHARS`] characters in SQL.
    pub text: String,
}

/// One page of the source of the index after a cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    /// The texts, in sequence order.
    pub rows: Vec<SourceRow>,
    /// The docs that events of the page take back: the prompt of a
    /// `prompt_withdrawn`, the steers of a `steering_withdrawn`, and the steers that a
    /// `prompt_queued` sends again, whose text is in the new prompt. A drop always
    /// names an earlier event; one with no doc is no error.
    pub drops: Vec<Seq>,
    /// The cursor after this page: the last event that the page read when a bound
    /// ended it, else the newest event that the read saw. Never past the read's
    /// snapshot.
    pub through: Seq,
}

/// How far the index is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchState {
    /// Every event up to this one is in the index.
    pub indexed_seq: Seq,
    /// The version of the redaction that wrote the index; 0 before any.
    pub redaction: u32,
    /// The newest event of the log.
    pub newest_seq: Seq,
}

/// One round of the indexer: the docs of a [`Pending`] page after efrd's text rules,
/// and the cursor before and after it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchPage {
    /// The cursor that the page was read after; the write is stale unless the index
    /// is still there.
    pub after: Seq,
    /// The cursor after the page ([`Pending::through`]).
    pub through: Seq,
    /// The version of the redaction that made the docs; the write is stale unless the
    /// index has the same.
    pub redaction: u32,
    /// The docs to write. A text that was empty after the text rules has no doc.
    pub docs: Vec<SourceRow>,
    /// The docs to take back ([`Pending::drops`]).
    pub drops: Vec<Seq>,
}

/// What [`WriterHandle::search_apply`](crate::WriterHandle::search_apply) did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchApplied {
    /// The page is in the index and the cursor is at its `through`.
    Applied,
    /// The cursor or the redaction of the index differ from the page's, so the page
    /// wrote nothing; the indexer reads the state again.
    Stale,
}

/// The cursor and the redaction of the index, and the newest event of the log.
pub fn state(conn: &Connection) -> Result<SearchState, StoreError> {
    let (indexed, redaction): (i64, i64) = conn.query_row(
        "SELECT indexed_seq, redaction FROM search_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(SearchState {
        indexed_seq: sql::to_seq(indexed, STATE, "indexed_seq")?,
        redaction: u32::try_from(redaction)
            .map_err(|source| sql::decode_error(STATE, "redaction", source))?,
        newest_seq: events::last_seq(conn)?,
    })
}

/// The source events of the index, with the text that each carries cut in SQL, and
/// the event kinds that take a doc back. Codes 0 to 3 are [`DocKind`]s.
const PENDING_SQL: &str = "\
    SELECT seq, conversation_id, \
        CASE kind \
            WHEN 'prompt_queued' THEN 0 \
            WHEN 'turn_steered' THEN 1 \
            WHEN 'assistant_message_completed' THEN 2 \
            WHEN 'tool_call_started' THEN 3 \
            WHEN 'prompt_withdrawn' THEN 4 \
            ELSE 5 \
        END, \
        CASE \
            WHEN kind IN ('prompt_queued', 'turn_steered', 'assistant_message_completed') \
                AND json_type(payload, '$.text') = 'text' \
                THEN substr(json_extract(payload, '$.text'), 1, ?3) \
            WHEN kind = 'tool_call_started' AND json_extract(payload, '$.tool') = 'shell' \
                AND json_type(payload, '$.input.command') = 'text' \
                THEN substr(json_extract(payload, '$.input.command'), 1, ?3) \
        END, \
        CASE WHEN kind IN ('prompt_queued', 'steering_withdrawn') \
            THEN json_extract(payload, '$.steers') END, \
        CASE WHEN kind = 'prompt_withdrawn' \
            THEN (SELECT queued_seq FROM turns WHERE turns.id = events.turn_id) END \
    FROM events \
    WHERE seq > ?1 AND seq <= ?2 AND conversation_id IS NOT NULL \
        AND kind IN ('prompt_queued', 'turn_steered', 'assistant_message_completed', \
            'tool_call_started', 'prompt_withdrawn', 'steering_withdrawn') \
    ORDER BY seq";

/// The next page of the index's source after `after`, in sequence order: at most
/// [`PAGE_ROWS`] texts, [`PAGE_BYTES`] of text or [`PAGE_SCAN`] events. Only the text
/// fields leave SQLite, each cut to [`SOURCE_CHARS`] characters. Runs inside one read
/// transaction, so `through` never passes what the read saw.
pub fn pending(conn: &Connection, after: Seq) -> Result<Pending, StoreError> {
    let newest = events::last_seq(conn)?;
    let end = after.get().saturating_add(u64::from(PAGE_SCAN)).min(newest.get());
    let mut page = Pending { through: Seq::new(end.max(after.get())), ..Pending::default() };
    if end <= after.get() {
        return Ok(page);
    }
    let mut stmt = conn.prepare(PENDING_SQL)?;
    let mut rows = stmt.query(params![sql::seq(after), sql::seq(Seq::new(end)), SOURCE_CHARS])?;
    let mut bytes = 0;
    while let Some(row) = rows.next()? {
        let seq = sql::to_seq(row.get(0)?, "events", "seq")?;
        let conversation: String = row.get(1)?;
        let code: i64 = row.get(2)?;
        if let Some(kind) = DocKind::from_code(code) {
            if let Some(steers) = row.get::<_, Option<String>>(4)? {
                page.drops.extend(steer_seqs(&steers, seq)?);
            }
            let Some(text) = row.get::<_, Option<String>>(3)? else {
                continue;
            };
            bytes += text.len();
            page.rows.push(SourceRow {
                seq,
                conversation_id: sql::parse(&conversation, "events", "conversation_id")?,
                kind,
                text,
            });
            if page.rows.len() >= PAGE_ROWS as usize || bytes >= PAGE_BYTES {
                page.through = seq;
                break;
            }
        } else if let Some(prompt) = row.get::<_, Option<i64>>(5)? {
            page.drops.push(sql::to_seq(prompt, "turns", "queued_seq")?);
        } else if let Some(steers) = row.get::<_, Option<String>>(4)? {
            page.drops.extend(steer_seqs(&steers, seq)?);
        }
    }
    Ok(page)
}

/// The sequence numbers in the `steers` array of the event `seq`.
fn steer_seqs(json: &str, seq: Seq) -> Result<Vec<Seq>, StoreError> {
    let steers: Vec<u64> =
        serde_json::from_str(json).map_err(|source| StoreError::DecodeEvent { seq, source })?;
    Ok(steers.into_iter().map(Seq::new).collect())
}

/// Writes one page of the indexer in the writer's transaction: the docs, the title of
/// each conversation whose first prompt is among them, the drops, and the cursor.
/// Writes nothing when the cursor or the redaction of the index differ from the page's.
pub(crate) fn apply(conn: &Connection, page: &SearchPage) -> Result<SearchApplied, StoreError> {
    let current = state(conn)?;
    if current.indexed_seq != page.after
        || current.redaction != page.redaction
        || page.through < page.after
    {
        return Ok(SearchApplied::Stale);
    }
    let mut docs = Docs::prepare(conn)?;
    for doc in &page.docs {
        // NOTE: a stale page cannot reach here, so a doc never exists twice; a row that
        // does is taken out first anyway, so the index and the docs stay in step.
        docs.remove(doc.seq)?;
        docs.insert(doc)?;
    }
    for &seq in &page.drops {
        docs.remove(seq)?;
    }
    conn.execute(
        "UPDATE search_state SET indexed_seq = ?1 WHERE id = 1",
        [sql::seq(page.through)],
    )?;
    Ok(SearchApplied::Applied)
}

/// Empties the index and sets its cursor to the start, for a backfill with the
/// redaction `redaction`. The free pages of the old index are used again; the file
/// does not get smaller.
pub(crate) fn reset(conn: &Connection, redaction: u32) -> Result<(), StoreError> {
    conn.execute_batch(
        "INSERT INTO search_fts (search_fts) VALUES ('delete-all');
         DELETE FROM search_docs;
         DELETE FROM search_titles;",
    )?;
    conn.execute(
        "UPDATE search_state SET indexed_seq = 0, redaction = ?1 WHERE id = 1",
        [redaction],
    )?;
    Ok(())
}

/// Merges the segments of the index toward one, writing about `pages` pages, and
/// answers whether it did work. A negative FTS5 `'merge'` merges every level, also one
/// below the `usermerge` threshold, so the merges after a backfill end with one
/// segment and a search reads few. FTS5 counts the command itself as one change, and
/// work as more (its documentation of `'merge'`).
pub(crate) fn merge(conn: &Connection, pages: u32) -> Result<bool, StoreError> {
    let before = conn.total_changes();
    let pages = -i64::from(pages.max(1));
    conn.execute("INSERT INTO search_fts (search_fts, rank) VALUES ('merge', ?1)", [pages])?;
    Ok(conn.total_changes().saturating_sub(before) >= 2)
}

/// The statements of one write job over `search_docs`, `search_fts` and
/// `search_titles`, prepared once per job.
struct Docs<'c> {
    conn: &'c Connection,
    read: Statement<'c>,
    insert: Statement<'c>,
    insert_fts: Statement<'c>,
    delete: Statement<'c>,
    delete_fts: Statement<'c>,
    title: Statement<'c>,
}

impl<'c> Docs<'c> {
    fn prepare(conn: &'c Connection) -> Result<Self, StoreError> {
        Ok(Docs {
            conn,
            read: conn.prepare("SELECT conversation_id, kind, text FROM search_docs WHERE seq = ?1")?,
            insert: conn.prepare(
                "INSERT INTO search_docs (seq, conversation_id, kind, text) VALUES (?1, ?2, ?3, ?4)",
            )?,
            insert_fts: conn.prepare(
                "INSERT INTO search_fts (rowid, text, conversation_id, kind) VALUES (?1, ?2, ?3, ?4)",
            )?,
            delete: conn.prepare("DELETE FROM search_docs WHERE seq = ?1")?,
            delete_fts: conn.prepare(
                "INSERT INTO search_fts (search_fts, rowid, text, conversation_id, kind) \
                 VALUES ('delete', ?1, ?2, ?3, ?4)",
            )?,
            // The oldest prompt names the conversation, also when docs come out of
            // order.
            title: conn.prepare(
                "INSERT INTO search_titles (conversation_id, title, first_seq) \
                 VALUES (?1, ?2, ?3) \
                 ON CONFLICT (conversation_id) DO UPDATE SET \
                     title = excluded.title, first_seq = excluded.first_seq \
                 WHERE excluded.first_seq < search_titles.first_seq",
            )?,
        })
    }

    fn insert(&mut self, doc: &SourceRow) -> Result<(), StoreError> {
        let seq = sql::seq(doc.seq);
        let conversation = doc.conversation_id.to_string();
        let kind = doc.kind.code();
        self.insert.execute(params![seq, conversation, kind, doc.text])?;
        self.insert_fts.execute(params![seq, doc.text, conversation, kind])?;
        if doc.kind == DocKind::Prompt
            && let Some(title) = title_from(&doc.text)
        {
            self.title.execute(params![conversation, title, seq])?;
        }
        Ok(())
    }

    /// Takes the doc at `seq` out of the index, and passes the title of its
    /// conversation on when it was the prompt that named it. A seq with no doc is no
    /// error.
    fn remove(&mut self, seq: Seq) -> Result<(), StoreError> {
        let seq = sql::seq(seq);
        let old: Option<(String, i64, String)> = self
            .read
            .query_row([seq], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?;
        let Some((conversation, kind, text)) = old else {
            return Ok(());
        };
        self.delete_fts.execute(params![seq, text, conversation, kind])?;
        self.delete.execute([seq])?;
        if DocKind::from_code(kind) == Some(DocKind::Prompt) {
            rename(self.conn, &conversation, seq)?;
        }
        Ok(())
    }
}

/// After the prompt doc `seq` of `conversation` left the index: when it named the
/// conversation, the oldest prompt doc that remains names it, or no title when none
/// does. The prompts come from the conversation's events, then their docs by seq,
/// because `search_docs` has no index on the conversation.
fn rename(conn: &Connection, conversation: &str, seq: i64) -> Result<(), StoreError> {
    let named: Option<i64> = conn
        .query_row(
            "SELECT first_seq FROM search_titles WHERE conversation_id = ?1",
            [conversation],
            |row| row.get(0),
        )
        .optional()?;
    if named != Some(seq) {
        return Ok(());
    }
    let mut prompts = conn.prepare(
        "SELECT d.seq, d.text FROM events AS e JOIN search_docs AS d ON d.seq = e.seq \
         WHERE e.conversation_id = ?1 AND e.kind = 'prompt_queued' ORDER BY e.seq",
    )?;
    let mut rows = prompts.query([conversation])?;
    while let Some(row) = rows.next()? {
        let text: String = row.get(1)?;
        if let Some(title) = title_from(&text) {
            let first: i64 = row.get(0)?;
            conn.execute(
                "UPDATE search_titles SET title = ?2, first_seq = ?3 WHERE conversation_id = ?1",
                params![conversation, title, first],
            )?;
            return Ok(());
        }
    }
    conn.execute("DELETE FROM search_titles WHERE conversation_id = ?1", [conversation])?;
    Ok(())
}

#[cfg(test)]
mod tests;
