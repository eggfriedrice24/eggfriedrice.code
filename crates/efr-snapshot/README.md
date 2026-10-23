# efr-snapshot

## Purpose

efr's own snapshot store. It shows what the agent changed in files, in every directory
that a call can write, git repository or not, and it plans the undo of a turn (efr's
auto spec, section 10).

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
  the files it wrote to `touched` and moves `last` to a snapshot taken then, so a
  stop of efrd keeps the write. A call snapshot that drops before
  `Snapshots::after_call` (an interrupted call), or one marked `runs_on` (a command
  that runs past its timeout), marks its roots as not complete for the turn: undo
  acts on their paths outside `touched` only with `--force`.
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
- `Root::with_skip`: directories at the top of a root that no snapshot of it takes.
  efrd gives `$SCRATCH` the skip `jobs`, where it writes the output of each background
  job and keeps it itself. The listings of a snapshot leave such a directory out with
  an exclude pathspec, and the first snapshot of a store in a process (with no known
  tree) drops from the index what an older snapshot took below it
  (`git rm --cached`). The store's format does not change.
- `Snapshots::after_job` (`job.rs`): at the end of a background job, the files that
  the job changed join the turn that started it. The job's start call takes a
  `CallSnapshot` of the job's roots right after the start; at the job's end each root
  is compared with it. A path that another turn or an undo changed in the root since
  the job's start stays out: the `touched` paths of each turn's `last` or `post` and
  of each undo's `before` or `after` whose commit time is at or after the start. While
  the start turn runs, the job's paths join its `touched` paths as after a call, and
  the root is not complete for the turn. Once it ended, `post` goes, and `last` names
  `post`'s tree with the job's paths as the job left them (built in a second index,
  `<root-id>.build-index`, so the persistent index keeps its stat cache), with the
  job's paths in `touched` and the record not complete. The planner then reads
  `last`, so the undo of the start turn takes the job's writes back, and only with
  `--force`.
- `Snapshots::gc` (and `gc_capped`, with `snapshot.mirror_max_mib`): the newest
  `keep_turns` turns of each conversation stay, counted across every store, the home
  store with them, so a turn stays or goes whole: its refs in each root store, its
  mirror, its home copies and the refs of its undos (then `git prune` of loose
  objects older than an hour, and `git gc --auto` with `gc.auto=6700`, which packs the
  loose objects once there are about that many; the store's `gc.auto=0` holds for
  every other command). The objects that only the mirrors of a root store reach stay
  under four times `mirror_max_bytes`; above it, the mirrors of the oldest turns go
  first, and an undo of such a turn says `StoreFull` for its refs. A store without a
  snapshot for `max_age` goes whole; the home store counts from its last copy
  (`home.root`). The first tree of a running turn and the tree before a running call
  reach no ref yet, so the prune and the packing run with them pinned under
  `refs/efr-live/<tree>`. Only a `<16 hex>.git` is a root store and `home.git` the
  home store: another repository in the directory is never listed, trimmed or deleted.
  `gc_keeping` also keeps the turns that it names: efrd names the turn that started
  each background job that still runs.
