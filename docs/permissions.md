# Permissions

Before a tool call runs, the daemon decides one of four effects: `allow` (the call
runs), `contain` (the call runs at once in the `auto` sandbox), `ask` (the turn waits
until you approve or deny the call) or `deny` (the model gets an error that names the
reason). `efr-permissions` holds the rules and the
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
| `auto` | Every shell command, in a kernel sandbox: it writes only in the turn's project, the registered projects that it names, `$SCRATCH`, a private `/tmp` and private tool caches, and it has no network. An action that leaves the sandbox (an exit) asks. The file tools follow `cautious`. See "The auto sandbox". |

`permissions.mode` in `config.toml` sets the mode of a turn when the prompt names none.
A turn keeps its mode until it ends. In `auto`, the kernel holds what a command does,
not the engine's reading of the line: build scripts, tests, git hooks and code that the
model wrote in the same turn run in the same sandbox, so they cannot write your
config, reach the network or use `sudo` either. When the sandbox cannot run on this
machine, or the turn's project is your home directory, `auto` runs as `cautious` and
says why; see "The auto sandbox".

A registered project is a directory that you list in `projects.toml` in efr's config
directory. A turn whose hidden shell is in a project's root or below it runs in that
project: `cautious` writes freely below the root, and in `auto` the sandbox can write
the project, so its build, test and git commands run there. `efr project add` registers the git work tree
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
effect. The strictest effect decides the call, in the order
`allow < contain < ask < deny`.

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
has its own table; see "The auto sandbox".

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

## The auto sandbox

In `auto`, efr runs each shell command of the model in a kernel sandbox: bubblewrap,
Landlock and seccomp. The sandbox holds the command and every program that it starts.
[`docs/sandbox.md`](sandbox.md) tells what the sandbox allows, the exits, the
fallback and the limits. This section tells how the engine decides in `auto`.

- A command line gets the effect `contain`, between `allow` and `ask`: it runs at
  once, in the sandbox, with no question. The engine reads the line only to find
  exits; text analysis never lifts the sandbox.
- No rule lifts the sandbox. A rule of yours that allows a command still contains it,
  and a rule cannot have the effect `contain`. A rule of yours that asks or denies
  keeps its effect, and the reason names your rule.
- Before the run, efr finds the actions of the line that leave the sandbox: a write
  outside the write roots, the network, a socket, the bus, a device, a read of a
  sandbox mask, a destructive git or file command, `sudo` and the other programs that
  give more rights, `git push` and other uploads, rc files, services and cron. Each one
  is an exit with the effect `ask`, and the reason names its kind, such as `exit:
  write`. After a "yes", the call runs in the sandbox with exactly that path, socket,
  device or the network opened for this one call, or, for privilege, persistence,
  upload and some writes, outside the sandbox. The model can also ask for an exit
  with the shell tool's `needs`.
- A line that the engine cannot read gets only the exits that it can find. It runs in
  the sandbox, which holds it.
- The floors hold, as in every mode: a privileged program asks, and only you can
  approve it; a secret is denied unless a rule of yours names it; efr's config is
  denied; a change of the settings asks.
- `nested_shell` is denied in `auto`.
- `read_file`, `write_file` and `edit` run in efrd, outside the sandbox. They follow
  the rules of `cautious`, plus two: a read of a sandbox mask, such as a project `.env`,
  asks, and a write to a floor path, such as `~/.zshrc` or `.git/hooks`, asks.

The `auto` mode has its own table. It is not the `cautious` table: the private `/tmp`
and the cache overlays are write roots of the sandbox, so a write there must not ask.

