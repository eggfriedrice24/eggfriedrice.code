-- The exact messages of a finished turn as the provider saw them, with the provider's
-- own items (encrypted reasoning, item ids) that no event holds, so a conversation
-- that goes on after a restart sends them back to the same provider and model. Not a
-- projection: the event log cannot rebuild it, so a projection rebuild leaves it
-- alone, and it has no foreign key to `turns`, which a rebuild empties. The writer
-- keeps the newest turns of each conversation that the history may carry.
CREATE TABLE turn_messages (
    conversation_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    message TEXT NOT NULL CHECK (json_valid(message)),
    -- The sequence number of the turn's terminal event, which orders the turns.
    turn_seq INTEGER NOT NULL,
    PRIMARY KEY (turn_id, position)
) STRICT;

CREATE INDEX turn_messages_conversation ON turn_messages (conversation_id, turn_seq);