- Undo (`undo.rs`): `Snapshots::plan_undo` plans the undo of one turn in each root
  whose store holds its `pre`. It acts only on the paths that the turn's calls changed
  (`touched` of `post`, or of `last` when efrd stopped in the turn). For each path it
  compares three states: in `pre` (the target), after the turn (`post`, the source)
  and on disk now (read with `lstat`, never through a link). A path as the turn left
  it goes back to `pre`; a path that already has its `pre` state is `AlreadyDone`; a
  path that changed after the turn, that a later turn that is not undone changed, of a
  turn with no `post`, or in a root with another `(dev, ino)` waits for `force`. A path
  that no call changed is `OutsideTurnCalls` (also with `force`, unless the record of
  the turn's paths is cut or a snapshot failed). A path that `pre` or `post` left out,
  or that the cut record may have left out, is not covered: undo never writes or
  removes it. A path that is now a directory, a special file or below a link is
  `Unknown`: never written, also with `force`. Each write over an existing path and
  each removal names the blob of what is there now, for the undo trash. A file that
  undo makes again gets the mode that a snapshot recorded, else 0600 when it is
  ignored, else git's mode. The plan writes nothing. `Snapshots::capture_before_undo`
  takes the "before" snapshot (`refs/efr-undo/<conversation>/<undo>/before`), puts the
  trash bytes into the store and writes the journal ref `started`;
  `Snapshots::finish_undo` takes the "after" snapshot and the marker
  `refs/efr/<conversation>/<turn>/undone`. An undo with `started` and no end resumes
  at the next plan of its turn, under its own id. `Snapshots::plan_redo` takes back
  the newest undo: the paths that it wrote go from the state after it (or `pre`, for
  an undo that stopped half way) to the state before it. `turns_with_refs` lists the
  turns, newest first, each with `auto` set when its first snapshot of a root recorded
  the mode `auto` or `full`, or when it has home copies: undo then covers what the
  turn wrote outside the roots; `full` is set too when that mode was `full`, for the
  label of `efr undo --list`; `stack_blocker` names the newer turn that is not undone, since
  undo is a stack.
- The refs of an undo (`undo/refs.rs`): each ref that differs between the mirror's
  record at `pre` and the record at `post` moves back by compare and swap; a ref that
  moved after the turn waits for `force`, and a root without the turn's mirror, or
  whose git dirs the daemon does not name as the recorded ones, moves no ref.
  `capture_before_undo` fetches the values that the undo moves away from into
  `refs/efr-undo/<conversation>/<undo>/mirror/`, so a redo can give them back after
  the project lost their objects.
- The home store (`home.rs`, spec 10.8): `$D/snapshots/home.git`, a bare repository
  with no index, for the paths outside the roots that a call can change. The daemon
  reads each path itself, without following a link, and hands the entries over
  through a `HomeReader`; the bytes stream into `git fast-import`, at most one file
  (16 MiB) in memory, so git never reads a home path and no filter runs. Each copy is
  one ref `refs/efr/<conversation>/<turn>/home/<n>`: `home_before` writes a "before"
  commit before the call, `home_after` an "after" commit, its child, after the call,
  and lists what changed (`HomeDelta`). A tree holds `files/<path>`, `state/crontab`
  and `state/linger`, and the manifest `efr-home.json`: per path a file with its whole
  mode, a link (stored as a link; its real file is an entry of its own), a missing
  path, a state, or why no copy covers it. A path that only the "after" lists was
  missing before, so undo moves it to the undo trash. The undo of a turn takes, per
  path, the state before the first copy and the state after the last (`HomePlan`); a
  path below a root of the plan belongs to the root's plan; a copy with no "after"
  (the call did not finish) is `CallUnfinished`, and with `force` its paths take what
  the applier finds (`Expect::Any`). `home_undo_before` copies the home paths that an
  undo changes, for its redo. Its root file `home.root` is the stamp of the last copy.
  A path with a part that git refuses in a tree (`git_refuses`: `.git` in any case and
  its NTFS names, `.gitmodules` for a link) is not covered (`LeftOut`): a copy never
  escapes the name, as a snapshot leaves a nested repository out, so a copy of a dir
  with a repository runs. Each entry can name its missing parents
  (`HomeEntry::missing_parents`, `parents` in the manifest, which an older copy does
  not have); a step that removes a path that was missing lists them in
  `HomeStep::remove_dirs`, the deepest first, for the applier to remove while they
  are empty.

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
takes a full snapshot. A snapshot never writes the project's `.git`. Only an applied
undo writes it, and only its refs and objects (`Snapshots::restore_refs`): `update-ref`
by compare and swap, `symbolic-ref HEAD` while `HEAD` still names what the plan
expects, and the objects that the project lost, from the store's `pack-objects` into
the project's `index-pack`, while the daemon holds the plan lock of the root. Its
config, its hooks and its index are never written. A snapshot reads its `info/exclude` (at
most 64 KiB, never through a link) and its `HEAD`; with the git dirs that the daemon
trusts (`GitDirs`), also its refs and objects, through `ProjectGit` (`project_git.rs`):
a second hardened mode with the project's git dir as `GIT_DIR`, no system or global
config, `-c core.hooksPath=/dev/null -c core.fsmonitor=false -c
core.alternateRefsCommand=`, no automatic gc or maintenance, bounded packing, and only
the commands that the module names. The same `-c` list is in the `--upload-pack` of
the mirror's fetch, because the `-c` of a fetch does not reach a local `upload-pack`.
Objects are written by `git add`, `commit-tree`, `hash-object` (the bytes for the undo
trash) and the fetches in the store, never hardlinked. One snapshot, commit or collection of a store runs at a time. Under that
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
files, links, ignored, large and build files and the file limit, the left-out record,
the modes, and a project whose config would run code at every git key it can;
`bytes.rs` exact bytes under any `.gitattributes`; `turns.rs` the turn's refs, `last`,
`touched`, a failed snapshot, a lost `pre`, and its diff with and without the patch;
`mirror.rs` the mirror and each reason for none; `cover.rs` the coverage check;
`undo.rs` undo, redo and the stack with an applier of the test that does what the
daemon's does, the refs by compare and swap and with objects that the project lost,
and an undo that stopped half way; `home.rs` the home store's copies and its part of
an undo, a copy of a dir with a repository and the missing parents of a path; `gc.rs` the bounds of the collector across stores, the mirror cap, the undo
refs, the packing and the home store; `jobs.rs` the skip of the job outputs, the files
of a job that join its start turn at its end without the paths of a later turn, and
the collector's keep set; `logs.rs` the
log lines of an add that fails (a file that git cannot read, skipped when the tests
run as root, and a git wrapper that stops with exit code 128). The tests run on the
developer's git and, in CI's shell job and `just test-shell-ubuntu`, on Ubuntu's.

`cost.rs` is the gate of 16.4: on a fixture the size of this repository (its sources,
a synthetic history of 1000 commits, the ignored files of 10.2), the median CPU time
(the process and its children) of 20 first snapshots of a new turn, with the mirror
and the ignored walk, must stay at most 200 ms, and the median wall time at most a
bound that grows with the fixture's fastest `git status` on the machine. It fails only
with `EFR_TEST_SNAPSHOT_GATE=1`, which `just test` and CI set in a serial step of its
own; otherwise it reports. It also reports a call, a coverage check and a walk of 2000
files. The numbers go to stderr and to the CI step summary:

```sh
EFR_TEST_SNAPSHOT_GATE=1 cargo nextest run -p efr-snapshot --success-output immediate -E 'test(/^cost::/)'
```

## Benches

The `bench_perf_*` benches are ignored. `just bench-perf` runs them in release, one at
a time. They report and do not fail on a time. Their fixtures are fresh git
repositories below `target/tmp/snapshot-worlds/`. A second store runs git through the
logging script of `World::logged` (`efr_scope::Git::with_program`), so each case also
reports the git processes that it starts.

- `bench_perf_snapshot_call` (`tests/it/perf.rs`): the first snapshot of a turn (with
  the mirror), one call with 0, 1 and 100 changed files, and the turn's end, in a
  repository of 100, 2,000 and 20,000 tracked files. Also 2,000 tracked files with a
  `target/` dir of 10,000 files that is ignored, and that is not ignored.
- `bench_perf_left_out` (`tests/it/perf.rs`): the turn's end with 300, 3,000, 10,000
  and 20,000 ignored files that the snapshots leave out, in wall time and in the CPU
  time of the test process.
