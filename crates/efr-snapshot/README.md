# efr-snapshot

## Purpose

efr's own snapshot store. It shows what the agent changed in files, in every directory
that a call can write, git repository or not (efr's auto spec, section 10; phase 4
without undo).

- One bare git repository per root in efr's data root: `$D/snapshots/<root-id>.git`,
  where `root-id` is the first 16 hex characters of the SHA-256 of the canonical root
  path. Its persistent index is `<root-id>.index` next to it. It keeps git's stat
  cache, so a later snapshot hashes only changed files. `<root-id>.root` holds the
  root's path for the collector. `$D` is masked in the sandbox, so sandboxed code
  cannot read, change or delete a snapshot.
- `Snapshots::before_call` and `after_call`: a tree of each `Root` before and after a
  call that can write, and the `FileChanges` of the call. The first snapshot of a root
  in a turn is the turn's `pre`. After a call, the index takes the root's changes and
  is compared with the tree before (`git diff-index --cached`); the new tree is written
  only when a later snapshot needs it.
- `Snapshots::before_write`: the turn's first snapshot of a root before a file tool
  writes into it. The tool's own diff lists its change.
- `Snapshots::finish_turn`: the turn's last tree of each root, the refs
  `refs/efr/<conversation>/<turn>/pre` and `/post` (commits with the root, the prefix
  of its paths and the project's `HEAD` in an `efr-meta:` line), and the turn's
  changes.
- `Snapshots::turn_diff`: a finished turn's changes and unified diff, for
  `conversation.diff`, with every root's paths under its prefix.
- `Snapshots::gc`: the refs of the newest `keep_turns` turns of each conversation stay
  in each store (then `git prune` of loose objects older than an hour, and `git gc
  --auto`), and a store without a snapshot for `max_age` goes whole. The first tree of
  a running turn and the tree before a running call reach no ref yet, so the prune
  runs with them pinned under `refs/efr-live/<tree>`.

What a snapshot takes (`Limits`): tracked files; new untracked files up to
`max_file_bytes` (a link as a link, never followed); at the first and last snapshot of
a turn, ignored files up to 1 MiB outside `SKIPPED_DIRS` (`target`, `node_modules`,
`.venv`, `venv`, `__pycache__`, `dist`, `build`, `.next`, `.cache`), such as `.env`. It
leaves out empty directories, nested repositories, and a root with more than
`max_files` files, which the debug log names. Counting the files of a root walks all
of it, so a skipped root stays skipped for 10 minutes without a count. The same size limits hold for a file
that the store already has: one that grows past its limit (`max_file_bytes`, or 1 MiB
for an ignored file) leaves the index, so no later call hashes it again, and it shows
as changed with no line counts, not as deleted. A call's own list does not show a new
ignored file; the turn's does.

A changed path shows with its root's prefix (`shown_prefix`): nothing for the turn's
project, `$SCRATCH/` for the conversation's scratch directory, `~/...` below the home
directory, else the absolute path.

## Tier

Tier 2.

## Allowed dependencies

`efr-scope` (`Git::command`, the hardened git command, and `Home`), `efr-protocol`
(`FileChanges` and the ids) and `efr-stdx` (the clock, atomic writes).
`xtask/src/deps.rs` holds the allowlist; `efr-daemon` is the only crate that depends on
this one.

Third-party crates: `tokio` (the git processes, the blocking pool and the store's
gate), `futures` (the roots of a call run together), `sha2` (the root id),
`serde_json` (the `efr-meta:` line), `jiff`, `thiserror` and `tracing`.

## Invariant

git runs only through `efr_scope::Git::command`, hardened in `runner.rs`: the store's
own `GIT_DIR` and `GIT_INDEX_FILE`, the root as `GIT_WORK_TREE` only for the commands
that read it, `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`, `-c
core.fsmonitor=false -c core.hooksPath=/dev/null -c core.untrackedCache=false`, no
automatic gc, no line-end conversion, literal pathspecs, and `--no-ext-diff
--no-textconv` on every diff. No filter or diff driver is defined, so a
`.gitattributes` in a root runs nothing. The project's `.git`, its config, its hooks
and its index are never written; only its `info/exclude` (at most 64 KiB, never through
a link) and its `HEAD` are read. Objects are written by `git add` in the store, never
hardlinked. One snapshot, commit or collection of a store runs at a time. Under that
gate, a lock that a killed git left (the index's or `packed-refs`'s) is removed, so a
timeout or a stop of efrd cannot block a store for good; a `git add` that a lock still
stops fails the snapshot instead of listing no changes.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-snapshot
```

The tests need a `git` binary. They run in temporary directories with git's system and
global config switched off. `tests/it/calls.rs` covers a plain directory and a git
project (whose `.git` and index must keep their bytes), every kind of change, binary
files, links, ignored, large and build files and the file limit; `turns.rs` the turn's
refs and its diff with and without the patch; `gc.rs` both bounds of the collector.
`cost.rs` measures one call on a copy of this repository and prints the numbers:

```sh
cargo nextest run -p efr-snapshot --success-output immediate -E 'test(/^cost::/)'
```
