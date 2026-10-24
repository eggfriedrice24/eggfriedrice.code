-- The deletes of conversations (v0.0.5). A delete first records the event
-- `conversation_deleted`, whose batch writes the tombstone row here and removes the
-- conversation's projection rows, so no list or search shows it from that commit on.
-- Then efrd removes the rest in short steps; a start of efrd finishes each delete whose
-- `done_at` is NULL. Not a projection: a rebuild leaves it alone and skips the events of
-- each conversation that it names.
CREATE TABLE conversation_deletes (
    conversation_id TEXT PRIMARY KEY,
    -- The sequence number of the `conversation_deleted` event, which stays in the log.
    seq INTEGER NOT NULL,
    requested_at INTEGER NOT NULL,
    -- What the conversation had outside the database, read from its events in the
    -- batch of the tombstone, because the events go before the files can:
    -- {"ptys": [...], "undos": [...]}.
    parts TEXT NOT NULL CHECK (json_valid(parts)),
    -- When the rows of the conversation were gone, and when its files were gone too.
    rows_done_at INTEGER,
    done_at INTEGER
) STRICT, WITHOUT ROWID;

CREATE INDEX conversation_deletes_open ON conversation_deletes (seq) WHERE done_at IS NULL;

-- Events stay facts: only an event of a deleted conversation from before its tombstone
-- event may go. The tombstone event itself, and every daemon-wide event, stays.
DROP TRIGGER events_no_delete;

CREATE TRIGGER events_no_delete BEFORE DELETE ON events
WHEN OLD.conversation_id IS NULL
    OR NOT EXISTS (
        SELECT 1 FROM conversation_deletes AS d
        WHERE d.conversation_id = OLD.conversation_id AND d.seq > OLD.seq
    )
BEGIN
    SELECT RAISE(ABORT, 'events are append-only');
END;
