-- Each step of a conversation delete removes the receipts of its events by their
-- sequence number. Without this index, each step read the whole table.
CREATE INDEX receipts_seq ON receipts (seq);
