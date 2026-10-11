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
  in a turn is the turn's `pre`: its commit and the ref
  `refs/efr/<conversation>/<turn>/pre` are written at once, so a stop of efrd in the
  turn keeps it. The commit's `efr-meta:` line holds the root, the prefix of its
  paths, the project's `HEAD`, the turn's mode (`Root::with_mode`), the root's
  `(dev, ino)`, the left-out record and the modes. After a call, the index takes the
  root's changes and is compared with the tree before (`git diff-index --cached`). A
  call that changed files writes its tree, adds its paths to the turn's `touched`
  paths of the root, and moves `refs/efr/<conversation>/<turn>/last` there (`touched`
  in its line, at most 5000 paths, then `touched_more`). A change between two calls,
  such as an edit of the user, is in no call's list, so it never joins `touched`.
- `Snapshots::before_write`: the turn's first snapshot of a root before a file tool
  writes into it. The tool's own diff lists its change; `Snapshots::after_write` adds
  the files it wrote to `touched`.
- The mirror (`mirror.rs`): at the turn's first snapshot of a root with trusted git
  dirs (`Root::with_git`), the store fetches the project's `refs/heads/*`,
  `refs/tags/*`, `refs/stash` and a detached `HEAD` into
  `refs/efr-mirror/<conversation>/<turn>/`, and the `pre` line records their values,
  `HEAD`, the git dirs and their `(dev, ino)`. `post` records the values at the end.
  `refs/remotes/*` stays out. No mirror, and `TurnEnd::not_covered` says
  `refs: <why>`, for a root with a `.git` but no trusted dirs, a git dir that is a
  link or that changed, alternates, a `commondir` file where none belongs, a shallow
  clone, an object dir above `Limits::mirror_max_bytes` (an `lstat` walk that follows
  no link; 0 turns the mirror off), a failed fetch, or a fetch that adds more than
  `mirror_max_bytes` to the store (its refs go again). A linked work tree fetches
  from its own git dir, so its `HEAD` is its own.
- `Snapshots::covers` (`cover.rs`): whether the turn's snapshot holds what a
  destructive command can destroy (`RootCover`), so that undo can bring it back. It
  takes the root's snapshot now (the turn's `pre` and mirror when they are missing)
  and checks positively against the fresh tree, never against the left-out record. A
  path is covered when it lies below the root, is not the root, is in no git dir and
  each file at or below it (an `lstat` walk, at most `max_files`) is in the tree, or
  when it does not exist. `Tracked` needs the project's index to equal `HEAD`, the
  commit of `HEAD` in the store, and each tracked file that exists in the tree.
  `Untracked` needs no untracked file left out. `Refs` needs the turn's mirror, and
  `Stash` also a stash of at most one entry. A failed snapshot of the root covers
  nothing (`CoverResult`).
- A snapshot that fails marks the root for the turn. When the first one fails, no
  later snapshot becomes `pre`, because a call of the turn may have written in
  between.
- `Snapshots::turn_end` (`TurnEnd`): the turn's last tree of each root as `post`, a
  child of `pre` (`last` goes, `post` holds `touched`), and the turn's changes, the
  `pre` and `post` of each root (`SnapshotInfo`), what they left out, and the roots
  whose record is not complete: a snapshot that failed, or a `pre` ref that is gone,
  as when an approved command outside the sandbox changed the store.
  `Snapshots::finish_turn` returns only the changes.
- `Snapshots::turn_diff`: a finished turn's changes and unified diff, for
  `conversation.diff`, with every root's paths under its prefix. When one root lies
  inside another, the files below the inner root come from the inner root alone, in
  the list and in the patch, so each file shows once. A changed file that the ignore
  rules name (a small ignored file that a snapshot took, such as `.env`) stays in the
  list, but the patch holds only a line `<path>: ignored file, content not shown`: it
  may hold secrets that nobody read in this turn, and every read-scope client and the
  scrollback get the patch. `turn_end` records these paths in the `efr-meta:` line of
  `post` (`hidden`). A turn with only `pre`, which efrd stopped before its end, has no
  record after the turn and shows no changes.
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
of it, so a skipped root stays skipped for 10 minutes without a count. A root without a
snapshot yet lists its ignored files only after the count, so a root that is too big
never pays for that listing; a root with a snapshot lists both at the same time. The same size limits hold for a file
that the store already has: one that grows past its limit (`max_file_bytes`, or 1 MiB
for an ignored file) leaves the index, so no later call hashes it again, and it shows
as changed with no line counts, not as deleted. A call's own list does not show a new
ignored file; the turn's does.

