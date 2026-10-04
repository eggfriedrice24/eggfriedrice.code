# Storage

`efr-store` is the only crate that opens SQLite. This file describes what is on disk.
The schema lands with `efr-store` in milestone 1; the tables below are the plan.

## Directory layout

| Path | Content | Owner |
|---|---|---|
| `$XDG_DATA_HOME/efr/efr.sqlite` (+ `-wal`, `-shm`) | the one database | `efr-store` |
| `$XDG_DATA_HOME/efr/daemon.lock` | exclusive `flock`; decides single instance | `efr-daemon/src/lock.rs` |
| `$XDG_DATA_HOME/efr/recordings/<pty_id>/<start_seq>.rec` | append-only PTY recording segments | `efr-store/src/recording.rs` |
| `$XDG_DATA_HOME/efr/scratch/<YYYY-MM-DD>-<slug>-<idtail>/` | `$SCRATCH` per conversation, claimed with a non-recursive `mkdir` | `efr-conversation/src/scratch.rs` |
| `$XDG_DATA_HOME/efr/secrets/<provider>.json` (dir 0700, files 0600) | credential records | `efr-credentials/src/file_store.rs` |
| `$XDG_DATA_HOME/efr/backups/efr.sqlite.<user_version>` | copy taken before each migration | `efr-store/src/migrations.rs` |
| `$XDG_STATE_HOME/efr/logs/` | optional JSON log file | `efr-daemon/src/telemetry.rs` |
| `$XDG_RUNTIME_DIR/efr/daemon.sock` (0600) | the Unix socket | `efr-transport/src/unix_listener.rs` |
| `$XDG_RUNTIME_DIR/efr/daemon.json` | `{pid, socket, protocol, daemon_id, tailnet_endpoint?}` for discovery | `efr-daemon/src/discovery.rs` |
| `$XDG_RUNTIME_DIR/efr/notices/<tty>` | notices for one terminal, shown and removed by the zsh plugin at the next prompt (`<tty>` is `$TTY` without `/dev/`, with `/` as `-`) | the daemon writes, `shell/zsh/efr.plugin.zsh` reads |
| `$XDG_CONFIG_HOME/efr/config.toml`, `projects.toml` | config; the explicit project registry | `efr-daemon/src/config.rs`, `efr-scope/src/registry.rs` |

`EFR_DATA_DIR` and the matching variables for the other roots override each root, which
is how tests and `just run` use temporary directories. The zsh plugin follows
`EFR_RUNTIME_DIR` too, so a shell pointed at a `just run` daemon shows its notices.

## SQLite access

- `rusqlite` with the `bundled` feature, opened only through `efr_store::db::open`
  (clippy's `disallowed-methods` routes `rusqlite::Connection::open*` there).
- Pragmas: `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000`,
  `foreign_keys=ON`, `journal_size_limit=64MiB`, `temp_store=MEMORY`.
- One writer: `StoreWriter` is an actor that owns the only read-write connection.
  `append(Batch) -> Seq` writes events, projections, receipts and outbox rows in one
  transaction, then broadcasts the new events. Publishing in commit order needs no
  extra lock.
- Readers: a few read-only connections behind `Readers::with(|conn| ...)`, used from
  `spawn_blocking`. Whether one reader thread beats a small pool is still open.

## Schema

- `events(seq INTEGER PRIMARY KEY, conversation_id, turn_id, kind TEXT, payload JSON,
  created_at)`. `seq` is global and only grows. Events carry full state, never deltas,
  so a subscriber can resume from `after_seq` without the projections. The payload is
  the serde form of `efr_protocol::Event`; an unknown kind reads back as
  `Event::Unknown { kind, payload }` so old readers keep advancing.
- Projections `conversations`, `turns`, `approvals`, `shells` are rebuilt from events
  by `cargo xtask rebuild-projections`, and a test asserts the rebuild matches. They
  serve listing and paging; they are not a second source of truth.
- `receipts(command_id PRIMARY KEY, method, result JSON, seq, created_at)`. A duplicate
  `command_id` returns the stored result; a rejected one stays rejected; nothing is
  replayed automatically after a reconnect.
- `outbox(id, kind, payload, replay_safe INTEGER, claimed_at, done_at)`. Replay-safe
  rows survive a restart; process-bound rows are cancelled at startup.
- `recording_segments(pty_id, start_seq, path, bytes, started_at, closed_at)` indexes
  the recording files.

## Migrations

- SQL files in `crates/efr-store/src/migrations/`, named `NNNN_<noun>.sql`, embedded
  with `include_str!` and applied by `rusqlite_migration` (tracked in
  `PRAGMA user_version`, forward-only, one transaction per file).
- Before migrating, the daemon copies the database to `backups/`. The socket opens only
  after migrations finish.
- Milestone 1 ships `0001_events.sql`, `0002_conversations.sql`,
  `0003_receipts_outbox.sql` and `0004_shells_recordings.sql`. The devices and scopes
  table waits for the phone milestone, because a forward-only migration makes an unused
  table permanent.
- A migration never edits an earlier file. New columns use `ALTER TABLE ... ADD COLUMN`
  with a default so old projections keep reading.
- Tests call `Migrations::validate()` and migrate a copy of every fixture database in
  `crates/efr-store/fixtures/db/`.

## Recording format

Each PTY's output is appended to segment files where the byte offset is the stream
sequence. Every chunk has a 16-byte header with a timestamp, followed by the raw bytes.
A segment rotates at 8 MiB. `pty.attach(since_seq)` and the shell tool's output slices
read through `efr_store::recording::read_range(pty_id, start, end)`. Shell marks carry
recording offsets, so the output of one command is the range between its OSC 133 `C`
and `D` marks.
