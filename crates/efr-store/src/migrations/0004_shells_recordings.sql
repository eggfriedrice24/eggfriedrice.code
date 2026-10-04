-- The hidden shell of each conversation, projected from shell_started, shell_exited
-- and cwd_changed events.
CREATE TABLE shells (
    pty_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    status TEXT NOT NULL,
    cwd TEXT NOT NULL,
    host TEXT,
    pid INTEGER,
    exit_code INTEGER,
    started_seq INTEGER NOT NULL,
    started_at INTEGER NOT NULL,
    exited_seq INTEGER,
    exited_at INTEGER
) STRICT;

CREATE INDEX shells_conversation ON shells (conversation_id, started_seq);

-- The index of PTY recording segments. `start_seq` is the stream offset of the
-- segment's first byte, `path` is relative to the recordings directory, and `bytes`
-- counts stream bytes and is final once `closed_at` is set. Not a projection: the
-- recording writer maintains it.
CREATE TABLE recording_segments (
    pty_id TEXT NOT NULL,
    start_seq INTEGER NOT NULL,
    path TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    started_at INTEGER NOT NULL,
    closed_at INTEGER,
    PRIMARY KEY (pty_id, start_seq)
) STRICT, WITHOUT ROWID;

-- Approval requests, projected from approval_requested, approval_resolved and
-- approval_expired events. `decision` and `resolved_by` hold wire names.
CREATE TABLE approvals (
    call_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    turn_id TEXT NOT NULL,
    summary TEXT NOT NULL,
    diff_preview TEXT,
    status TEXT NOT NULL,
    decision TEXT,
    resolved_by TEXT,
    requested_seq INTEGER NOT NULL,
    requested_at INTEGER NOT NULL,
    resolved_seq INTEGER,
    resolved_at INTEGER
) STRICT;

CREATE INDEX approvals_pending ON approvals (conversation_id, requested_seq)
    WHERE status = 'pending';