| # | Action | Resource | Effect |
|---|---|---|---|
| 0 | any | any | ask |
| 1 | read | any | allow |
| 2 | write | project (the turn's project) | allow |
| 3 | write | class scratch | allow |
| 4 | write | a root of the sandbox: a named project, `/tmp`, `/var/tmp`, `/dev/shm`, a cache overlay, `sandbox.write_roots`; shell calls only | allow |
| 5 | any | class secrets | deny |
| 6 | execute | any | contain |

For a shell call, a path or a network need that rule 0 makes `ask` becomes `contain`:
the sandbox holds it, and an exit asks when the action leaves the sandbox. For
`read_file`, `write_file` and `edit`, rule 4 does not match, because efrd would write
the host's real `/tmp` and caches, so rule 0 asks.

**Exits.** An action that leaves the sandbox is an exit. The engine reads the line,
the paths that the tool declares and the facts that the daemon collects, and gives
each exit its own reason. An exit asks; `secret` and `config` are denied. Each exit
has a kind:

| Kind | Examples |
|---|---|
| `write` | a write outside the write roots: `echo x > ~/notes.txt`, `git worktree add ../wt`. A write to a node that the sandbox has, such as `2>/dev/null` or `>/dev/stderr`, is not an exit. A path below such a node, such as `/dev/fd/3/x`, goes through an open file to a place that efr cannot see, so it is a write that only you approve |
| `host` | a network need: `curl`, `git fetch`, `npm ci`, `checkupdates` |
| `host_view` | `ss`, `ip`, `nmcli`, `netstat` |
| `socket`, `bus` | a Unix socket; `hostnamectl`, `systemctl --failed`, `loginctl list-sessions` |
| `desktop_ipc` | `xrandr`, `swaymsg`, `hyprctl`, `xdotool` (only you approve) |
| `device` | a node below `/dev` that the sandbox does not have: `nvme smart-log /dev/nvme0n1`, `cat x > /dev/sda` |
| `masked_read` | a read of a masked path: `.env`, `~/.zsh_history`, a browser profile (only you approve) |
| `destructive` | `git reset --hard`, `git clean -f`, `git checkout -- P`, `dd of=`, `shred`, `: > F`, `rm -r` of tracked files |
| `privilege` | `sudo`, `doas`, `pkexec`, `run0`, `yay`, `paru`, `docker`, `podman`, a changing `systemctl`, `busctl call` (only you approve) |
| `persistence` | `~/.zshrc`, `crontab`, `systemctl --user enable`, `loginctl enable-linger`, autostart, `.envrc` or `.git/hooks` in a project (only you approve) |
| `upload` | `git push`, `npm publish`, `scp`, `rsync host:`, `curl -d`, `-F`, `-T` or `-X POST` (only you approve) |
| `synced_write` | a write to `~/Dropbox` and the other `sandbox.synced_dirs` (only you approve) |
| `above_root` | a write at or above a write root or efr's config: `rm -rf ..`, `rm -rf ~/.config` (only you approve) |
| `outside` | the model asks to run outside the sandbox |
| `secret`, `config` | a read or write of a secret, a write of efr's config: denied |

The engine reads every line, also one with a substitution or a group, and finds the
exits that it can see. Text can only add a question; the sandbox holds what the text
does not show. When a fact is missing, the engine assumes the stricter case: a target
exists, and a directory holds tracked files.

After a command fails in the sandbox, the model can call `shell` again with `needs`:
paths to write, hosts, sockets, a bus, a device, masked paths to read, or `outside`.
Each entry is an exit that asks. Other modes ignore `needs`.

An exit of the kinds `privilege`, `persistence`, `upload`, `outside`, `synced_write`,
`above_root`, or a write that no bind can serve, runs outside the sandbox. Such a line
must be one command, plus read-only helpers from the table above, such as `echo` in
`echo x | sudo tee /etc/x.conf`, and `sudo -k`, `sudo -K` or `sudo --reset-timestamp`
alone, which only forget the cached password: `sudo -k; sudo true` asks once.
`sudo -v && ./helper` gets an error with no question.

A turn from the phone runs with at most `cautious`, so it is never contained.
`crates/efr-permissions/src/exits.rs` holds the exit rules, and
`crates/efr-permissions/tests/fixtures/auto-corpus.toml` holds the result for each
line of the command corpus.

`auto` needs a working sandbox. When the probe fails, when `sandbox.enabled` is
`false`, or when the turn's project is your home directory, the turn runs as
`cautious`. The turn records why, `efr history` shows it, and the model is told the
mode that it really has.

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
permission rules and the registry defines the write roots of `auto`, so a model that
could write them could give itself any permission.

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

Config protection judges what a call declares. In `auto`, a write of efr's config is
a `config` exit, which is denied. The sandbox also holds the code that a call runs:
efr's config directory and the files behind its links are read-only in the sandbox, so
a build, a test or a script that the model wrote cannot write them either. A command
that runs outside the sandbox, in any mode, runs with your rights and can write any
file you can; that is why such a command asks.

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

With the action `network`, a command rule lets that command reach the network. A
command rule with the action `any` lets the command run but never opens the network.
In `auto`, no rule lifts the sandbox: an allowed command still runs in it, and its
network need is a `host` exit that asks.

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
turn runs in. In `auto`, the rule changes nothing: `cargo test` runs in the sandbox
anyway.

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
