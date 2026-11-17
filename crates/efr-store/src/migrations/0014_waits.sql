-- Who can act on a conversation (v0.0.12). Like every projection, the new columns and
-- the new table are filled from the log when the migration rebuilds the projections,
-- so this file has no backfill.
--
-- `approvals.local_only`: 1 when only the terminal may answer the approval.
-- `conversations.attention`: the wire name of `efr_protocol::Attention`, or NULL for
-- none.
ALTER TABLE approvals ADD COLUMN local_only INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversations ADD COLUMN attention TEXT;

-- What a running turn waits for apart from an approval: `input` (a call that waits for
-- a line, by its call id), `question` (a question of the model, by its question id) or
-- `surface` (a quarantine question, by its question id). The end of the turn removes
-- its rows.
CREATE TABLE waits (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    turn_id TEXT NOT NULL,
    kind TEXT NOT NULL
) STRICT;

CREATE INDEX waits_conversation ON waits (conversation_id);
