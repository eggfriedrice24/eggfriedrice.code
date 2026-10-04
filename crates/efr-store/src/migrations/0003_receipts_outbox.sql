-- One receipt per client command id. `outcome` is `accepted` (with the method's
-- result in `result`) or `rejected` (with the wire error body in `result`). `seq` is
-- the last event committed with the receipt.
CREATE TABLE receipts (
    command_id TEXT PRIMARY KEY,
    method TEXT NOT NULL,
    outcome TEXT NOT NULL,
    result TEXT NOT NULL CHECK (json_valid(result)),
    seq INTEGER,
    created_at INTEGER NOT NULL
) STRICT;

-- Durable side effects, claimed in id order. AUTOINCREMENT keeps an id from ever
-- naming two rows, so a late `done` cannot finish the wrong item.
CREATE TABLE outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL CHECK (json_valid(payload)),
    replay_safe INTEGER NOT NULL CHECK (replay_safe IN (0, 1)),
    created_at INTEGER NOT NULL,
    claimed_at INTEGER,
    done_at INTEGER,
    cancelled_at INTEGER
) STRICT;

CREATE INDEX outbox_open ON outbox (id) WHERE done_at IS NULL AND cancelled_at IS NULL;
