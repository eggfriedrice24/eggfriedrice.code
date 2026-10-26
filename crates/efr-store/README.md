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
  to `backups/efr.sqlite.<user_version>`. Then the projections are rebuilt from the
  log, but only when an applied step adds or reshapes a projection (`REBUILDS`, one
  flag per step); `0009_search.sql` adds only the search tables, so the start after it
  does not replay the log. `MigrationReport::rebuilt` says whether a rebuild ran.
- `StoreWriter` and `WriterHandle`: the single writer. A thread owns the only
  read-write connection; `WriterHandle::append(Batch)` commits events, the projections
  derived from them, command receipts and outbox rows in one transaction, and then
  broadcasts the committed events in commit order.
- `Readers`: read-only connections used through `spawn_blocking`, behind
  `Readers::with(|conn| ...)`. An in-memory store runs reads on the writer's
  connection with `query_only` set, because a private in-memory database has one
  connection.
- `events`: the event log, append-only except for the delete of a conversation. `read_after(seq, limit)` and the per
  conversation reads return `EventEnvelope`s; a kind this build does not know reads
  back as `Event::Unknown`. `read_turn_history_without` is the page of a turn's start
  without the body events (`BODY_KINDS`: the assistant's text, each tool call and its
  result) of the turns that the caller names. SQLite never copies such an event out,
  the event still counts towards the limit, and every event of the page is the event
  of the log.
- `conversations`, `approvals`, `shells`, `compactions`, `jobs`: projections of the log for
  listing and paging, rebuilt from the events by `WriterHandle::rebuild_projections`.
  `compactions::latest` reads the newest `conversation_compacted` with a summary and
  the newest of any kind with a query of their own, so the page of newest events never
  decides what the model sees. A turn ends
  with `turn_completed`, `turn_failed`, `turn_interrupted`, `turn_cancelled` or, for
  a queued prompt that the user took back before it started, `prompt_withdrawn`
  (status `withdrawn`), so a restart never cancels a withdrawn prompt again.
- `receipts`: one receipt per command id. A duplicate command id returns the stored
  outcome, and a rejected command stays rejected. A receipt records the sequence
  number of its batch's last event, or of the event that `NewReceipt::seq_of_event`
  names, such as `prompt_queued` when `turn_started` follows it, so a retry gets the
  number the first answer reported.
- `turn_messages`: the exact messages of finished turns as the provider saw them,
  with its own items (encrypted reasoning, item ids) that no event holds, keyed by
  turn and position with the provider and the model. A batch saves them with the
  event that ends the turn (`Batch::turn_messages`), keeps every turn until a summary
  covers it, because every request sends those turns word for word, and
  `of_conversation` reads them back, so a conversation that goes on after a restart
  sends the same request. `of_conversation_except` reads only the turns that the
  reader does not hold, and gives the ids of the others. They are not a
  projection: a rebuild leaves them alone. A `conversation_compacted` with a summary
  drops, in its own batch, the saved turns before its cut, and its `through_turn` too
  when the cut covers that whole turn: the summary takes their place in every later
  request. A compaction that only pruned drops nothing.
- `jobs` (phase C, migration 7): the background jobs of each conversation, one row per
  job from its `job_started`, `job_ended` and `job_watch_matched` events, with the
  launcher's process id and start time from `job_started` (never from a file that a
  job can write), and one row per end or watch match with the turn whose `job_notice`
  took it. The forwarded ports of a job (`JobInfo::forwards`) come from its
  `job_port_forwarded` events: `forwarded` adds a port, `stopped` removes it, and the
  job's end removes all of them; the other outcomes change nothing, and no such event
  is a notice. `next_number` seeds the ids of new jobs, `running` finds the jobs that ran
  when efrd stopped, `of_conversation` and `all` list them newest first, and
  `undelivered_notices` reads the notices that no turn took. The first
  `job_notice` with `woke` of a turn makes that turn's row in `turns`, with
  `wake = 1`, no command id and the prompt `job notice: j3, j4`; the `turn_started`
  after it in the same batch sets `started_at`. `wake_starts_since` reads the start
  times of such turns for the limit of turns that notices start per hour, so a
  restart does not reset it.
- `conversations` keeps the slots of the terminals (migration 8): `route`, `shell_pid`
  and `routed_seq` of each row with a tty, from `conversation_created`,
  `terminal_attached`, `prompt_queued` from the row's tty and `turn_started` (which
  fills a missing route from the scope). An old `conversation_created` with no route
  takes every row of its tty, so a rebuild of an old log gives the rows that its daemon
  had. `slots` reads every slot in one query for efrd's start, `of_tty` lists one
  terminal's conversations with its current one first, `newest_of_route` finds the
  candidate of a pick-up, and `route` reads one conversation's key. A route that this
  build cannot read counts as `outside`. `recent` lists the conversations of
  `,resume`: those of one key first, then the others, each part newest first, with
  their prompt turns (a job notice turn and a withdrawn prompt do not count).
