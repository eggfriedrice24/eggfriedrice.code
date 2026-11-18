-- The attachments of each conversation (v0.0.13, ADR 0026): a projection of
-- `prompt_queued`, `tool_call_completed`, `attachments_withheld` and
-- `prompt_withdrawn`. No event before v0.0.13 holds an attachment, so the table is
-- right when it is empty, and this step rebuilds no projection.
--
-- Each attachment has its own row, also when two have the same bytes; only the blob
-- file of efrd's data dir, which has the SHA-256 as its name, is shared. No bytes go
-- into this table.
CREATE TABLE attachments (
    conversation_id TEXT NOT NULL REFERENCES conversations (id),
    -- The number of its `efr_protocol::AttachmentId`: 3 for `a3`.
    number INTEGER NOT NULL CHECK (number >= 1),
    -- The SHA-256 of the original bytes, in lowercase hex: the name of the blob file.
    sha256 TEXT NOT NULL CHECK (length(sha256) = 64),
    -- The `efr_protocol::Attachment` as JSON.
    attachment TEXT NOT NULL CHECK (json_valid(attachment)),
    -- 1 after an `attachments_withheld` event named it: efrd sends it as a label from
    -- then on.
    withheld INTEGER NOT NULL DEFAULT 0 CHECK (withheld IN (0, 1)),
    -- The sequence number of the event that recorded it.
    seq INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, number)
) STRICT, WITHOUT ROWID;

-- The rows that share a blob file, for the delete of a withdrawn prompt.
CREATE INDEX attachments_blob ON attachments (conversation_id, sha256);
