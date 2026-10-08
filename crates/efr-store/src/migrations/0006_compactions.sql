-- The compactions of each conversation's context, a projection of the
-- `conversation_compacted` events. A turn reads the newest compaction with a summary
-- and the newest of any kind with a query of their own, so the page of newest events
-- never decides what the model sees. Rebuilt from the log like the other projections.
CREATE TABLE compactions (
    -- The sequence number of the `conversation_compacted` event.
    seq INTEGER PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    compaction_id TEXT NOT NULL,
    -- 1 when the compaction wrote a summary, 0 when only pruning ran.
    has_summary INTEGER NOT NULL,
    -- The whole `efr_protocol::Compaction` as JSON.
    compaction TEXT NOT NULL CHECK (json_valid(compaction))
) STRICT;

CREATE INDEX compactions_conversation ON compactions (conversation_id, has_summary, seq);
