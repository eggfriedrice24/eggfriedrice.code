# efr-scope

## Purpose

Turns the shell's working directory into an `efr_protocol::Scope`, again on every
turn, because the user may `cd /etc` between two `,` lines.

`derive(cwd, home, registry, git)` applies four rules in order:

1. inside a registered project root (the deepest one): `Project(id)`, even for `$HOME`
   or `/`, because registering is the one explicit way to make them a project;
2. `$HOME`, `/` or a directory above `$HOME`: `Machine`;
3. inside a git work tree, after the guards below: `Path(root)`;
4. anything else: `Machine`.

The result also carries the `Basis` (which rule applied) and the `Repo` (root and
branch) for the live-state preamble.

Modules:

- `registry`: `Registry`, the explicit project registry at
  `$XDG_CONFIG_HOME/efr/projects.toml` (`[[project]]` tables with `id`, `root` and an
  optional `name`; unknown keys refused). Read with `load` (a missing file is an empty
  registry), changed with `register` and `remove`, written atomically with `save`.
  `RegistryEdit` (`registry/edit.rs`) changes the file for `efr project add` and
  `remove`, which the daemon runs: it reads and checks the whole file first and never
  changes one with an error, edits the TOML document in place so every comment and the
  layout stay, and goes through `efr_stdx::fs::LinkedFile`, as `config.toml` does: it
  writes the file behind a symbolic link (a link to nothing is refused), and writes
  nothing when the file changed since it was read. A new file starts with a
  comment that says what it is.
- `git`: `Git::discover`, guarded discovery. git runs in the working directory through
  `efr_stdx::process::command` with `GIT_CEILING_DIRECTORIES=$HOME:/`, without
  `GIT_DIR`, `GIT_WORK_TREE` and the other variables that redirect it or inject
  configuration, with optional locks and terminal prompts off, and under a timeout on
  the injected `efr_stdx::time::Clock`. A work tree whose root is `$HOME`, `/` or a
  directory above `$HOME` comes back as `Discovery::Guarded`, never as a work tree.
  `Git::command` and `Git::run` are public, so the daemon runs its hardened
  `git status` of the `auto` sandbox (the counts of a destructive exit and the
  turn-end report) the same way.
- `dotfiles`: `detect_dotfiles`, the layouts that make `$HOME` a work tree: a `~/.git`,
  yadm (`$XDG_DATA_HOME/yadm/repo.git` and the two older places), and a bare
  repository among the dot directories of `$HOME` whose `core.worktree` is `$HOME`.
  The daemon runs it every turn and records the result in machine memory.
- `derive`: `derive`, `Derivation`, `Basis`.
- `home`: `Home`, the home directory in normal form and with links resolved. It is
  always a parameter; nothing in this crate reads the environment.

## Tier

Tier 1.

## Allowed dependencies

`efr-protocol` (`Scope`, `ProjectId`) and `efr-stdx` (the process constructor, the
clock for git timeouts, atomic writes). `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `serde` and `toml` (the registry file), `toml_edit` (changes of the
file that keep its comments), `thiserror`, and `tokio`
for the type of the git command that `efr-stdx` builds and for `spawn_blocking`.

## Invariant

A `.git` directory alone never makes a project, and `$HOME` and `/` are `Machine`
unless the user registered them. In a dotfiles home, git never climbs from a
subdirectory into `~/.git`, and a work tree rooted at `$HOME`, `/` or above `$HOME` is
never reported as a work tree.

The scope is derived again every turn from the current `ShellContext` and the current
registry, never cached. When derivation fails (a relative directory, git missing or
hung, a file system that does not answer), the caller uses `Machine`, the scope that
widens nothing. Derivation never blocks an async worker: every look at the file system
(`is_dir`, `canonicalize`, listing `$HOME`) and the start of git run on tokio's
blocking pool under the git timeout, because a `stat` on a hung network mount never
returns.

Known limits: a repository owned by another user (etckeeper's `/etc`) counts as no
repository, because git refuses it; register it to make it a project. A bare dotfiles
repository used only through `--work-tree=$HOME` on the command line leaves no trace on
disk and is not detected.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-scope
```

The tests need a `git` binary. Each one runs in a temporary directory with a `home`
inside it, passed as the home directory, and runs git with the system and global
configuration switched off, so no test reads the real `$HOME`. Timeouts use fake
clocks, so no test waits on real time. No network and no Zig.
