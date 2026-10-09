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
| `$XDG_DATA_HOME/efr/snapshots/<root-id>.git`, `<root-id>.index`, `<root-id>.root` | efr's own snapshot store: one bare git repository per project or `$SCRATCH` (`root-id` is the first 16 hex characters of the SHA-256 of the root's canonical path), its persistent index and the root's path; refs `refs/efr/<conversation>/<turn>/pre` and `/post` per turn. The sandbox masks it with the rest of the data root. The collector keeps the newest `snapshot.keep_turns` turns of each conversation and deletes a store without a snapshot for `snapshot.max_age_days` | `efr-snapshot` |
| `$XDG_DATA_HOME/efr/secrets/<provider>.json` (dir 0700, files 0600) | credential records | `efr-credentials/src/file_store.rs` |
| `$XDG_DATA_HOME/efr/backups/efr.sqlite.<user_version>` | copy taken before each migration | `efr-store/src/migrations.rs` |
| `$XDG_STATE_HOME/efr/logs/` | optional JSON log file | `efr-daemon/src/telemetry.rs` |
| `$XDG_STATE_HOME/efr/model_catalog.json` (0600) | the last model catalog that the subscription backend sent, with its base URL, efr's version, the time and the `ETag`; efrd starts with it while the backend does not answer, and replaces it in one step after each fetch | `efr-daemon/src/catalog.rs`, `efr-provider-openai/src/catalog/cache.rs` |
| `$XDG_STATE_HOME/efr/sandbox/<conversation>/tmp/` | the private `/tmp` and `/var/tmp` of the `auto` sandbox, one per conversation | `efr-daemon` makes it, `efr-sbx` binds it |
| `$XDG_STATE_HOME/efr/sandbox/<conversation>/cache/<name>/upper/`, `work/` | the private upper layer of one tool cache overlay; efrd deletes it after `sandbox.cache_days` without a call, with the conversation, or when all layers pass `sandbox.cache_max_gib` | `efr-daemon`, `efr-sandbox/src/spec.rs` (`CacheOverlay`) |
| `$XDG_STATE_HOME/efr/sandbox/<conversation>/last-call` | an empty file that each call's plan touches; the cache collector counts idle days from its time | `efr-daemon/src/sandbox/gc.rs` |
| `$XDG_STATE_HOME/efr/sandbox/<conversation>/cache.gone-<n>/` | cache layers that the collector moved aside under its lock and deletes next; a call that starts meanwhile gets new, empty layers | `efr-daemon/src/sandbox/gc.rs` |
| `$XDG_STATE_HOME/efr/sandbox/<conversation>/quarantine/<call>/` | git settings that a call planted and the surface guard moved away; the quarantine question moves them back | `efr-sbx` |
| `$XDG_STATE_HOME/efr/sandbox/projects/<root>.json` | the git dir and common dir of a worktree or submodule project, recorded at `efr project add` | `efr-daemon`, `efr-sandbox/src/worktree.rs` |
| `$XDG_STATE_HOME/efr/sandbox/probe/` | the throwaway project of the sandbox probe | `efr-sbx probe` |
| `$XDG_RUNTIME_DIR/efr/daemon.sock` (0600) | the Unix socket | `efr-transport/src/unix_listener.rs` |
| `$XDG_RUNTIME_DIR/efr/daemon.json` | `{pid, socket, protocol, daemon_id, tailnet_endpoint?}` for discovery | `efr-daemon/src/discovery.rs` |
| `$XDG_RUNTIME_DIR/efr/bin/efr-sbx` (0500) | the copy of the sandbox launcher that hidden shells run; efrd copies it at start from the installed `efr-sbx` and checks its SHA-256 | `efr-daemon` |
| `$XDG_RUNTIME_DIR/efr/zsh/efr-child.zsh`, `efr-editor` | the script of the sandboxed child shell, and the editor stub that `EDITOR` names in a call | `efr-shell` assets |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/path` | the hidden shell's `PATH`, where efrd resolves the programs that an exit question names | the hidden shell's precmd hook |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/snapshot.zsh` | the hidden shell's functions, aliases and options, which each sandboxed call replays | the hidden shell's wrapper |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/state.zsh`, `state.json` | what sandboxed calls of the conversation defined (functions, aliases, exports that stay in the sandbox) | `efr-sbx` |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/config-listings.json` | what git listed of each git config content that the last call's surface guard read, so the next call runs git only for a new content | `efr-sbx`, `efr-sandbox/src/surface/listings.rs` |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/<call>/` (0700, files 0600) | one call: `spec.json` and `nonce` from efrd, `line` (the model's line), `started`, `apply` (`cd` and promoted exports for the hidden shell) and `result.json` from the launcher, `times` (the time of the wrapper's steps, for the debug log) from the hidden shell | `efr-daemon`, `efr-shell`, `efr-sbx` |
| `$XDG_RUNTIME_DIR/efr/notices/<tty>` | notices for one terminal, shown and removed by the zsh plugin at the next prompt (`<tty>` is `$TTY` without `/dev/`, with `/` as `-`) | the daemon writes, `shell/zsh/efr.plugin.zsh` reads |
| `$XDG_CONFIG_HOME/efr/config.toml`, `projects.toml` | config; the explicit project registry | `efr-config`, `efr-scope/src/registry.rs` |

`EFR_DATA_DIR` and the matching variables for the other roots override each root, which
is how tests and `just run` use temporary directories. Without its own variable, a root
is below `EFR_HOME` when that is set (`$EFR_HOME/config`, `data`, `state`, `runtime`),
else the XDG directory above; the runtime root falls back to `/run/user/<uid>/efr` when
`XDG_RUNTIME_DIR` is unset and `/run/user/<uid>` is the user's own with mode 0700.
`efr paths` shows each root and where it came from. The zsh plugin follows
`EFR_RUNTIME_DIR` too, so a shell pointed at a `just run` daemon shows its notices.
`config.toml` may be a symbolic link (into a dotfiles repository); the daemon watches
the directory of its target too, and `efr config set` writes the target and keeps the
link.

Inside a call of the `auto` sandbox, the data, state and runtime roots are empty: only
the conversation's `$SCRATCH` comes back, and the database reads as missing. So a
sandboxed command cannot read the call files, the nonce, the cache layers of other
conversations or the daemon's socket. The config root stays readable and read-only.
`efr paths` names the launcher and the sandbox's state and runtime directories;
[`docs/sandbox.md`](sandbox.md) tells what each part does.

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
- Projections `conversations`, `turns`, `approvals`, `shells`, `compactions` are
  rebuilt from events by `cargo xtask rebuild-projections`, and a test asserts the
  rebuild matches. They serve listing and paging; they are not a second source of
  truth.
- `receipts(command_id PRIMARY KEY, method, result JSON, seq, created_at)`. A duplicate
  `command_id` returns the stored result; a rejected one stays rejected; nothing is
  replayed automatically after a reconnect.
- `outbox(id, kind, payload, replay_safe INTEGER, claimed_at, done_at)`. Replay-safe
  rows survive a restart; process-bound rows are cancelled at startup.
- `recording_segments(pty_id, start_seq, path, bytes, started_at, closed_at)` indexes
  the recording files.
- `turn_messages(conversation_id, turn_id, position, provider, model, message JSON,
  turn_seq)`, keyed by `(turn_id, position)`: the exact messages of a finished turn
  with the provider's own items (`provider_raw`), which no event holds. Saved in the
  batch of the turn's terminal event (`turn_seq` is its sequence number); each
  conversation keeps every turn that no summary covers yet. Not a projection: the log
  cannot rebuild it, so a rebuild leaves it alone and it has no foreign key to
  `turns`. A `conversation_compacted` with a summary deletes the rows of the turns
  before its cut in the same batch.
- `compactions(seq, conversation_id, compaction_id, has_summary, compaction JSON)`:
  one row per `conversation_compacted` event, a projection. A turn reads the newest
  row with a summary and the newest row of any kind with a query of their own, never
  from the page of newest events.

## Migrations

- SQL files in `crates/efr-store/src/migrations/`, named `NNNN_<noun>.sql`, embedded
  with `include_str!` and applied by `rusqlite_migration` (tracked in
  `PRAGMA user_version`, forward-only, one transaction per file).
- Before migrating, the daemon copies the database to `backups/`. The socket opens only
  after migrations finish.
- Milestone 1 ships `0001_events.sql`, `0002_conversations.sql`,
  `0003_receipts_outbox.sql`, `0004_shells_recordings.sql`,
  `0005_turn_messages.sql` and `0006_compactions.sql`. The devices and scopes
  table waits for the phone milestone, because a forward-only migration makes an unused
  table permanent.
- A migration never edits an earlier file. New columns use `ALTER TABLE ... ADD COLUMN`
  with a default so old projections keep reading.
- Tests call `Migrations::validate()` and migrate a copy of every fixture database in
  `crates/efr-store/fixtures/db/`.

## Recording format

Each PTY's output is appended to segment files where the byte offset is the stream
sequence. Every chunk has a 16-byte header with a kind and a timestamp, followed by its
bytes: kind 1 holds raw output bytes, kind 2 holds a new size of the PTY (columns and
rows) at the offset of the next output byte, and takes no stream bytes. A recording
from before kind 2 reads as before. A segment rotates at 8 MiB; a segment that holds
sizes alone never rotates. `pty.attach(since_seq)` and the shell tool's output slices
read through `efr_store::recording::read_range(pty_id, start, end)`. Shell marks carry
recording offsets, so the output of one command is the range between its OSC 133 `C`
and `D` marks.
