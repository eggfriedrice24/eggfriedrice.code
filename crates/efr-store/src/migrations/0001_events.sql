-- The event log. One row per event; `seq` is global, starts at 1 and only grows, and
-- the writer assigns it. `payload` is the serde JSON of efr_protocol::Event with its
-- `kind` member; `kind`, `conversation_id` and `turn_id` repeat parts of it so they
-- can be indexed. Times are microseconds since the Unix epoch.
CREATE TABLE events (
    seq INTEGER PRIMARY KEY,
    conversation_id TEXT,
    turn_id TEXT,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL CHECK (json_valid(payload)),
    created_at INTEGER NOT NULL
) STRICT;

CREATE INDEX events_conversation ON events (conversation_id, seq);

-- Events are facts: projections and subscribers rely on a row never changing.
CREATE TRIGGER events_no_update BEFORE UPDATE ON events
BEGIN
    SELECT RAISE(ABORT, 'events are append-only');
END;

CREATE TRIGGER events_no_delete BEFORE DELETE ON events
BEGIN
    SELECT RAISE(ABORT, 'events are append-only');
END;
