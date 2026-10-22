# 0019: Conversation search

Status: accepted, 2026-10-22.

## Context

The user asked for "a very good search" in `,resume`: the titles and the text inside
the conversations, with a live picker "kinda like neovim" (Telescope). The picker is
fzf, inline below the command line, and fzf is an optional program, as bubblewrap is.
efr is not a TUI. The same search must serve the phone later. The index must not slow
the end of a turn or the send of a prompt, and it must not use much memory or disk.

The facts that decide the shape:

- efr-store is the only owner of SQLite, with one writer thread. A long writer job
  delays every other commit, also the streamed text of a turn.
- efr-store may not reach efr-conversation, so the store cannot redact text.
- The bundled SQLite of rusqlite has FTS5. SQLite checks its interrupt flag between
  VDBE opcodes, but the FTS5 code never reads it. The merge of a prefix and the count
  of `bm25()` run as one step before the first row.
- A command line of a process is readable by every local user in `/proc`.
- A model-side peer of the socket holds the `read` scope.

This choice is expensive to reverse: it adds tables that only grow to every user's
database, a background writer beside the turns, and two methods that the phone will
call.

## Decision

1. **A background indexer in efrd.** One task keeps the index current with the event
   log. Its cursor is in the database (`search_state.indexed_seq`), so a crash or a
   restart loses nothing. It wakes after a batch that ends a turn or takes text back,
   waits 500 ms on the clock so that more turn ends join one round, and then reads the
   next page after the cursor on a read connection of its own. It applies the text
   rules on the blocking pool and writes the page with one short writer job. The batch
   that ends a turn writes nothing for the index.

2. **A duty cycle.** A backfill reads many pages. After each page or merge job that
   leaves more work, the task waits 4 times the time of the job, at least 10 ms, so it
   takes at most a fifth of the writer. While a turn runs, it writes at most one page
   per second. During the pause it already reads and cleans the next page. After a
   backfill of more than one page, it merges the FTS5 segments in steps of 64 leaves,
   at the same duty, until a step does no work. After the commit of a search job,
   when the write-ahead log holds 500 frames or more, SQLite runs a passive
   checkpoint in that job. So the index pays for the checkpoints of what it writes,
   and a turn's commit seldom reaches its own checkpoint at 1,000 frames.

3. **Only cleaned, redacted text goes in.** The indexed text is the user's prompts and
   steers, the model's answers, and the command lines of `shell` calls without the
   bodies of heredocs. Never the output of a command, the contents of a file, a diff,
   the command of a background job or a compaction summary. Each text is cut to 33,792
   characters in SQL, its control and format characters go, then each line goes
   through the redaction of the preamble with the marker `[***]`, after a pass for
   private keys and `name: value` secrets of config text. Then the text is cut to the
   cap of its kind (32 KiB for a prompt or a steer, 16 KiB for an answer, 2 KiB for a
   command). The marker has no letter or digit, so a search for `redacted` lists
   nothing. A change of what the redaction replaces must increase
   `REDACTION_VERSION`, and the indexer then builds the index again from the start of
   the log. A row whose rules panic gives no doc.

4. **The schema.** `search_docs` holds one doc per indexed event, with the event's
   sequence number as its key, so newest first needs no sort. `search_fts` is an
   external content FTS5 table over it, with `unicode61 remove_diacritics 2` and a
   prefix index of 3 characters. Its merges run in short steps (`automerge` 8, leaves
   of 1,000 bytes). `search_titles` holds a title per conversation from the first
   indexed prompt, so every row shows a redacted title. `search_state` holds the cursor
   and the version of the redaction. These tables are not projections: a rebuild of
   the projections leaves them alone. Migration 9 creates them empty and runs no
   projection rebuild.

5. **The backfill is no SQL in the migration.** The indexer fills the tables after the
   start, at the duty cycle. The store cannot redact, and a Rust hook in the migration
   would delay the start of efrd. Until the backfill ends, a search says how far the
   index is.

