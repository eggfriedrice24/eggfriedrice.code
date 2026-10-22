# Undo and diff

<!-- sources: README.md "Use" (the diff and undo bullet), "The auto mode" (deletes in the project, config files that run code); docs/sandbox.md "Undo", "Undo and jobs"; ARCHITECTURE.md "The project's git dir and undo"; docs/config.md [undo], [snapshot], [diff], [render]; crates/efr-cli/src/cli.rs (diff, undo); crates/efr-cli/README.md (the rows of efr diff and efr undo, the changes row, the end of a turn, groups of changed files); crates/efr-cli/src/commands/undo.rs, diff.rs and their snapshots; docs/adr/0008-snapshot-config-keys.md, 0009-undo-and-home-store.md -->

efr takes its own snapshots of your files while the agent works. With them, efr shows
what each turn changed, and `efr undo` takes a turn back. This works in every
registered project and in `$SCRATCH`, with or without git. efr keeps the snapshots in
its own store and never writes the project's `.git` to take one.

## What a turn changed

### While the turn runs

- When the agent writes a file with a file tool, the call shows the diff: its first 20
  lines, then a dim `… 12 more lines`. `render.diff_lines` sets the count, and `0`
  shows no diff.
- When a shell command changes files in the project or in `$SCRATCH`, one dim line
  under its result names them:

  ```text
  changed src/a.rs +3 -1 · deleted old.rs · new notes.md (+2 more)
  ```

These lines say what changed while the call ran, not who changed it. A file that your
editor saved at the same time shows too.

### At the end of the turn

A turn that changed files ends with a line such as `3 files changed, +24 -7`. When a
changed file is not code, the line names up to two groups:

```text
9 files changed, +635 -44 (evals +503, tests +77)
```

The built-in groups are:

- `generated`: lock files such as `Cargo.lock` and `package-lock.json`, `*.snap`
  files, and files below `fixtures/`;
- `tests`: files below `tests/`, `test/`, `__tests__/` or `spec/`, and names such as
  `a_test.go` or `a.spec.ts`;
- `docs`: `*.md` files and files below `docs/`;
- `code`: every other file.

You can add your own groups in `config.toml`. Your groups match first, in the order of
the file:

```toml
[diff.groups]
evals = ["evals/", "*.eval.toml"]
```

A glob without `/` matches the name of a file in any directory. A glob that ends in
`/` matches every file below a directory of that name. Other globs match the whole
path from the project root.

## efr diff

`efr diff` prints the whole diff of the last turn of this terminal's conversation into
your scrollback.

| Command | What it shows |
|---|---|
| `efr diff` | The diff of the last turn. |
| `efr diff --stat` | A table of the groups, then each file with its counts. |
| `efr diff --stat --all` | The totals of every turn of the conversation that was not undone. |
| `efr diff --turn <id>` | The diff of another turn. |
| `efr diff --conversation <id>` | The last turn of another conversation. |

```text
$ efr diff --stat
code   +1  -1
docs   +1
total  +2  -1

changed src/main.rs  +1 -1
new     notes.md     +1
2 files changed, +2 -1
```

In a pipe, `efr diff` writes the plain diff, for `git apply` or a pager. An ignored
file such as `.env` shows in the list, but efr does not show its contents.

## efr undo

`efr undo` takes back the newest turn of this terminal's conversation that is not
undone. It shows what it will do, and then asks:

```text
$ efr undo
undo turn 0193...: change a.txt and add notes.md
restore     a.txt
trash       notes.md
skip        b.txt (changed after the turn; --force overwrites it)
not covered .env.large (the snapshot left it out)
ref         main 1a2b3c4 -> 5d6e7f8
your git index is unchanged; git status can show staged changes
undo? y = yes, n = no
undone: 1 file restored, 1 file moved to ~/.local/share/efr/undo-trash/u-0193.../, 1 ref moved
```

The rows of the preview:

| Row | Meaning |
|---|---|
| `restore` | efr writes the file back as it was before the turn. The current file goes to the undo trash first. |
| `trash` | The turn made the file. efr moves it to the undo trash. |
| `delete` | efr removes a link or an empty entry that the turn made, after a copy in the undo trash. |
| `command` | efr runs a command that undoes a change, such as `crontab` for your crontab. |
| `skip` | The file changed after the turn. `--force` overwrites it. |
| `not covered` | The snapshot left the file out, such as a large ignored file. Undo never touches it. |
| `ref` | A branch, a tag or `HEAD` that the turn moved goes back. |

