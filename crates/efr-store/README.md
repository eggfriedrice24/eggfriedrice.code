# efr-store

## Purpose

The only crate in efr that opens SQLite. It keeps everything the daemon must remember
across a restart:

- `db`: `open(path)` and `open_in_memory()`, the only calls to
  `rusqlite::Connection::open*` in the workspace (`clippy.toml` routes every other call
  here), with the pragmas `journal_mode=WAL`, `synchronous=NORMAL`,
  `busy_timeout=5000`, `foreign_keys=ON`, `journal_size_limit=64MiB` and
  `temp_store=MEMORY`. A new database file is created with mode 0600.
- `Migrations`: the forward-only SQL files in `src/migrations/`, embedded with
  `include_str!` and applied by `rusqlite_migration`, one transaction per file,
  tracked in `PRAGMA user_version`. Before migrating an existing database, a copy goes
  to `backups/efr.sqlite.<user_version>`.
- `StoreWriter` and `WriterHandle`: the single writer. A thread owns the only
  read-write connection; `WriterHandle::append(Batch)` commits events, the projections
  derived from them, command receipts and outbox rows in one transaction, and then
  broadcasts the committed events in commit order.
- `Readers`: read-only connections used through `spawn_blocking`, behind
  `Readers::with(|conn| ...)`. An in-memory store runs reads on the writer's
  connection with `query_only` set, because a private in-memory database has one
  connection.
- `events`: the append-only event log. `read_after(seq, limit)` and the per
  conversation reads return `EventEnvelope`s; a kind this build does not know reads
  back as `Event::Unknown`.
- `conversations`, `approvals`, `shells`: projections of the log for listing and
  paging, rebuilt from the events by `WriterHandle::rebuild_projections`.
- `receipts`: one receipt per command id. A duplicate command id returns the stored
  outcome, and a rejected command stays rejected. A receipt records the sequence
  number of its batch's last event, or of the event that `NewReceipt::seq_of_event`
  names, such as `prompt_queued` when `turn_started` follows it, so a retry gets the
  number the first answer reported.
- `outbox`: durable side effects, enqueued with the batch that decides them and
  claimed in id order through `WriterHandle::outbox_claim` and `outbox_done`.
  Replay-safe rows survive a restart; process-bound rows are cancelled at startup by
  `WriterHandle::outbox_cancel_process_bound`.
- `recording`: PTY recordings. Append-only segment files under
  `recordings/<pty_id>/<start_seq>.rec`, each chunk behind a 16-byte header with a
  timestamp, rotated at 8 MiB, indexed in `recording_segments`. Offsets are stream
  sequence numbers: byte `n` of a PTY's output has `Seq` `n`, which is what
  `pty.attach(since_seq)` and the shell marks use. `Recordings::read_range` slices
  them.
- `Store` and `StoreConfig`: open, back up, migrate and start the writer and readers
  in one call.

Consumers: `efr-conversation` appends turn events, `efr-test-support` builds an
in-memory store, and `efr-daemon` opens the store, reconciles after a restart, serves
subscriptions and history from it, and records PTY output through it.

## Tier

Tier 1.

## Allowed dependencies

`efr-protocol` (the events and ids it stores) and `efr-stdx` (the clock, the named
writer thread, private file creation). `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `rusqlite` (with `bundled`), `rusqlite_migration`, `tokio` (for
the channels and `spawn_blocking`), `serde`, `serde_json`, `jiff`, `thiserror`.

## Invariant

- No other crate reaches `rusqlite`; `cargo xtask deps` enforces that, and clippy's
  `disallowed-methods` sends every `Connection::open` to `efr_store::db`.
- One writer: only the writer thread holds a read-write connection, so commits are
  serial and the broadcast is in commit order without a further lock. A subscriber
  that subscribes, then reads the high-water mark, then forwards live events above it
  misses nothing and sees nothing twice.
- The event log is append-only (triggers refuse `UPDATE` and `DELETE`), `seq` starts at
  1 and only grows, and events carry full state, so projections are a function of the
  log and can be rebuilt at any time.
- A batch is all or nothing: events, projections, receipts and outbox rows commit in
  one transaction or not at all, and a failed batch does not use up sequence numbers.
- An approval is answered at most once: the writer applies `approval_resolved` only to
  a pending approval of the event's conversation and otherwise fails the batch with
  `StoreError::ApprovalNotPending`, so two racing answers cannot both commit and an
  answer never revives an expired approval. The check runs inside the transaction, not
  before it on a reader.
- Time comes from the injected `efr_stdx::time::Clock`; nothing here reads the wall
  clock.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-store
```

Most tests use an in-memory store; the reader pool, the backups, the fixture
databases under `fixtures/db/` and the recordings use temporary directories. The
tests use no network, no real-time sleeps and no Zig.