6. **Two read methods.** `conversations.search` gives one row per conversation, best
   first: a fuzzy score of the title (after fzf's algorithm) and the FTS5 rank of the
   best doc, and a newer conversation wins a near tie. The empty query gives the recent
   list. `conversation.search` gives the hits in one conversation, for the preview.
   efrd parses the query, so no FTS5 syntax comes from a client. Every word must match
   in one doc, and only the last word matches as a prefix of 3 or more characters.

7. **Search lanes and a time cap.** A search runs on one of 3 read connections of its
   own, never on the 4 readers of the turns, so a burst of keys cannot slow a prompt.
   A search reads at most 10,000 rows of the index, newest first, and ranks in Rust. At
   150 ms (the soft cap) the row loop ends and the search ranks what it saw. At 200 ms
   (the hard cap) efrd answers with `time_cap` and interrupts the statement. The hard
   cap is an answer deadline: one step inside FTS5 cannot stop, so the lane stays busy
   until that step ends. The query rules and the prefix index keep that step short.
   When the client goes away, the transport drops the handler, and the search stops
   the same way.

8. **User-side peers only.** A model-side peer may not search. Search would give it a
   content oracle over every project and the places of each redacted secret. The phone
   keeps search when its process is the user's.

9. **efr starts fzf.** `efr resume` with no words, on a terminal, with fzf 0.46 or
   newer on an absolute entry of the `PATH`, starts fzf as a child. fzf draws inline
   (`--height=40%`), never full screen, and clears its rows when it ends. efr writes
   the first rows to its stdin and reads the key and the row from its stdout. Without
   fzf, efr prints the numbered list as before.

10. **The query reaches efr in the environment.** fzf runs `efr search --picker` on
    each key and `efr search --preview` on each move of the focus. Both read the query
    from `FZF_QUERY`, never from `{q}` in their command line. Each waits a short time
    first (50 ms and 30 ms): fzf ends the previous command at the next key, so a fast
    typist starts no search per key. The command runs the same efr by its exact path.

11. **The words of `,resume <words>` stay private.** The plugin hands them to efr in
    `EFR_RESUME_WORDS`, and efr hands them to fzf as the first query in
    `FZF_DEFAULT_OPTS`, never in an argument. efr removes the user's own fzf defaults,
    so they cannot make a popup or a full screen.

Rejected:

- An index write in the batch that ends a turn: it makes the commit that the user waits
  for longer, a turn that a crash ends writes nothing, and a backfill needs a second
  path.
- A backfill in SQL inside the migration: the store cannot redact, and the start waits.
- A contentless FTS5 table: no text for the snippets and the marks. A trigram table:
  poor ranking, no match under 3 characters, and 2 to 4 times the size.
- A new crate for search: the store owns SQLite, and the bundled SQLite has FTS5.
- A picker in zsh: the resume loop, the question for a held conversation and the path
  of an older efrd are in efr, and `efr resume` typed by hand gets the same picker.
- `{q}` in the commands of fzf: the query would show in the process list.
- A TUI picker of efr's own: efr is not a TUI.
- Search for model-side peers.
- A `load:refresh-preview` bind: since fzf 0.41.1 a reload with `--disabled` runs the
  preview again, also for the same row, so the bind ran the preview twice per key.

## Consequences

- The database grows by the index: at most 10 MB per 1,000 turns of the bench corpus,
  so at 20 turns a day about 73 MB a year. The file never gets smaller (no
  `auto_vacuum`), and each backup before a migration holds the index too.
- A new `REDACTION_VERSION` costs a full backfill, and the old pages stay in the file
  for use again.
- The redaction is made for command lines. It misses prose such as "my password is
  hunter2", keys of lower case hex only, and a YAML value on the next line. The raw text
  stays in `events`, as before.
- After the upgrade, a search finds fewer conversations until the backfill ends, about
  two minutes for 20,000 turns.
- A lane can stay busy after its answer, at most for one FTS5 step. A fourth search at
  once waits for a lane, and gets `busy` after 200 ms.
- A turn's commit can still carry a WAL checkpoint, as before the index, when 1,000
  frames come between two search jobs. A search job that checkpoints is longer, and a
  turn's commit that comes during it waits for it.
- A later delete of a conversation must also delete its `search_docs` rows (each with
  an FTS5 `'delete'`) and its `search_titles` row.
- Only `efr search <words>` typed by hand keeps the words in its arguments, and its
  help says so.
- The real fzf runs only in `just test-fzf` on a developer machine, not in CI.
