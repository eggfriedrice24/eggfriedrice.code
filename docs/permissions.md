# Permissions

Before a tool call runs, the daemon decides one of three effects: `allow` (the call
runs), `ask` (the turn waits until you approve or deny the call) or `deny` (the model
gets an error that names the reason). `efr-permissions` holds the rules and the
engine. `efr-conversation/src/turn.rs` is the only place that applies them. This file
describes the permission modes, the rules of each, how the engine reads a command
line, how efr keeps its own config out of reach, and how you add your own rules in
`config.toml`.

## Permission modes

Every turn runs in one of three modes. The mode is the built-in policy of the turn,
and your `[[permissions.rules]]` come after it, so your rules win where they match in
every mode.

| Mode | What runs without a question |
|---|---|
| `manual` | Nothing, except what your rules allow. Every read, write, command and network access asks; secrets are denied. |
| `cautious` | The default: reading outside secrets, writing in `$SCRATCH` and in the turn's registered project, and the read-only commands of the table below. |
| `auto` | Everything that `cautious` allows, plus the curated list of the `auto` table: the writer programs, the project's build, test, format and lint tools, and local git. |

`permissions.mode` in `config.toml` sets the mode of a turn when the prompt names none.
A turn keeps its mode until it ends. `auto` runs the build and test code of the
directory without a question: `cargo test`, `npm test` and `make` run whatever the
code, its build scripts and its git hooks do. That code is not only the project's own.
The model writes freely in the project and in `$SCRATCH`, so in `auto` it can write a
`Makefile`, a `build.rs` or a `package.json` script and then run it with `make`,
`cargo build` or `npm run`, all without a question, also in a turn with no registered
project (in `$SCRATCH`). Such code runs with your rights and with full network access:
it can send your files out, and it can write `config.toml`, which the daemon reloads
without a question. So the engine's limits, config protection and the approval of the
settings tool do not hold against a model that follows injected instructions (from a
web page, a file or a command's output) in `auto`. Choose `auto` only for work where
you accept that. The commands that `auto` itself allows never reach the network
outside the package rows; see "The auto table".

A registered project is a directory that you list in `projects.toml` in efr's config
directory. A turn whose hidden shell is in a project's root or below it runs in that
project: `cautious` writes freely below the root, and `auto` also runs the project's
build, test and git commands there. `efr project add` registers the git work tree
that holds the current directory (or the directory itself), `efr project add PATH`
registers PATH, `efr project list` shows the projects and `efr project remove PATH`
takes one out. The daemon makes the change: it keeps the comments of the file and a
link to it, and the next tool call uses the new set of projects. The home directory
and `/` are a project only when you name them. No tool may write `projects.toml`; see
"Config protection".

A turn from the phone runs with at most `cautious`: `auto` there counts as
`cautious`, and the engine asks for everything outside `$SCRATCH` anyway.

## How the engine decides

A tool call declares what it needs: the paths it reads or writes, the command line it
runs, network access, and input at the terminal. The engine gives each requirement an
effect. The strictest effect decides the call, in the order `allow < ask < deny`.

1. The machine policy sets the effect of each requirement. It is the built-in policy
   of the turn's mode, then your `[[permissions.rules]]`. The last rule that matches
   wins, so your rules win where they match. A requirement that no rule matches is
   denied. For a secret, only a rule that names secrets decides; see "Your rules".
2. The rules of one conversation come after the machine policy. They can make any
   decision stricter. They can never loosen a decision about a secret or a system
   path.
3. A turn from the phone asks for everything outside `$SCRATCH`, also when a rule
   allows it.

Some floors hold in every mode, whatever the rules say:

- a call that can wait for input at the terminal, such as `sudo`, asks; once you
  approve it, it may run past the model's timeout while you follow the turn in a
  terminal, up to `interactive_timeout_minutes` under `[shell]` (60 by default);
- a program that runs commands as another user (`sudo`, `doas`, `su`, `pkexec`,
  `run0`) asks;
- a turn from the phone runs with at most `cautious` and asks outside `$SCRATCH`;
- secrets are denied unless a rule of yours names them, and the daemon's own
  `secrets/` is denied even then;
- the rules of a conversation only make a decision stricter;
- no tool writes efr's config; see "Config protection";
- a change of efr's settings through the settings tool asks, and a turn from the
  phone cannot make one; see "The settings tool".

## Path classes

Every path has one class. The target path decides the class, never the directory the
shell is in.

| Class | Examples | Read | Write |
|---|---|---|---|
| Scratch | the conversation's `$SCRATCH` | free | free |
| User config | `~/.config`, `~/.zshrc`, other dot entries in `~`, a repository's `.git` in `~` or `$SCRATCH` | free | approval |
| User data | `~/Documents`, `~/p`, `~/.local/share`, `~/.cache` | free | approval, free below the root of the turn's registered project |
| System | everything outside `~` | free | approval |
| Secrets | `~/.ssh`, `~/.gnupg`, password stores, credential files such as `~/.aws/credentials`, `/etc/shadow`, a process's `environ`, `root`, `cwd`, `fd`, `map_files` and `mem` under `/proc`, the daemon's `secrets/`, and `permissions.secret_paths` | denied | denied |

The columns show the `cautious` mode; `manual` asks where this table says free.
`crates/efr-permissions/src/path_class.rs` lists every secret location. No rule opens
the daemon's own `secrets/`, where efr keeps your login, not even one of yours. A
repository's `.git` is user config, also inside the turn's project or `$SCRATCH`:
`git status`, `git diff` and `git log` run without approval, and they run the programs
that the repository's config names (`core.fsmonitor`, filter drivers,
`diff.external`), so a write to `.git/config` or `.git/hooks` asks.