### What undo does and does not do

- Undo acts only on the files that the turn's own calls changed. A change that you
  made between two calls stays.
- Every file that undo overwrites or removes goes to the undo trash first:
  `~/.local/share/efr/undo-trash/<undo-id>/`. So you can get them back.
- Undo moves back the branches, tags, `refs/stash` and `HEAD` that the turn moved.
  Your git index stays as it is.
- Undo is a stack. You cannot undo a turn while a newer turn of the conversation is
  not undone, unless you add `--force`.
- An undo that did not complete, such as at a stop of efrd, goes on at the next
  `efr undo`.

### Options

| Option | What it does |
|---|---|
| `--list` | List the turns, and which are undone or stopped. Change nothing. |
| `--turn <id>` | Undo this turn. |
| `--conversation <id>` | Undo a turn of another conversation. |
| `--dry-run` | Show the preview. Change nothing. |
| `--yes`, `-y` | Undo with no question. Without a terminal, `efr undo` needs it. |
| `--force` | Also overwrite a file that changed after the turn, and undo a turn below a newer one. |
| `--redo` | Take back the newest undo. |

When only `--force` can act on the rows of the preview, `efr undo` changes nothing and
says so:

```text
efr undo changed nothing: 1 path acts only with --force; efr undo --force acts on it
```

### When undo refuses

Undo refuses while one of these runs in the conversation:

- a turn, a compaction or another undo;
- a command that still runs after its timeout;
- a background job. A job of another conversation that can write in the same project
  blocks undo too. Stop it first: `efr jobs stop j3`.

A prompt that you send during an undo waits for it.

When a job ends, its changes join the turn that started it, and the undo of that turn
needs `--force`. [Background jobs](jobs.md#jobs-and-undo) has more.

## Files outside the project

In `auto`, before the agent writes a file outside the project, such as `~/.zshrc`, efr
copies it into its store. In full mode, efr copies such a file when the command line
names it. In `manual` and `cautious`, efr makes no copy. After a copy, the turn ends
with a line such as:

```text
changed ~/.zshrc; efr undo reverts it
```

When a call can write a whole directory outside the project, efr copies the directory
first, up to `snapshot.home_max_files` (2000) files and `snapshot.home_max_mib`
(256 MiB). A larger directory gets no copy, and the end of the turn says so.

## Undo in each mode

Undo works for a turn of every mode that has a snapshot. The hint `efr undo reverts
this turn` shows only after an `auto` turn.

In `manual` and `cautious`, a command runs in your own shell, and its writes outside
the project are not covered. The preview of such a turn says so.

In `auto`, a delete in the project, such as `git reset --hard` or `rm -r build`, runs
with no question when efr's snapshot holds every file that it can touch, because
`efr undo` brings them back. The turn then says:

```text
changed 4 files (+12 -3) in <project>; efr undo reverts this turn
```

`shred`, `git clean -x` and a file that the snapshot left out still go to the
reviewer. [Undo](../sandbox.md#undo) has the full rules.

A phone cannot undo. The agent has no undo tool, and only you can change the `undo.*`
and `snapshot.*` settings.

## Settings

| Setting | Default | What it sets |
|---|---|---|
| `undo.enabled` | `true` | `false` turns undo off. In `auto`, deletes then go to the reviewer or to you. |
| `snapshot.enabled` | `true` | `false` takes no snapshot: no changed files, no `efr diff`, no undo. |
| `snapshot.max_file_mib` | `10` | Files above this size stay out of a snapshot. |
| `snapshot.ignored` | `"small"` | Which ignored files a snapshot takes: `none`, or `small` ones such as `.env`. |
| `snapshot.max_files` | `20000` | A project with more files gets no snapshot. |
| `snapshot.keep_turns` | `50` | The turns of each conversation whose snapshots stay. |
| `snapshot.max_age_days` | `30` | Days with no snapshot before efr deletes the store of a project. |
| `snapshot.mirror_max_mib` | `512` | The most MiB of git objects that efr copies, so undo can move refs back. `0`: no ref undo. |
| `render.diff_lines` | `20` | The lines of a file tool's diff that a call shows. |
| `diff.groups` | `{}` | Your own groups of changed files. |

[config.toml reference](../config.md#snapshot) lists every key.