Each snapshot keeps a record of what it left out, with a reason (`LeftOutReason`): a
file above the size limit, an ignored file above 1 MiB, past the 2000 ignored files, in
a build directory or while `snapshot.ignored` is not `small`, a file that git could
not read, a name that is not UTF-8, a nested repository, or the whole root. It also
keeps the mode of each file that it looked at when the mode is not 0644 or 0755, such
as a `.env` with 0600. The `efr-meta:` line of the turn's commits holds both (at most
1000 entries each, `left_out_more` counts the rest). The record is for the report
only: whether a snapshot holds a file is a check against its tree, never a look at
this list. `blob_id` and `link_id` give the id that git gives a file's bytes or a
link's target, so a caller can compare a file with a snapshot without git.

A file that git cannot read, such as a file that only root may read, stays out of the
tree. `git add --ignore-errors` adds the rest. The debug log names the count of these
files and the first path, once per root for the life of the process, from the first
8 KiB of git's standard error. A git that stops with exit code 128 adds nothing, and
the log gives a warning with git's `fatal:` line at each snapshot.

The lists say what changed in a root between two snapshots, not who changed it. A
write by another conversation's call, or by the user's editor, between a call's
snapshots (or a turn's first and last) shows in that call's list (or turn's) too,
also in `efr diff`. The plan lock of `auto` does not cover the snapshot after a call,
and the other modes take no lock for a `shell` call.

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
gate), `futures` (the roots of a call run together), `sha2` (the root id), `sha1` (the
blob ids), `serde` and `serde_json` (the `efr-meta:` line), `jiff`, `thiserror` and `tracing`.

## Invariant

git runs only through `efr_scope::Git::command`, hardened in `runner.rs`: the store's
own `GIT_DIR` and `GIT_INDEX_FILE`, the root as `GIT_WORK_TREE` only for the commands
that read it, `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`, `-c
core.fsmonitor=false -c core.hooksPath=/dev/null -c core.untrackedCache=false`, no
automatic gc, no line-end conversion, literal pathspecs, and `--no-ext-diff
--no-textconv` on every diff. No filter or diff driver is defined, so a
`.gitattributes` in a root runs nothing. The store's own `info/attributes` turns off
`text`, `eol`, `ident`, `filter` and `working-tree-encoding`, and git reads no other
attribute file outside the root (`core.attributesFile=/dev/null`,
`GIT_ATTR_NOSYSTEM=1`), so each blob holds the exact bytes of its file. A store from
before this rule has no `efr.format=2` in its config: it drops its index once and
takes a full snapshot. The project's `.git`, its config, its hooks
and its index are never written by a snapshot. A snapshot reads its `info/exclude` (at
most 64 KiB, never through a link) and its `HEAD`; with the git dirs that the daemon
trusts (`GitDirs`), also its refs and objects, through `ProjectGit` (`project_git.rs`):
a second hardened mode with the project's git dir as `GIT_DIR`, no system or global
config, `-c core.hooksPath=/dev/null -c core.fsmonitor=false -c
core.alternateRefsCommand=`, no automatic gc or maintenance, bounded packing, and only
the commands that the module names. The same `-c` list is in the `--upload-pack` of
the mirror's fetch, because the `-c` of a fetch does not reach a local `upload-pack`.
Objects are written by `git add`, `commit-tree` and the mirror's fetch in the store,
never hardlinked. One snapshot, commit or collection of a store runs at a time. Under that
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
refs and its diff with and without the patch; `gc.rs` both bounds of the collector;
`logs.rs` the log lines of an add that fails (a file that git cannot read, skipped
when the tests run as root, and a git wrapper that stops with exit code 128).
`cost.rs` measures one call on a copy of this repository and prints the numbers:

```sh
cargo nextest run -p efr-snapshot --success-output immediate -E 'test(/^cost::/)'
```