A write to the root of the project itself, or to a directory above it, asks: it could
replace or remove the whole project. A write of a directory asks when a secret, or
efr's config, lies below it, even when a rule allows the write.

## The built-in rules

The `cautious` mode, the default, decides by these rules:

| # | Action | Resource | Effect |
|---|---|---|---|
| 0 | any | any | ask |
| 1 | read | any | allow |
| 2 | write | class user data | ask |
| 3 | write | project | allow |
| 4 | write | class user config | ask |
| 5 | write | class system | ask |
| 6 | write | class scratch | allow |
| 7 | any | class secrets | deny |
| 8 and on | execute | a read-only command below | allow |

The `manual` mode keeps only rule 0 and rule 7, as its rules 0 and 1. The `auto` mode
keeps all of them and adds the `auto` table after them.

Rule 0 makes every command line and all network access ask. The rules from 8 on allow
read-only commands. A command matches a row when it starts with the program and the
words in `args`, holds none of the words in `forbid`, has at least `min` and at most
`max` operands and at most `options` options after `args`, and passes the `check`:

| # | Program | Args | Forbid | Min | Max | Options | Check |
|---|---|---|---|---|---|---|---|
| 8 | `ls` |  |  |  |  |  |  |
| 9 | `pwd` |  |  |  |  |  |  |
| 10 | `cat` |  |  |  |  |  |  |
| 11 | `head` |  |  |  |  |  |  |
| 12 | `tail` |  | `-f` `-F` `--follow` |  |  |  |  |
| 13 | `wc` |  |  |  |  |  |  |
| 14 | `file` |  | `-C` `--compile` |  |  |  |  |
| 15 | `stat` |  |  |  |  |  |  |
| 16 | `du` |  |  |  |  |  |  |
| 17 | `df` |  |  |  |  |  |  |
| 18 | `lsblk` |  |  |  |  |  |  |
| 19 | `blkid` |  | `-g` `--garbage-collect` `-c` `--cache-file` |  |  |  |  |
| 20 | `findmnt` |  | `-p` `--poll` |  |  |  |  |
| 21 | `free` |  | `-s` `--seconds` |  |  |  |  |
| 22 | `uptime` |  |  |  |  |  |  |
| 23 | `uname` |  |  |  |  |  |  |
| 24 | `whoami` |  |  |  |  |  |  |
| 25 | `id` |  |  |  |  |  |  |
| 26 | `groups` |  |  |  |  |  |  |
| 27 | `hostname` |  | `-F` `--file` `-b` `--boot` |  | 0 |  |  |
| 28 | `date` |  | `-s` `--set` |  |  |  |  |
| 29 | `which` |  |  |  |  |  |  |
| 30 | `type` |  |  |  |  |  |  |
| 31 | `command` | `-v\|-V` |  |  |  |  |  |
| 32 | `echo` |  |  |  |  |  |  |
| 33 | `printf` |  | `-v` |  |  |  |  |
| 34 | `realpath` |  |  |  |  |  |  |
| 35 | `readlink` |  |  |  |  |  |  |
| 36 | `basename` |  |  |  |  |  |  |
| 37 | `dirname` |  |  |  |  |  |  |
| 38 | `tree` |  | `-o` `-R` |  |  |  |  |
| 39 | `rg` |  | `--pre` `--hostname-bin` |  |  |  |  |
| 40 | `grep` |  |  |  |  |  |  |
| 41 | `egrep` |  |  |  |  |  |  |
| 42 | `fgrep` |  |  |  |  |  |  |
| 43 | `diff` |  |  |  |  |  |  |
| 44 | `cmp` |  |  |  |  |  |  |
| 45 | `sort` |  | `-o` `--output` `--compress-program` `--files0-from` |  |  |  |  |
| 46 | `uniq` |  |  |  | 1 |  |  |
| 47 | `cut` |  |  |  |  |  |  |
| 48 | `tr` |  |  |  |  |  |  |
| 49 | `column` |  |  |  |  |  |  |
| 50 | `jq` |  | `-i` `--in-place` `-f` `--from-file` `-L` `--library-path` `env` `ENV` `include` `import` |  |  |  |  |
| 51 | `ps` |  | `e` `-e` `--format` |  |  |  |  |
| 52 | `ps` | `-e\|-ef\|-eF\|-ely\|-eLf\|-ejH` |  |  | 0 | 0 |  |
| 53 | `pgrep` |  |  |  |  |  |  |
| 54 | `ss` |  | `-K` `--kill` `-D` `--diag` |  |  |  |  |
| 55 | `ip` | `addr\|address\|a` |  |  | 0 |  |  |
| 56 | `ip` | `addr\|address\|a` `show\|list` |  |  |  |  |  |
| 57 | `ip` | `route\|r` |  |  | 0 |  |  |
| 58 | `ip` | `route\|r` `show\|list` |  |  |  |  |  |
| 59 | `ip` | `link\|l` |  |  | 0 |  |  |
| 60 | `ip` | `link\|l` `show\|list` |  |  |  |  |  |
| 61 | `journalctl` |  | `--vacuum*` `--rotate` `--flush` `--sync` `--relinquish-var` `--smart-relinquish-var` `--setup-keys` `--update-catalog` `--cursor-file` `-f` `--follow` |  |  |  |  |
| 62 | `systemctl` | `status\|list-units\|list-unit-files\|is-active\|is-enabled\|is-failed\|cat` | `-H` `--host` |  |  |  |  |
| 63 | `systemctl` | `--user` `status\|list-units\|list-unit-files\|is-active\|is-enabled\|is-failed\|cat` | `-H` `--host` |  |  |  |  |
| 64 | `systemctl` | `show` | `-H` `--host` `-p` `-P` `--property` | 1 |  |  |  |
| 65 | `systemctl` | `--user` `show` | `-H` `--host` `-p` `-P` `--property` | 1 |  |  |  |
| 66 | `git` | `status\|diff\|log\|show\|rev-parse\|ls-files\|blame` | `--output` |  |  |  |  |
| 67 | `git` | `--no-pager` `status\|diff\|log\|show\|rev-parse\|ls-files\|blame` | `--output` |  |  |  |  |
| 68 | `git` | `branch` | `-d` `-D` `--delete` `-m` `-M` `--move` `-c` `-C` `--copy` `-f` `--force` `-u` `--set-upstream-to` `--unset-upstream` `--edit-description` `-t` `--track` `--no-track` `--create-reflog` |  | 0 |  |  |
| 69 | `git` | `--no-pager` `branch` | `-d` `-D` `--delete` `-m` `-M` `--move` `-c` `-C` `--copy` `-f` `--force` `-u` `--set-upstream-to` `--unset-upstream` `--edit-description` `-t` `--track` `--no-track` `--create-reflog` |  | 0 |  |  |
| 70 | `git` | `remote` |  |  | 0 |  |  |
| 71 | `git` | `--no-pager` `remote` |  |  | 0 |  |  |
| 72 | `pacman` | `-Q*\|--query` |  |  |  |  |  |
| 73 | `pacman` | `-Ss\|-Ssq\|-Sqs\|-Si\|-Sii` | `-y` `-u` `-w` `-c` `--refresh` `--sysupgrade` `--downloadonly` `--clean` |  |  |  |  |
| 74 | `lspci` |  |  |  |  |  |  |
| 75 | `lsusb` |  |  |  |  |  |  |
| 76 | `sensors` |  | `-s` `--set` |  |  |  |  |
| 77 | `nproc` |  |  |  |  |  |  |
| 78 | `find` |  | `-exec` `-execdir` `-ok` `-okdir` `-delete` `-fprint*` `-fls` |  |  |  |  |
| 79 | `cd` |  |  |  |  |  |  |
| 80 | `pushd` |  |  |  |  |  |  |
| 81 | `popd` |  |  |  |  |  |  |
| 82 | `sed` |  |  |  |  |  | `sed_print_only` |

