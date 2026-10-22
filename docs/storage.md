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
| `$SCRATCH/jobs/` (0700, marker `.efr-jobs`), `$SCRATCH/jobs/<id>.out` | the output of each background job of the conversation, which efrd writes from the job's pipe: the first 1 MiB and the last 8 MiB. efrd makes the dir before every contained call; an entry of that name without the marker goes aside to `jobs.moved-<n>`. Every contained call reads it but cannot write it, and snapshots leave it out. efr removes an output 7 days after its job ended, and keeps at most 20 outputs of ended jobs per conversation | `efr-daemon/src/jobs/output.rs` |
| `$XDG_DATA_HOME/efr/snapshots/<root-id>.git`, `<root-id>.index`, `<root-id>.root` | efr's own snapshot store: one bare git repository per project or `$SCRATCH` (`root-id` is the first 16 hex characters of the SHA-256 of the root's canonical path), its persistent index and the root's path. Refs per turn: `refs/efr/<conversation>/<turn>/pre` (written at the turn's first snapshot of the root), `/last` (moved after each call, with the paths that the turn's calls changed), `/post` (at the turn's end) and `/undone` (after an undo). `refs/efr-mirror/<conversation>/<turn>/` holds the project's branches, tags, `refs/stash` and `HEAD` at the turn's start, at most `snapshot.mirror_max_mib` MiB of objects per turn and 4 times that per store. `refs/efr-undo/<conversation>/<undo-id>/` holds the state before an undo (`before`, `started`, `mirror/`), so a redo can take it back. The store holds the project's `.env` files and other small ignored files too, for up to `snapshot.max_age_days` days. The sandbox masks it with the rest of the data root. The collector keeps or drops a turn whole across every store: the newest `snapshot.keep_turns` turns of each conversation. It deletes a store without a snapshot for `snapshot.max_age_days` | `efr-snapshot` |
| `$XDG_DATA_HOME/efr/snapshots/home.git` | efr's home store: before an approved write outside the project and `$SCRATCH` in `auto`, efrd copies each file that the call can change into it, and after the call it records the state after the call. Ref `refs/efr/<conversation>/<turn>/home/<n>` per copy: a commit "before" and a child commit "after", each with `files/<absolute path>` and a manifest `efr-home.json` (the state of each path, its mode, a link's target, the dirs above a missing path that were missing too). efrd streams the bytes to `git fast-import`, so git never reads a home path. At most 16 MiB per file; a directory with more than `snapshot.home_max_files` files or `snapshot.home_max_mib` MiB gets no copy. The collector keeps it with the turns of the root stores | `efr-snapshot/src/home.rs`, `efr-daemon/src/home.rs` |
| `$XDG_DATA_HOME/efr/undo-trash/<undo-id>/` (dirs 0700) | every file that `efr undo` overwrote or removed, written from the store before the write: `root/<root-id>/<path in the root>` and `home/<absolute path without the leading />`. A file keeps its mode without set-id bits. efr's own floor protects it, and the sandbox masks it. The collector deletes an undo dir older than `snapshot.max_age_days` | `efr-daemon/src/tools/undo.rs` |
| `$XDG_DATA_HOME/efr/secrets/<provider>.json` (dir 0700, files 0600) | credential records | `efr-credentials/src/file_store.rs` |
| `$XDG_DATA_HOME/efr/backups/efr.sqlite.<user_version>` | copy taken before each migration | `efr-store/src/migrations.rs` |
| `$XDG_DATA_HOME/efr/file-backups/<conversation>/<turn>/<n>/<file name>`, `meta.json` (dirs 0700, files 0600) | legacy: the copies of floor files from before the home store. efrd writes no new copy here. The collector deletes a turn older than `snapshot.max_age_days`, and the dir when it is empty. An old event keeps its turn-end line `changed <path>; backup at <path>` | `efr-daemon/src/backups.rs` |
| `$XDG_STATE_HOME/efr/logs/` | optional JSON log file | `efr-daemon/src/telemetry.rs` |
| `$XDG_STATE_HOME/efr/model_catalog.json` (0600) | the last model catalog that the subscription backend sent, with its base URL, efr's version, the time and the `ETag`; efrd starts with it while the backend does not answer, and replaces it in one step after each fetch | `efr-daemon/src/catalog.rs`, `efr-providers/src/openai/catalog/cache.rs` |
| `$XDG_STATE_HOME/efr/sandbox/<conversation>/tmp/` | the private `/tmp` and `/var/tmp` of the `auto` sandbox, one per conversation | `efr-daemon` makes it, `efr-sbx` binds it |
| `$XDG_STATE_HOME/efr/sandbox/<conversation>/cache/<name>/upper/`, `work/` | the private upper layer of one tool cache overlay; efrd deletes it after `sandbox.cache_days` without a call, with the conversation, or when all layers pass `sandbox.cache_max_gib` | `efr-daemon`, `efr-sandbox/src/spec.rs` (`CacheOverlay`) |
| `$XDG_STATE_HOME/efr/sandbox/<conversation>/last-call` | an empty file that each call's plan touches; the cache collector counts idle days from its time | `efr-daemon/src/sandbox/gc.rs` |
| `$XDG_STATE_HOME/efr/sandbox/<conversation>/cache.gone-<n>/` | cache layers that the collector moved aside under its lock and deletes next; a call that starts meanwhile gets new, empty layers | `efr-daemon/src/sandbox/gc.rs` |
| `$XDG_STATE_HOME/efr/sandbox/<conversation>/quarantine/<call>/` | git settings that a call planted and the surface guard moved away; the quarantine question moves them back | `efr-sbx` |
| `$XDG_STATE_HOME/efr/sandbox/<conversation>/jobs/<call>/` | one contained background job: its own cache layers (`cache/<name>/`), so it never holds the layer lock of a foreground call; `surface-before.json`, the git surface at the job's start; `guard.json`, the result of the last `efr-sbx guard-check`. efrd removes the dir at the job's end, and all of `jobs/` at its start after the checks of the jobs that the last efrd left | `efr-daemon`, `efr-sbx` |
| `$XDG_STATE_HOME/efr/sandbox/projects/<root>.json` | the git dir and common dir of a worktree or submodule project, recorded at `efr project add` | `efr-daemon`, `efr-sandbox/src/worktree.rs` |
| `$XDG_STATE_HOME/efr/sandbox/probe/` | the throwaway project of the sandbox probe | `efr-sbx probe` |
| `$XDG_STATE_HOME/efr/repo-hints.json` (0600) | when efrd last showed the hint that a git repository is not a registered project, as a JSON object of the repository's root and the time; efrd reads it at start and writes it by a rename after each hint, without the entries older than a day, so a restart shows no hint again on the same day | `efr-daemon/src/repo_hint.rs` |
| `$XDG_RUNTIME_DIR/efr/daemon.sock` (0600) | the Unix socket | `efr-transport/src/unix_listener.rs` |
| `$XDG_RUNTIME_DIR/efr/daemon.json` | `{pid, socket, protocol, daemon_id, tailnet_endpoint?}` for discovery | `efr-daemon/src/discovery.rs` |
| `$XDG_RUNTIME_DIR/efr/bin/efr-sbx` (0500) | the copy of the sandbox launcher that hidden shells run; efrd copies it at start from the installed `efr-sbx` and checks its SHA-256 | `efr-daemon` |
| `$XDG_RUNTIME_DIR/efr/zsh/efr-child.zsh`, `efr-editor` | the script of the sandboxed child shell, and the editor stub that `EDITOR` names in a call | `efr-shell` assets |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/path` | the hidden shell's `PATH`, where efrd resolves the programs that an exit question names | the hidden shell's precmd hook |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/snapshot.zsh` | the hidden shell's functions, aliases and options, which each sandboxed call replays | the hidden shell's wrapper |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/state.zsh`, `state.json` | what sandboxed calls of the conversation defined (functions, aliases, exports that stay in the sandbox) | `efr-sbx` |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/config-listings.json` | what git listed of each git config content that the last call's surface guard read, so the next call runs git only for a new content | `efr-sbx`, `efr-sandbox/src/surface/listings.rs` |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/<call>/` (0700, files 0600) | one call: `spec.json` and `nonce` from efrd, `line` (the model's line), `mounts` and `mounts.json` (the host's mount table and its sizes, only for a line that runs `df` or `findmnt`), `started`, `apply` (`cd` and promoted exports for the hidden shell) and `result.json` from the launcher, `times` (the time of the wrapper's steps, for the debug log) from the hidden shell. A contained call of a conversation whose running jobs forward ports also has `reach.sock` (0600), the socket of its reach, which efrd binds before the launch; the call's reach helper (`efr-reach`) reaches it read-only at `$XDG_RUNTIME_DIR/efr-reach.sock` inside the sandbox, and the call's own commands cannot connect to it. A call with no such job has none. A call that you moved into a background job with Ctrl+B also has `moved`, an empty file (0600) that efrd makes before the move; the launcher that finds it writes no `apply` and no sandbox state at its end, as a job does. Such a dir belongs to the job: efrd removes it at the job's end, and the shell dir above it stays | `efr-daemon`, `efr-shell`, `efr-sbx` |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/<call>/bus/` (0700) and `jobs/<call>/bus/` | the sockets of the call's or the job's bus filters, `system.sock` and `session.sock`, which the sandbox sees at `/run/dbus/system_bus_socket` and `$XDG_RUNTIME_DIR/bus`; holds nothing else. efrd removes a stale socket before each start | `efr-daemon/src/sandbox/bus.rs`, `xdg-dbus-proxy` under `efr-sbx dbus-proxy` |
| `$XDG_RUNTIME_DIR/efr/sbx/bus-test/<id>/` | the throwaway dir of the bus filter's self-test, which efrd removes after the test | `efr-daemon/src/sandbox/bus.rs` |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/env` (0600) | the hidden shell's exported variables, NUL-separated, which its integration writes at each prompt where they changed; efrd copies it into a job's call dir, so a job gets the shell's environment. It holds the values that you export, secrets too, in the runtime root, which the sandbox masks | the hidden shell's integration (`efr-shell`) |
| `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/jobs/<call>/` (0700, files 0600) | one background job: `spec.json` (with the job plan), `nonce`, `line`, `env` (at most 1 MiB) and, for a line that runs `df` or `findmnt`, `mounts` and `mounts.json` from efrd, `launcher.log` (the launcher's stderr), `started` and `result.json` from the launcher. A job with port forwards also has `forward.sock` (0600), the socket of its forwards, which efrd binds before the launcher starts; the job's forwarder (`efr-forward`) reaches it read-only at `$XDG_RUNTIME_DIR/efr-forward.sock` inside the sandbox, and the job's own commands cannot connect to it. Never a direct entry of the shell dir, so no check for a running call counts a job. efrd removes it at the job's end | `efr-daemon`, `efr-sbx` |
| `$XDG_RUNTIME_DIR/efr/projects.tsv` (0600) | the registered projects for the zsh plugin's scope marker: one line per project, `<root>` and a tab and `<name>`, the deepest root first. efrd writes it by a rename at start, after `efr project add` and `remove`, and at a prompt when `projects.toml` changed by hand. A root with a tab or a line break is left out, so the marker shows no project there | `efr-daemon/src/projects_file.rs` writes, `shell/zsh/efr.plugin.zsh` reads |
| `$XDG_RUNTIME_DIR/efr/notices/<tty>` | notices for one terminal, shown and removed by the zsh plugin at the next prompt (`<tty>` is `$TTY` without `/dev/`, with `/` as `-`) | the daemon writes, `shell/zsh/efr.plugin.zsh` reads |
| `$XDG_CONFIG_HOME/efr/config.toml`, `projects.toml` | config; the explicit project registry | `efr-config`, `efr-scope/src/registry.rs` |

`EFR_DATA_DIR` and the matching variables for the other roots override each root, which
is how tests and `just run` use temporary directories. Without its own variable, a root
is below `EFR_HOME` when that is set (`$EFR_HOME/config`, `data`, `state`, `runtime`),
else the XDG directory above; the runtime root falls back to `/run/user/<uid>/efr` when
`XDG_RUNTIME_DIR` is unset and `/run/user/<uid>` is the user's own with mode 0700.
`efr paths` shows each root and where it came from. The zsh plugin follows
`EFR_RUNTIME_DIR` too, so a shell pointed at a `just run` daemon shows its notices. It drops
a notice file whose time is before the shell loaded the plugin, unseen: such a file is for
an earlier terminal with the same name, such as one before a reboot. The time of the file is
the time of its last line, so one new line shows the older lines of the file too.
`config.toml` may be a symbolic link (into a dotfiles repository); the daemon watches
the directory of its target too, and `efr config set` writes the target and keeps the
link.

Inside a call of the `auto` sandbox, the data, state and runtime roots are empty: only
the conversation's `$SCRATCH` comes back, with `$SCRATCH/jobs` read-only, and the
database reads as missing. So a
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
- Search: the reads of conversation search never take one of these readers, so a burst
  of keys in the picker cannot slow a prompt. `SearchLanes` has 3 read-only
  connections of its own (the terminal's list, its preview and a phone at one time),
  with a semaphore of their own; the permit goes back only when the blocking work ends.
  efrd's search indexer has one more read-only connection (`IndexReader`), outside that
  semaphore. Each of these 4 connections sets a page cache of 512 KiB. They open on
  first use. An in-memory store runs them on the writer's connection.

## Schema

- `events(seq INTEGER PRIMARY KEY, conversation_id, turn_id, kind TEXT, payload JSON,
  created_at)`. `seq` is global and only grows. Events carry full state, never deltas,
  so a subscriber can resume from `after_seq` without the projections. The payload is
  the serde form of `efr_protocol::Event`; an unknown kind reads back as
  `Event::Unknown { kind, payload }` so old readers keep advancing.
- Projections `conversations`, `turns`, `approvals`, `shells`, `compactions`, `jobs`
  and `job_notices` are rebuilt from events by `cargo xtask rebuild-projections`, and a test asserts the
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
- `jobs(conversation_id, number, turn_id, call_id, command, launch, state, started_at,
  ends_by, ended_at, exit_code, stopped_by, output_bytes, launcher_pid, launcher_start,
  start_seq, info JSON)`, keyed by `(conversation_id, number)`: one row per background
  job, from `job_started`, `job_ended` and `job_watch_matched`. A restart of efrd reads
  the launcher's pid and start time here, never from a file that a job can write, and
  the job ids go on from the highest number. `job_notices` has one row per event that
  a notice tells about, so a start of efrd finds the notices that no turn took.
  `turns.wake` is 1 for a turn that a job notice started; the limits of wake turns per
  hour count these rows, also over a restart.
- `conversations.route`, `shell_pid` and `routed_seq` (migration 8): the slots of the
  terminals. A terminal keeps one conversation per scope key, a registered project or
  everything outside projects. A row with a `tty` is the conversation of that
  terminal's slot for its `route` (the JSON of `efr_protocol::RouteScope`);
  `shell_pid` is the shell that holds the slot, and `routed_seq` the slot's newest
  routed event, so the terminal's current conversation is its row with the highest.
  `conversation_created` with a tty and a route takes the slot from the row of the same
  tty and route; one from before routing by scope has no route and takes every row of
  its tty, as the daemon of then did. `terminal_attached` moves a conversation into a
  slot. A `prompt_queued` whose context names the row's tty sets `routed_seq`, and
  `shell_pid` only when it is unknown. `turn_started` gives a row without a route the
  key of its scope. efrd reads every slot at its start from these columns in one query.
  The index `conversations_tty` covers `(tty, route)`, and `conversations_route` finds
  the newest conversation of a key. Only a row with a `routed_seq`, which a terminal
  held, can be picked up.
- The search index (migration 9): `search_docs(seq, conversation_id, kind, text)`, one
  doc per indexed event, keyed by its sequence number: the text of a prompt (kind 0),
  a steer (1), an answer of the model (2) or the command line of a `shell` call (3),
  without the bodies of heredocs. `search_fts` is an FTS5 table over it: external
  content, `unicode61 remove_diacritics 2`, a prefix index of 3 characters, and merges
  in short steps (`automerge` 8, leaves of 1,000 bytes). `search_titles(conversation_id,
  title, first_seq)` holds the title of each conversation from its first indexed
  prompt. `search_state` holds the cursor (`indexed_seq`) and the version of the
  redaction. efrd writes these tables only from text that it cut, cleaned and
  redacted (the index never holds a secret that the redaction recognises, a control
  character, the output of a command or the contents of a file). They are not
  projections: the log cannot give them back without efrd's redaction, so a rebuild of
  the projections leaves them alone. The batch that ends a turn writes nothing here.
  The index `conversations_updated` (`updated_at, last_seq`) gives the recent list and
  the titles of a search newest first with no sort. A later delete of a conversation
  must also delete its `search_docs` rows, each with an FTS5 `'delete'`, and its
  `search_titles` row.

## Migrations

- SQL files in `crates/efr-store/src/migrations/`, named `NNNN_<noun>.sql`, embedded
  with `include_str!` and applied by `rusqlite_migration` (tracked in
  `PRAGMA user_version`, forward-only, one transaction per file).
- Before migrating, the daemon copies the database to `backups/`. The socket opens only
  after migrations finish.
- Milestone 1 ships `0001_events.sql`, `0002_conversations.sql`,
  `0003_receipts_outbox.sql`, `0004_shells_recordings.sql`,
  `0005_turn_messages.sql`, `0006_compactions.sql`, `0007_jobs.sql` (the jobs of
  phase C), `0008_routes.sql` (the route columns of the conversations, v0.0.4) and
  `0009_search.sql` (the search index, v0.0.4). Each step has a flag that says whether
  it changes a projection. The first start of an efrd with such a migration rebuilds
  the projections from the log once: about 3 s for 100,000 events in a debug build.
  `0009_search.sql` has no flag, so it rebuilds nothing. The devices and scopes table
  waits for the phone milestone, because a forward-only migration makes an unused
  table permanent.
- `0009_search.sql` creates the search tables empty, with the cursor at 0. After the
  start, efrd's indexer fills them in the background from the log (the backfill): it
  reads a page of at most 32 texts, 32 KiB of text or 20,000 events, cleans and
  redacts it, and writes it with one short writer job. After each page it waits 4
  times the time of the job, at least 5 ms, so the backfill takes at most a fifth of
  the writer; while a turn runs, it writes at most one page per second. During the
  wait it already reads and cleans the next page. When a round wrote more than one
  page, it then merges the index's segments in steps at the same duty, until a step
  does no work. A store of 20,000 turns takes about a minute and a half. Until the
  backfill ends, a search finds fewer conversations and says how far the index is.
  The search jobs run no checkpoint of their own; SQLite's checkpoint at 1,000 frames
  runs in the commit that crosses it, as before.
- The index only grows. It takes at most 10 MB per 1,000 turns of the bench corpus
  (whose text has long answers and heredocs), so at 20 turns a day about 73 MB a year.
  The file never gets smaller (no `auto_vacuum`), and each backup of `backups/` before
  a migration holds the index too. A change of what the redaction replaces increases
  `REDACTION_VERSION` (efr-conversation), and the next start empties the index and
  builds it again from the start of the log: a full backfill. The free pages of the
  old index are used again.
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
