-- The search index of the conversations (v0.0.4). efrd fills it in the background from
-- the event log, with text that it bounded, cleaned and redacted first, so the store
-- never decides what goes in. Not a projection: the store cannot redact, so a rebuild
-- of the projections leaves these tables alone, and the migration creates them empty.
-- The indexer then reads the log from `search_state.indexed_seq` and fills them.

-- One row per indexed event: the text of a prompt (kind 0), a steer (1), an answer of
-- the model (2) or the command line of a `shell` call (3). The row id is the sequence
-- number of the event, so newest first is `ORDER BY seq DESC` for every row, also for
-- rows that the backfill writes later. The turn and the time of a row are in `events`.
CREATE TABLE search_docs (
    seq INTEGER PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    kind INTEGER NOT NULL,
    text TEXT NOT NULL
) STRICT;

-- The full text index over `search_docs`. External content: the writer adds and
-- removes each row of the index in the transaction that changes `search_docs`, with no
-- trigger. `_`, `-`, `/` and `.` split tokens, so the parts of a path or a snake_case
-- name match. The prefix indexes of 2 and 3 characters keep the one prefix of a query
-- cheap.
CREATE VIRTUAL TABLE search_fts USING fts5 (
    text,
    conversation_id UNINDEXED,
    kind UNINDEXED,
    content = 'search_docs',
    content_rowid = 'seq',
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3'
);

-- The search title of each conversation: the first line of its first prompt that is
-- not blank, from the indexed text, so it holds no secret that the redaction found.
-- `first_seq` is the sequence number of that prompt; when it leaves the index, the
-- next prompt names the conversation.
CREATE TABLE search_titles (
    conversation_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    first_seq INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- How far the index is: every event up to `indexed_seq` is in it, with the redaction of
-- version `redaction`. Version 0 is no redaction yet, so the indexer of a new build
-- starts from the beginning.
CREATE TABLE search_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    indexed_seq INTEGER NOT NULL,
    redaction INTEGER NOT NULL
) STRICT;

INSERT INTO search_state (id, indexed_seq, redaction) VALUES (1, 0, 0);

-- The newest conversations without a sort, for the recent list and the title search.
CREATE INDEX conversations_updated ON conversations (updated_at, last_seq);
