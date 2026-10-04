-- Projections of the event log for listing and paging. They are rebuilt from the
-- events at any time and are never a second source of truth. Enum columns hold the
-- snake_case wire names and have no CHECK, so a new value needs no table rebuild.
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    origin TEXT NOT NULL,
    title TEXT,
    status TEXT NOT NULL,
    tty TEXT,
    cwd TEXT,
    scope TEXT CHECK (scope IS NULL OR json_valid(scope)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_seq INTEGER NOT NULL
) STRICT;

-- `last_seq` is unique per conversation (every event belongs to at most one), so it
-- is the keyset for paging newest first.
CREATE INDEX conversations_last_seq ON conversations (last_seq);
CREATE INDEX conversations_tty ON conversations (tty) WHERE tty IS NOT NULL;

CREATE TABLE turns (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    command_id TEXT,
    prompt TEXT NOT NULL,
    status TEXT NOT NULL,
    queued_seq INTEGER NOT NULL,
    started_at INTEGER,
    ended_at INTEGER,
    last_seq INTEGER NOT NULL
) STRICT;

CREATE INDEX turns_conversation ON turns (conversation_id, queued_seq);
CREATE INDEX turns_status ON turns (status);