- `user_messages`: `of_conversation` reads every prompt and steer of a conversation,
  the events that take one back, and each `terminal_attached`, from the whole log. The
  record of an exit in `auto` keeps the messages for the reviewer, and the name check
  reads those of the newest two turns after the last `terminal_attached`, also when the
  page of newest events does not reach back to them.
- `search` (migration 9): the search index of the conversations. `search_docs` holds
  one doc per indexed event, keyed by its sequence number: the text of a prompt, a
  steer or an answer, or the command line of a `shell` call. `search_fts` is an FTS5
  index over it: external content, `unicode61 remove_diacritics 2`, and a prefix
  index of 3 characters, so a search expands no prefix shorter than 3. Its merges run
  in short steps (`automerge` 8, leaves of 1,000 bytes). `search_titles` holds the
  title of each conversation, from the first prompt doc that is not blank.
  `search_state` holds the cursor and the version of the redaction. efrd's indexer
  fills the tables in the background. `pending` reads the next page of source text
  after the cursor: at most `PAGE_ROWS` texts, `PAGE_BYTES` of text or `PAGE_SCAN`
  events, each text cut to `SOURCE_CHARS` characters in SQL. efrd bounds, cleans and
  redacts the text. Then `WriterHandle::search_apply` writes the docs, the titles, the
  docs that a withdrawal or a resent steer takes back, and the cursor in one
  transaction, or nothing when the cursor or the redaction changed (`Stale`).
  `search_reset` empties the index for a new redaction version, and `search_merge`
  merges its segments toward one after a backfill. After the commit of a search job,
  when the write-ahead log holds 500 frames or more, SQLite runs a passive checkpoint
  in that job (`SEARCH_CHECKPOINT_FRAMES`), so the index pays for the checkpoints of
  what it writes. A turn's commit runs one at 1,000 frames (`WAL_AUTOCHECKPOINT`,
  SQLite's default), which beside the indexer it seldom reaches. The tables
  are not projections: a rebuild leaves them alone. The batch that ends a turn writes
  nothing here. The delete of a conversation deletes its `search_docs` rows (its
  sequence numbers through `events_conversation`, each with an FTS5 `'delete'`) and its
  `search_titles` row, and `search_apply` writes no doc of a deleted conversation, so a
  page that the indexer read before the delete adds none of its text.