The `forbid` words and `max` stop what would change the machine or never end: an
option that writes a file (`sort -o`, `tree -o`, `git diff --output`), runs another
program (`rg --pre`, `find -exec`, `sort --compress-program`), deletes
(`find -delete`, `journalctl --vacuum-size`), changes a setting (`date -s`,
`hostname NAME`), reads the files that another file names (`sort --files0-from`), or
waits forever and keeps the hidden shell busy (`tail -f`, `journalctl -f`). `env` and
`printenv` are not in the table, because they print every variable, tokens included.
For the same reason no argument of `ps` but a long option may hold an `e`: in BSD
syntax `e` shows each process's environment, and `ps` reads the whole line as BSD
syntax when one word is not valid UNIX syntax, so `ps -ex` and `ps -e -x` show it too.
The `environ` output field shows it as well, so `--format` may not appear either.
The UNIX forms `ps -e`, `ps -ef`, `ps -eF`, `ps -ely`, `ps -eLf` and `ps -ejH` run
alone, with nothing after them. A `jq` program may not use `env` or `$ENV`, nor come
from a file (`-f`) or a module (`include`, `import`, `-L`) that the line does not
show, and `systemctl show` must name a unit (without one it shows the service
manager's environment).

`cd`, `pushd` and `popd` change only the hidden shell's directory. The shell tool
declares the target of `cd` and `pushd` as a read, so `cd ~/.ssh` is denied like
`ls ~/.ssh`, and the engine follows the change for the rest of the line (see "Your
rules" for what that means for `under`).

`sed` runs only in its print-only form, which the check `sed_print_only` proves:

- `-n`, `--quiet` or `--silent` is given;
- the only other options are `-e` and `--expression` with a script, `-E`, `-r`,
  `--regexp-extended`, `-u`, `--unbuffered`, `--posix` and `--sandbox`, written in
  full: no `-i` or `--in-place`, no `-f` or `--file`, no `-s`, `-z`, `-l` or
  `--debug`;
- each script holds only addresses (line numbers, `$`, `first~step`, `/regex/` with
  the flags `I` and `M`, ranges of them, `addr,+N` and `addr,~N`), `!`, and the
  commands `p`, `l`, `=`, `q` and `Q`, separated by `;` or newlines.

So `sed -n '1,20p' f` runs, and `sed -n 'w out' f`, `sed -n '1e date' f`,
`sed -n 's/a/b/p' f` and `sed -i ...` ask: `w` and `W` write a file, `r` and `R`
read one, `e` runs a program, and the `s` command has flags that do both. A regex
with a bracket expression asks too, because GNU and BSD sed end such a regex at
different places. What the check cannot prove matches no row.

`crates/efr-permissions/src/policy/defaults.rs` holds the table as data. A test keeps
this file, the table in the crate and the data equal.

## The auto table

The `auto` mode adds these rows to the rules of `cautious`. Each row becomes one
`execute` rule for each place where it may run, and the rows marked "Network" one
`network` rule more for each place. The `#` column numbers those rules, as a reason
names them, such as `by rule 93 of the machine policy`.

- **Writer programs**, anywhere: `rm`, `rmdir`, `mkdir`, `touch`, `mv`, `cp`, `ln`,
  `chmod`, `truncate` and `tee`. The shell tool declares their operands as writes in
  every mode (see "Which paths a command reads"), so the path rules decide: in the
  turn's project or `$SCRATCH` the call runs; a write to the project's root itself,
  above it or anywhere else asks; a secret or efr's config is denied. `rm -rf ..`
  in the project root asks, `mv src ~/x` asks, and `rm ~/.ssh/known_hosts` is denied
  in every mode.
- **Build, test, format and lint tools**, only while the hidden shell is in the
  turn's project or in `$SCRATCH` (`under = "project"` and `under = "scratch"`):
  `cargo`, `just`, `make`, `npm`, `pnpm`, `yarn`, `bun`, `go`, `pytest`, `uv`, `ruff`,
  `mypy`, `rustfmt`, `prettier`, `eslint`, `tsc` and `zig build`. Options that point
  them at another directory, another manifest, another registry or another shell are
  forbidden, and so is an operand that installs a package the project does not
  declare (`npm install left-pad`, `go run x@latest`). `cargo test` outside the
  project asks.
- **Local git**, in the project or `$SCRATCH`: `add`, `commit`, `switch`, `checkout`
  of a branch, `restore --staged`, `stash` with `push`, `list`, `show`, `apply` or
  `pop`, `merge`, `rebase` (not `-i`, not `--exec`), `cherry-pick`, `tag` (to create
  one), `mv`, `rm`, `worktree add` and `worktree list`, and `fetch` and `pull` from a
  remote the repository names. The check `ref_names` lets `checkout`, `fetch` and
  `pull` take only words that git reads as a ref or a remote: `git checkout -- .`,
  `git checkout .` and `git fetch https://host/x` ask. `git checkout` takes one
  branch, or `-b` with a new branch and the commit it starts at, so
  `git checkout main src/x.rs` asks. The text cannot tell a branch from a file of the
  same name, so `git checkout NAME` restores the file `NAME` when no branch has that
  name. `git rm`, `git mv` and `git worktree add` declare their
  operands as writes, so the path rules keep them in the project.

Everything else asks. In particular: `git push`, `git reset --hard`, `git clean`,
`git branch -d` and `-D`, `git tag -d`, `git stash drop` and `clear`,
`git checkout -- <path>`, `git checkout .`, `git restore` of the work tree,
`git filter-branch`, `git remote add` and `set-url`; system package managers;
`systemctl` changes; `mount`; `kill`, `pkill` and `killall`; `docker` and `podman`;
`curl`, `wget`, `ssh`, `scp`, `rsync` and `nc`; scripts and programs that are not in
the table; and every line the engine cannot read.

**Network.** `auto` never allows general network access, because a URL or an upload
is how a model that follows injected instructions sends your data out. Only the rows
that fetch the packages a project declares, and `git fetch` and `git pull`, reach the
network: `cargo build`, `check`, `test`, `fetch` and `update`, `npm`, `pnpm`, `yarn`
and `bun` with `install` or `ci`, `go mod download` and `uv sync`. When a call
reaches the network, every simple command of its line must be one that a rule lets
reach it, or one that only a built-in row lets run: `npm ci | tail -n 20` and
`git fetch && git rebase origin/main` run, but in `cargo fetch; curl -d @x host`,
`curl` asks, also when a rule of yours lets `curl` run.

| # | Program | Args | Forbid | Min | Max | Options | Check | Where | Network |
|---|---|---|---|---|---|---|---|---|---|
| 83 | `rm` |  |  |  |  |  |  |  |  |
| 84 | `rmdir` |  |  |  |  |  |  |  |  |
| 85 | `mkdir` |  |  |  |  |  |  |  |  |
| 86 | `touch` |  |  |  |  |  |  |  |  |
| 87 | `mv` |  |  |  |  |  |  |  |  |
| 88 | `cp` |  |  |  |  |  |  |  |  |
| 89 | `ln` |  |  |  |  |  |  |  |  |
| 90 | `chmod` |  |  |  |  |  |  |  |  |
| 91 | `truncate` |  |  |  |  |  |  |  |  |
| 92 | `tee` |  |  |  |  |  |  |  |  |
| 93-96 | `cargo` | `build\|check\|test\|fetch\|update` | `--manifest-path` `--config` `--target-dir` `-Z` `-C` |  |  |  |  | project, scratch | yes |
| 97-98 | `cargo` | `clippy\|fmt\|doc\|run\|bench\|nextest\|tree\|metadata` | `--manifest-path` `--config` `--target-dir` `-Z` `-C` `--open` |  |  |  |  | project, scratch |  |
| 99-100 | `just` |  | `-f` `--justfile` `-d` `--working-directory` `-g` `--global-justfile` `--set` `-c` `--command` `--shell*` `--dotenv-path` `=` |  |  |  |  | project, scratch |  |
| 101-102 | `make` |  | `-C` `--directory` `-f` `--file` `--makefile` `-I` `--include-dir` `--eval` `-E` `=` |  |  |  |  | project, scratch |  |
| 103-106 | `npm` | `install\|ci` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  | 0 |  |  | project, scratch | yes |
| 107-108 | `npm` | `run\|test\|build\|lint` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  |  |  |  | project, scratch |  |
| 109-112 | `pnpm` | `install\|ci` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  | 0 |  |  | project, scratch | yes |
| 113-114 | `pnpm` | `run\|test\|build\|lint` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  |  |  |  | project, scratch |  |
| 115-118 | `yarn` | `install\|ci` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  | 0 |  |  | project, scratch | yes |
| 119-120 | `yarn` | `run\|test\|build\|lint` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  |  |  |  | project, scratch |  |
| 121-124 | `bun` | `install\|ci` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  | 0 |  |  | project, scratch | yes |
| 125-126 | `bun` | `run\|test\|build\|lint` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  |  |  |  | project, scratch |  |
| 127-128 | `go` | `build\|test\|vet\|fmt\|run` | `-C*` `-modfile*` `-overlay*` `-toolexec*` `-exec*` `@` |  |  |  |  | project, scratch |  |
| 129-130 | `go` | `mod` `tidy` | `-C*` `-modfile*` `-overlay*` `-toolexec*` `-exec*` `@` |  |  |  |  | project, scratch |  |
| 131-134 | `go` | `mod` `download` | `-C*` `-modfile*` `-overlay*` `-toolexec*` `-exec*` `@` |  | 0 |  |  | project, scratch | yes |
| 135-136 | `pytest` |  | `--rootdir` `-c` `--config-file` `-p` |  |  |  |  | project, scratch |  |
| 137-138 | `uv` | `run` | `--directory` `--project` `--with*` `--index*` `--extra-index-url` `--default-index` `--find-links` |  |  |  |  | project, scratch |  |
| 139-142 | `uv` | `sync` | `--directory` `--project` `--with*` `--index*` `--extra-index-url` `--default-index` `--find-links` |  | 0 |  |  | project, scratch | yes |
| 143-144 | `ruff` |  |  |  |  |  |  | project, scratch |  |
| 145-146 | `mypy` |  |  |  |  |  |  | project, scratch |  |
| 147-148 | `rustfmt` |  |  |  |  |  |  | project, scratch |  |
| 149-150 | `prettier` |  |  |  |  |  |  | project, scratch |  |
| 151-152 | `eslint` |  |  |  |  |  |  | project, scratch |  |
| 153-154 | `tsc` |  |  |  |  |  |  | project, scratch |  |
| 155-156 | `zig` | `build` | `-p` `--prefix*` `--build-file` |  |  |  |  | project, scratch |  |
| 157-158 | `git` | `add` |  |  |  |  |  | project, scratch |  |
| 159-160 | `git` | `commit` |  |  |  |  |  | project, scratch |  |
| 161-162 | `git` | `switch` | `--discard-changes` `-f` `--force` |  |  |  |  | project, scratch |  |
| 163-164 | `git` | `checkout` | `-p` `--patch` `-f` `--force` `--ours` `--theirs` `-m` `--merge` `--conflict` `--overlay` `--no-overlay` `--pathspec-from-file` |  | 1 |  | `ref_names` | project, scratch |  |
| 165-166 | `git` | `checkout` `-b\|-B` | `-p` `--patch` `-f` `--force` `--ours` `--theirs` `-m` `--merge` `--conflict` `--overlay` `--no-overlay` `--pathspec-from-file` |  | 2 |  | `ref_names` | project, scratch |  |
| 167-168 | `git` | `restore` `--staged\|-S` | `-W` `--worktree` `-p` `--patch` |  |  |  |  | project, scratch |  |
| 169-170 | `git` | `stash` |  |  | 0 | 0 |  | project, scratch |  |
| 171-172 | `git` | `stash` `push\|list\|show\|apply\|pop` |  |  |  |  |  | project, scratch |  |
| 173-174 | `git` | `merge` | `-s` `--strategy` |  |  |  |  | project, scratch |  |
| 175-176 | `git` | `rebase` | `-i` `--interactive` `-x` `--exec` `--edit-todo` `-s` `--strategy` |  |  |  |  | project, scratch |  |
| 177-178 | `git` | `cherry-pick` | `-s` `--strategy` |  |  |  |  | project, scratch |  |
| 179-180 | `git` | `tag` | `-d` `--delete` `-f` `--force` `-s` `--sign` `-u` `--local-user` `-v` `--verify` |  |  |  |  | project, scratch |  |
| 181-182 | `git` | `mv` |  |  |  |  |  | project, scratch |  |
| 183-184 | `git` | `rm` |  |  |  |  |  | project, scratch |  |
| 185-186 | `git` | `worktree` `add\|list` |  |  |  |  |  | project, scratch |  |
| 187-190 | `git` | `fetch` | `--upload-pack` `--exec` `-o` `--server-option` `-s` `--strategy` |  |  |  | `ref_names` | project, scratch | yes |
| 191-194 | `git` | `pull` | `--upload-pack` `--exec` `-o` `--server-option` `-s` `--strategy` |  |  |  | `ref_names` | project, scratch | yes |

`crates/efr-permissions/src/policy/auto.rs` holds the table as data, with the
reasons for each `forbid` list. A test keeps this file, `policy/auto.md` and the data
equal.

## How the engine reads a command line

A command rule judges one simple command at a time. The engine splits a line on `;`,
`&&`, `||`, `|`, `|&` and newlines. It reads single quotes, double quotes and
backslashes as zsh reads them, so `rg 'a|b' src` is one command. It allows a line only
when it allows every simple command in it: `git status && git push` asks for
`git push`. The reason names the part that asks, and the approval question names
every part that asks on a line of its own, such as
`asks for: hostnamectl, systemctl --failed`. It shows each part by its program and a
few words after it, without long words, quoted text or the value of `--option=value`,
so a token on the line does not show twice.

The engine cannot see through some constructs. A line that holds one of them matches
no command rule. Only a rule whose resource is `any` can decide it, and the built-in
rule 0 asks:

- a command substitution: `$(...)` or backquotes;
- a process substitution: `<(...)`, `>(...)` or `=(...)`;
- a parameter or arithmetic expansion: `$HOME`, `${x}`, `$((1+1))`, `$'...'`;
- a history expansion: `!` outside single quotes;
- a group, subshell, function or brace expansion: `(`, `)`, `{`, `}`;
- a here-document or here-string: `<<`, `<<<`;
- an output redirection to anything but `/dev/null`. `2>&1`, `>&2` and `>/dev/null`
  are fine; `< file` is fine, and the path rules judge the file;
- a background job: `&` at the end of a command;
- a variable assignment before a command, except `LC_*`, `LANG`, `LANGUAGE`, `TZ`,
  `COLUMNS`, `LINES`, `NO_COLOR`, `TERM` and `CLICOLOR`. `PATH=... ls` and
  `LD_PRELOAD=... ls` ask. An assignment without a command asks, because it changes the
  shell for every later command;
- a pattern that could expand to an option: a word that starts with `*`, `?` or `[`,
  or a word that starts with `-` and holds a pattern. `cat *.rs` asks; `cat ./*.rs`
  is judged by its parts, and its glob reads everything below `.` (see below). A
  pattern elsewhere in a word matches only a row without `min`, `max`, `check` and
  `forbid` words without a dash: zsh replaces it with the names it matches, so
  `uniq in*` may name an output file, `systemctl show x*` may name no unit and
  `ps ax?` may become `ps axe`. These ask;
- a builtin that runs a string as commands or changes how later commands run: `eval`,
  `exec`, `source`, `.`, `alias`, `export`, `set`, `trap`, `builtin` and more;
- an unclosed quote, a backslash at the end of the line, a control character, a
  non-ASCII character outside quotes, or a `#`, `^`, `=` or `~` at the start of a
  word (`~` and `~/...` are fine, `~root` is not).

`sudo`, `sudoedit`, `doas`, `su`, `pkexec` and `run0` always ask. No command rule
matches them, also behind a wrapper such as `env` or `nice`. If a rule for every
command line allows them, the engine still asks.

What happens to the password after an approved `sudo` is `[shell] sudo_cache`:

- `keep` (the default): sudo keeps its own credential cache for the hidden shell's
  terminal, 5 minutes unless `timestamp_timeout` in sudoers says otherwise, so a
  second `sudo` in that time runs without the password. It still asks for approval.
- `per_call`: after each shell call, before anything else runs in that hidden shell,
  efr makes it forget the credentials (`sudo -k`, and `doas -L` when doas exists), so
  the next `sudo` asks for the password again. Nothing of this shows on the screen or
  in the output. It happens after every call, not only after a line that names sudo,
  because a script or a function can run sudo too. It needs efr's zsh integration in
  the hidden shell: a `shell.program` that is not a zsh, or a zsh whose integration
  did not load, keeps sudo's cache (the daemon logs a warning). A call that runs in a
  nested shell (`nested_shell`, such as a command inside `bash` or `ssh`) forgets
  only when that nested shell exits back to the hidden zsh.

With either value, every call that runs `sudo` asks for your approval first.

## Which paths a command reads

A read-only command still reads files, and the path rules judge what it reads. The
shell tool declares the paths that a line names, resolved against the hidden shell's
directory:

- every operand of every program is a read, and so is an option value that looks like
  a path, such as `--file=x` or `-f/x`, and the path in `HEAD:path`. `echo`, `printf`,
  `basename`, `dirname`, `which`, `type` and a few more declare nothing, because their
  arguments are not files. `$HOME` at the start of a word counts as `~`;
- the writer programs write their operands instead, in every mode: every operand of
  `rm`, `rmdir`, `mkdir`, `touch`, `mv`, `chmod`, `truncate` and `tee`; the last
  operand of `cp` (its other operands are read with everything below them, as a
  recursive copy reads them); the link that `ln` creates (its target is read). With
  `-t` or `--target-directory`, every operand of `cp` and `ln` counts as written, and
  so does every operand when an option follows an operand (GNU `cp` and `ln` read
  options anywhere, so in `cp a ~/.bashrc -S x` the `x` is the backup suffix and
  `~/.bashrc` is written) or when an option is not one of theirs. The value of `-S`
  or `--suffix` is never an operand. A
  hard link (`ln` without `-s`, `cp -l`) writes its sources too, because the new name
  writes the same file. The operands of `git rm`, `git mv` and `git worktree add` are
  written too;
- `ls` with no path lists the working directory, which is a read of it;
- `rg`, `grep -r`, `find`, `du`, `tree`, `ls -R` and `diff` read everything below their
  paths. With no path, they read the working directory. An option that the shell tool
  does not know makes it assume the worst: every operand is a path, and the working
  directory is read too;
- a glob reads everything below its fixed directory: `cat ~/.ss*/id*` reads all of `~`.
  A glob that a writer program writes writes its fixed directory, so `rm *.o` in the
  project's root asks and `rm target/*.o` runs in `auto`;
- `< file` is a read, and `> file` or `>> file` is a write;
- the commands inside `$(...)`, backquotes and groups declare their paths too;
- a `cd` in the line adds its target as one more directory where the rest of the line
  may run. After `cd -`, `popd` or `cd $DIR`, a relative path may be anywhere below `/`;
- a path that an earlier `cp`, `ln`, `mv`, `git mv` or `git worktree add` of the same
  line writes may be a symbolic link by the time a later command uses it, so a later
  path at or below it may be anywhere below `/` too: `ln -s ~ h && cat h/.ssh/x`
  asks.

So `cat ~/.ssh/id_ed25519` is denied, although `cat` runs freely, and `rm ~/.ssh/x`
is a denied write of a secret, not a read. A read of everything below a directory asks
when a secret lies below it, unless a rule allows that secret: `rg TOKEN ~/.aws` asks,
because `~/.aws/credentials` lies below `~/.aws`. Naming a secret denies the call
without asking you, unless one of your rules opens it.

What the text cannot show, the engine cannot judge. The daemon also declares what each
path reaches through a symbolic link, so `cat notes`, where `notes` links to
`~/.ssh/id_ed25519`, is denied like `cat ~/.ssh/id_ed25519`, and a recursive search
whose root is a link is judged by its target too. It does not see a link that a glob
expands to, a link below the root of a recursive search that the program follows
(`rg -L`, `grep -R`, `find -L`), or a link that a program other than the writer
programs creates in the same line, such as `git checkout` or a build. It does not see
the files that a program writes without naming them on the line, such as what a build,
a script or `git checkout` writes.

A rule trusts that a program name means what it says. The hidden shell sources your
startup files, and then removes every global alias (`alias -g`), every suffix alias
(`alias -s`), and every alias or function named like a program that a command rule
names (the tables of every mode and your rules), again before each command it runs. So
`ls` runs `ls` there even when your `.zshrc` aliases it to `eza`, `rm` runs `rm` even
when you alias it to `rm -i`, and your other aliases stay. A program that comes first
on your `PATH` under the same name still runs instead, and configuration that a
repository already has for `git`, or includes from its work tree, still changes what
`git` runs. A script or a build reads files that the line does not name.

## Config protection

No tool may write efr's config directory (`$XDG_CONFIG_HOME/efr`, with `config.toml`
and the project registry `projects.toml`), in any mode. When `config.toml`, or any
other entry of that directory, is a symbolic link, the real file behind it is
protected too, such as `~/dotfiles/efr/config.toml` in a dotfiles repository; so is
the real directory when the directory itself is a link. The config sets your
permission rules and the registry defines the project that `auto` trusts, so a model
that could write them could give itself any permission.

- `write_file`, an output redirection (`> ~/.config/efr/config.toml`), a writer program
  (`cp x ~/.config/efr/config.toml`, `ln -sf x ~/.config/efr/config.toml`,
  `rm ~/.config/efr/projects.toml`) and a path that reaches the config through a link
  are denied.
- No rule opens it for writing, not even one of yours: `{ under = "~/.config" }` with
  `write` and `allow` still denies `~/.config/efr/config.toml`. The rules of a
  conversation cannot open it either.
- A write of a directory above it, such as `rm -rf ~/.config`, asks, also when a rule
  allows it.
- Reading it is free in `cautious` and `auto`; it holds no secrets.

Config protection judges what a call declares. A build or a test that `auto` runs
runs the code of the directory, which the model may have written itself in the same
turn, and that code can write any file you can, efr's config included. The daemon
reloads a changed `config.toml` without a question, so such code can change the
permissions of the next turn. This is one more reason to choose `auto` only for work
where you accept that the model runs code of its own.

You change the file yourself, in your editor or with `efr config`, or you approve a
change of the settings tool. The daemon reads the links in the directory each time it
builds the engine.

## The settings tool

The model reads and changes efr's settings with its `settings` tool, never with
`write_file` or the shell, which config protection denies.

- `read` lists every setting with its source, the rules of `config.toml` with their
  numbers, the models with their efforts, and the last reload's error. It needs no
  approval for a turn from your terminal.
- `set` and `unset` change one key, `add_rule` appends a rule after your rules, and
  `remove_rule` removes one by its number. efr checks the whole new file first, as a
  load would: an unknown key, a value of the wrong kind, a model that is not in the
  model list or an effort the default model does not take goes back to the model, and
  you are not asked.
- Every valid change asks, in every mode, also in `auto`, and also when a rule of yours
  would allow writing the file. No rule can turn the question into an allow. The
  question shows a one-line summary and the unified diff of the file, which keeps your
  comments and layout; for a missing file, the diff from the example that the new file
  starts as.
- A change that loosens permissions is marked "loosens permissions": a new `allow`
  rule, a removed `deny` or `ask` rule, a mode toward `auto`, `shell.sudo_cache` from
  `per_call` to `keep`, or a removed secret path.
- The tool refuses every rule that names secrets (the class `secrets`, or an `under`
  path at or below a secret location), to add and to remove. You write such rules by
  hand.
- A turn from the phone can read the settings, after your approval as for every call
  of a remote turn, but a change is denied.
- When the file changes between the question and your answer, nothing is written; the
  model calls the tool again, which plans against the new file and asks again.
- After the write the daemon reloads at once. The change applies from the next turn
  (a rule from the next tool call); the tool's answer says so, and names a key that
  needs a restart.

## Your rules

Put your rules in `$XDG_CONFIG_HOME/efr/config.toml`. Each rule has an `action`
(`any`, `read`, `write`, `execute` or `network`), a `resource` and an `effect`
(`allow`, `ask` or `deny`). The resource is one of:

- `"any"`: every path, command line and network access;
- `{ class = "user_config" }`: every path of one class (`scratch`, `user_config`,
  `user_data`, `system`, `secrets`);
- `{ under = "~/.config/nvim" }`: a path and everything below it, absolute or under
  `~`;
- `"project"`: every path inside the root of the turn's registered project; for a
  write, every path below the root;
- `{ command = { ... } }`: a simple command, with these keys:
  - `program`: the first word, such as `cargo`. A path such as `/usr/bin/cargo` is a
    different program;
  - `args`: the words that must follow the program, in order. A word may list
    alternatives, as in `"status|diff"`, and may end in `*` to match every word that
    starts with it, as `"-Q*"` matches `-Qi`;
  - `forbid`: words that must not appear after the program. `--name` also matches
    `--name=value` and every abbreviation down to `--n`. `-x`, one letter, matches
    every cluster of short options that holds the letter, such as `-uo` for `-o`.
    `-name`, several letters after one dash, matches that word only. An option that
    ends in `*` matches every word that starts with it. A word without a dash matches
    every operand that contains it;
  - `max_operands` and `min_operands`: the most and the fewest operands after `args`.
    An operand is `-`, a word that does not start with `-`, or any word after `--`;
  - `max_options`: the most options after `args`. An option is a word that starts
    with `-`, other than `-`, up to and including `--`. `max_options = 0` with
    `max_operands = 0` allows the words of `args` and nothing after them;
  - `under`: where the command must run: a directory or below it, `"project"` for the
    root of the turn's registered project or below it, or `"scratch"` for the
    conversation's `$SCRATCH` or below it. It is the hidden shell's directory when the
    line starts. After a `cd`, `pushd` or `popd` earlier in the line, the directory is
    unknown and the rule matches nothing;
  - `check`: a check of the words after `args` that words alone cannot express:
    `"sed_print_only"` (for `sed` only) or `"ref_names"`, as the tables above use them.

With the action `network`, a command rule lets that command reach the network, as the
fetching rows of the `auto` table do. A command rule with the action `any` lets the
command run but never opens the network.

Your rules come after the built-in rules, so the last one of yours that matches wins.
Your rules are the only rules that can open a secret, and only a rule that names it
opens it: `{ class = "secrets" }`, or `under` a path at or below a secret location,
such as `{ under = "~/.ssh/config" }`. A rule for a wider resource, such as `"any"`,
`"project"` or `{ under = "~" }`, never opens a secret: `read` on `"any"` with `allow`
still denies `~/.ssh/id_ed25519`. After a rule that opened a secret, a wider rule can
only make the effect stricter, so `"any"` with `ask` asks for it again.

A rule that does not have the shape of a rule, or that names a relative path, a
program that is not one word, a check made for another program, or an action that its
resource never matches, stops the daemon at start. The error names the rule as
`permissions.rules[N]`, counted from 0. `efrd --print-config` shows your rules.

### Example: allow `cargo test` in one project

```toml
[[permissions.rules]]
action = "execute"
resource = { command = { program = "cargo", args = ["test"], under = "~/p/app" } }
effect = "allow"
```

`cargo test` and `cargo test --workspace` run without approval when the hidden shell is
in `~/p/app` or below it, in every mode. In `~/p/other`, and after a `cd` in the same
line, they ask. `cargo test` builds and runs the project's own code, so allow it only
in projects whose code you trust. `under = "project"` allows it in whichever project a
turn runs in, as the `auto` mode does.

### Example: allow `systemctl restart nginx`

```toml
[[permissions.rules]]
action = "execute"
resource = { command = { program = "systemctl", args = ["restart", "nginx"], max_operands = 0 } }
effect = "allow"
```

`systemctl restart nginx` runs without approval. `max_operands = 0` stops
`systemctl restart nginx sshd` from restarting a second unit; it asks.
`sudo systemctl restart nginx` still asks, because `sudo` always asks. When systemd
needs your password through polkit, the call waits for it on the hidden shell's
screen.