- `search` reads: `search::search` finds the conversations whose titles or text match
  a query, one row per conversation, best first; `search::search_in` finds the hits in
  one conversation for the preview; `titles`, `index_percent`, and
  `conversations::short_ids` and `conversations::rows` give what a row shows. The store
  parses the query itself (`search::query`), so no FTS5 syntax comes from a client:
  every word must match in one doc, and only the last word matches as a prefix of 3
  or more characters. A search reads at most 10,000 rows of the index, newest first
  with no sort, and the titles of the 10,000 newest conversations; it ranks in Rust
  (`search::rank`: `bm25()` with a weight per kind, and the fuzzy title score of
  `search::fuzzy`, after fzf), so it never scores every match before the first row.
  The title score leaves out a word with no letter and no digit, and no word matches
  inside the `[***]` marker of a redacted secret, so a search cannot list the titles
  that held a secret.
  FTS5 merges the doclists of every word of a prefix in memory before the first row
  when no prefix index has its length, which for a common prefix costs megabytes and
  tens of milliseconds. So a prefix longer than the prefix index of 3 characters
  becomes an `OR` of the words of the index that start with it, when they are at most
  64 (from an `fts5vocab` table in each search connection's temp schema), which FTS5
  reads lazily; a prefix of more words becomes its first 3 characters, which the
  prefix index serves, and the search keeps only the docs whose text holds the query
  (checked in Rust, also in the preview). `bm25()` runs only for a query below the row
  cap with no long prefix left; else a hit counts by its kind and the newer
  conversation wins.
  The snippets of the list come from the docs by sequence number (`search::snippet`),
  with no FTS5 statement per row; the preview runs one FTS5 statement over the range
  of the conversation's events, with `highlight()` (`search::marks`). Ranges count
  Unicode code points. The modules `query`, `fuzzy`, `rank`, `snippet` and `marks` are
  pure (`PURE_MODULES` in `xtask/src/tidy.rs`).
- `SearchLanes` and `IndexReader`: the read connections of search, apart from the
  `Readers` of the turns. Three lanes with a semaphore of their own; a search waits for
  a lane on the injected clock and fails with `StoreError::SearchBusy` when none frees
  in time. The permit lives in the blocking work, so a search whose caller stopped
  waiting holds its lane until it ends. `SearchStop` ends a search: `soft` at the next
  row, `hard` also with an interrupt of the running statement. The lane takes its
  interrupt out of the stop before the connection serves another search, so a late
  stop never interrupts the next one. One step inside FTS5 (the merge of a prefix,
  the count of `bm25()`) reads no interrupt and runs to its end. `IndexReader` is the
  one connection of efrd's indexer. Each of the four connections sets a page cache of
  512 KiB. That is no hard cap: the bundled SQLite puts the page caches of all
  connections of the process in one group whose limit is the sum of their sizes, so a
  lane can also use the room that the writer and the readers leave (up to about 3 MiB
  for one lane in `bench_search_memory_xl`). An in-memory store runs both on the
  writer's connection, with the flags of
  the stop but no interrupt.
- `deletes` (migration 10): the deletes of conversations. The event
  `conversation_deleted` makes the writer write the tombstone row
  (`conversation_deletes`, with the PTYs and undos that the conversation's events name)
  and delete the conversation's projection rows in the same batch, so from that commit
  on no read finds the conversation, and every later event of it fails its batch
  (`UnknownConversation`, or `ConversationExists` for a new `conversation_created` of
  its id). `WriterHandle::delete_step` then removes the rest in short transactions
  (`STEP_EVENTS`, 2,000 events with their search docs and receipts each; the last one
  the saved messages, the search title, the outbox rows and the recording segments),
  each of which a stop may break off and a later step does again, and
  `WriterHandle::delete_done` marks the end once efrd removed the files too.
  `pending` lists the deletes that a start of efrd finishes. The trigger of the log
  lets only an event of a deleted conversation from before its tombstone go; the
  tombstone event stays, so the sequence numbers never go back. A rebuild of the
  projections skips the events of a deleted conversation.
- `devices` (migration 11): the enrolled devices, such as a phone. One row per device:
  its id, its name, its P-256 public key (65 bytes, X9.63), the fingerprint of the key,
  the Tailscale `Node.StableID` and `UserProfile.ID` of its pairing, efrd's tailnet
  `StableID`, its scopes, and the times of its pairing, of its last proof and of its
  revoke. `WriterHandle::device_insert` writes a row after the user confirms the
  pairing; `devices::get` and `devices::list` read them. `device_revoke` keeps the row
  and sets its time once, and after the commit it sends the device's id to each
  receiver of `subscribe_revocations` (at most `REVOCATION_CAPACITY` behind; a receiver
  that lags reads its rows again). `device_touch` sets the time of the last proof of an
  active device only. One active device per key (a partial unique index); a revoked
  key can pair again as a new device. `NewDevice::new` refuses a value of the wrong
  shape: a name that a terminal cannot show as it is, a fingerprint or StableID that
  is not printable ASCII, a key that is not 65 bytes from `0x04`, and the `admin`
  scope. The store does not check the key's point or derive the fingerprint; efrd
  does. Not a projection: no event holds a device, and a rebuild leaves the table
  alone.
- `outbox`: durable side effects, enqueued with the batch that decides them and
  claimed in id order through `WriterHandle::outbox_claim` and `outbox_done`.
  Replay-safe rows survive a restart; process-bound rows are cancelled at startup by
  `WriterHandle::outbox_cancel_process_bound`.
- `recording`: PTY recordings. Append-only segment files under
  `recordings/<pty_id>/<start_seq>.rec`, each chunk behind a 16-byte header with a
  timestamp, rotated at 8 MiB, indexed in `recording_segments`. Offsets are stream
  sequence numbers: byte `n` of a PTY's output has `Seq` `n`, which is what
  `pty.attach(since_seq)` and the shell marks use. `RecordingWriter::resize` stores a
  new size of the PTY at the current offset; it takes no stream bytes.
  `Recordings::read_range` slices the bytes and returns the sizes in the range.
  `Recordings::collect` runs one pass of the collector (`recording/retention.rs`) with a
  `Retention`: the recording of a PTY whose shell exited more than `max_age` ago goes
  whole, and the oldest segments go first while all files pass `max_bytes`. The newest
  segment of a PTY that can still start a writer stays, with the segments before it up
  to `live_tail` stream bytes. A PTY can write while its shell runs, while the shells
  projection does not know it, and for `WRITER_GRACE` (an hour) after its exit. The
  rows go in one writer transaction, then the files; a file without a row is an orphan
  that the next pass removes. A read of a PTY whose first bytes went reports a later
  `RecordedRange::start`.
- `Store` and `StoreConfig`: open, back up, migrate and start the writer, the readers,
  the search lanes and the index reader in one call.

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

Dev-dependency: `efr-test-support`, for the seeded search corpus of the integration
binary `tests/it`. That binary links the one `efr-store` library that
`efr-test-support` links too, so the types agree; the unit tests in `src/` do not use
it, and keep their own small helpers in `testing.rs`.

## Invariant

- No other crate reaches `rusqlite`; `cargo xtask deps` enforces that, and clippy's
  `disallowed-methods` sends every `Connection::open` to `efr_store::db`.
- One writer: only the writer thread holds a read-write connection, so commits are
  serial and the broadcast is in commit order without a further lock. A subscriber
  that subscribes, then reads the high-water mark, then forwards live events above it
  misses nothing and sees nothing twice.
- The event log is append-only (triggers refuse `UPDATE` and `DELETE`; the one delete
  is of the events of a deleted conversation from before its tombstone event, which
  stays), `seq` starts at 1 and only grows, also over a delete, and events carry full state, so projections are a function of the
  log and can be rebuilt at any time.
- A batch is all or nothing: events, projections, receipts, outbox rows and turn
  messages commit in one transaction or not at all, and a failed batch does not use up sequence numbers.
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

The integration binary `tests/it` runs the search index on a database file of the
seeded corpus of `efr-test-support`. `index_disk_fits_its_budget` (a normal test)
backfills corpus S (1,000 turns) and checks that the search tables use at most 10 MB
per 1,000 turns; it prints the size of each table and of the prefix choices. The
benches `bench_search_apply_round` (1,000 live rounds of one turn) and
`bench_search_backfill_page` (the backfill and the merges of corpus L, 20,000 turns)
are ignored. `just bench-search` runs them in release, one at a time, and they fail
when a budget fails: a round at a p95 of 2 ms, a page at a p95 of 4 ms, and a max of
10 ms for both. Each p95 counts the jobs that carry no checkpoint of the write-ahead
log. The max of a round counts every job, the checkpoints of the search jobs too; the
max of a page counts the pages without a checkpoint only, because a checkpoint's flush
takes 10 to 150 ms on some disks and the backfill runs once, in the background. The
jobs with a checkpoint are on a line of their own. `bench_search_reset` reports the one job
that empties the index of corpus L and of XL after a change of the redaction; it has
no budget. The normal test
`search_jobs_checkpoint_and_turn_commits_seldom_do` checks where the checkpoints run.

The module `search_query` of `tests/it` checks the reads on the corpus:
`rust_snippet_agrees_with_highlight` (the ranges of the Rust snippets equal those of
FTS5's `highlight()`), `queries_find_their_conversation_first` (a fixed list of
queries from titles, with the rank that each must reach) and
`fts5_step_runs_past_the_interrupt`. Its benches, also ignored and run by
`just bench-search`, hold the budgets of the reads: `bench_search_first_row` (the
first row of the worst queries on corpus XL, 10,000 conversations, at most 150 ms),
`bench_search_query_s`, `_l` and `_xl` (200 queries of a fixed list: a p95 of at most
30, 50 and 60 ms, a rare word at most 30 ms on L, a max of 200 ms on XL),
`bench_search_empty_query` (a p95 of 10 ms on XL), `bench_search_preview` (30 ms on L,
40 ms on XL), `bench_search_snippets` (50 rows in 5 ms), `bench_search_titles_10k`
(5 ms), `bench_search_cancel` (a stop between rows frees the lane within 10 ms; the
time inside an FTS5 step is reported) and `bench_search_memory_l` and `_xl` (the
list on two lanes at once, each corpus in a process of its own: at most 12 MiB at the
peak and 10 MiB at rest over the start; the rest counts the heap that the allocator
keeps for the threads after the searches end, so it also prints the rest after the
list twice more). Each corpus file is
built once below `target/tmp/search-corpus/`, named by size, seed, schema and
generator version, and each test copies it. The numbers of a busy machine are not
valid.

## Benches

The `bench_perf_*` benches are ignored. `just bench-perf` runs them in release, one at
a time. They report, and only a bench whose budget the list names fails on a time.
Each one writes a log into a new store file below `target/tmp/perf-store/` and
measures after each part of the log. The first two write the perf corpus of
`efr-test-support` (20 conversations, interleaved by turn).

- `bench_perf_user_messages` (`tests/it/perf.rs`): the time of
  `user_messages::of_conversation` at 10k, 100k and 1M events in the log, the slope,
  and the lines of `EXPLAIN QUERY PLAN` of its query.
- `bench_perf_store_append` (`tests/it/perf.rs`): the p50, p95 and max of one append of
  an output update and of a turn's batch at 1k, 100k and 1M events, and of an output
  update when the write-ahead log is near the writer's checkpoint size.
- `bench_perf_previous_turn` (`tests/it/perf.rs`): the start of the previous turn
  for the newest turn of one conversation of 20, 200 and 2,000 turns of its own,
  from `conversations::previous_turn_start` and from the old read of every turn.
  This bench has a budget: at 2,000 turns the scalar query takes less than a third
  of the old read and less than 2 ms.
