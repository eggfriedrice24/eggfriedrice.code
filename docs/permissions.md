# Permissions

Before a tool call runs, the daemon decides one of three effects: `allow` (the call
runs), `ask` (the turn waits until you approve or deny the call) or `deny` (the model
gets an error that names the reason). `efr-permissions` holds the rules and the
engine. `efr-conversation/src/turn.rs` is the only place that applies them. This file
describes the rules, how the engine reads a command line, and how you add your own
rules in `config.toml`.

## How the engine decides

A tool call declares what it needs: the paths it reads or writes, the command line it
runs, network access, and input at the terminal. The engine gives each requirement an
effect. The strictest effect decides the call, in the order `allow < ask < deny`.

1. The machine policy sets the effect of each requirement. It is the built-in rules
   below, then your `[[permissions.rules]]`. The last rule that matches wins, so your
   rules win where they match. A requirement that no rule matches is denied.
2. The rules of one conversation come after the machine policy. They can make any
   decision stricter. They can never loosen a decision about a secret or a system
   path.
3. A turn from the phone asks for everything outside `$SCRATCH`, also when a rule
   allows it. A call that can wait for input at the terminal, such as `sudo`, always
   asks.

## Path classes

Every path has one class. The target path decides the class, never the directory the
shell is in.

| Class | Examples | Read | Write |
|---|---|---|---|
| Scratch | the conversation's `$SCRATCH` | free | free |
| User config | `~/.config`, `~/.zshrc`, other dot entries in `~` | free | approval |
| User data | `~/Documents`, `~/p`, `~/.local/share`, `~/.cache` | free | approval, free inside the turn's registered project |
| System | everything outside `~` | free | approval |
| Secrets | `~/.ssh`, `~/.gnupg`, password stores, credential files such as `~/.aws/credentials`, `/etc/shadow`, a process's `environ`, `root`, `cwd`, `fd`, `map_files` and `mem` under `/proc`, the daemon's `secrets/`, and `permissions.secret_paths` | denied | denied |

`crates/efr-permissions/src/path_class.rs` lists every secret location.

## The built-in rules

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

Rule 0 makes every command line and all network access ask. The rules from 8 on allow
read-only commands. A command matches a row when it starts with the program and the
words in `args`, holds none of the words in `forbid`, and has at least `min` and at
most `max` operands and at most `options` options after `args`:

| # | Program | Args | Forbid | Min | Max | Options |
|---|---|---|---|---|---|---|
| 8 | `ls` |  |  |  |  |  |
| 9 | `pwd` |  |  |  |  |  |
| 10 | `cat` |  |  |  |  |  |
| 11 | `head` |  |  |  |  |  |
| 12 | `tail` |  | `-f` `-F` `--follow` |  |  |  |
| 13 | `wc` |  |  |  |  |  |
| 14 | `file` |  | `-C` `--compile` |  |  |  |
| 15 | `stat` |  |  |  |  |  |
| 16 | `du` |  |  |  |  |  |
| 17 | `df` |  |  |  |  |  |
| 18 | `lsblk` |  |  |  |  |  |
| 19 | `blkid` |  | `-g` `--garbage-collect` `-c` `--cache-file` |  |  |  |
| 20 | `findmnt` |  | `-p` `--poll` |  |  |  |
| 21 | `free` |  | `-s` `--seconds` |  |  |  |
| 22 | `uptime` |  |  |  |  |  |
| 23 | `uname` |  |  |  |  |  |
| 24 | `whoami` |  |  |  |  |  |
| 25 | `id` |  |  |  |  |  |
| 26 | `groups` |  |  |  |  |  |
| 27 | `hostname` |  | `-F` `--file` `-b` `--boot` |  | 0 |  |
| 28 | `date` |  | `-s` `--set` |  |  |  |
| 29 | `which` |  |  |  |  |  |
| 30 | `type` |  |  |  |  |  |
| 31 | `command` | `-v\|-V` |  |  |  |  |
| 32 | `echo` |  |  |  |  |  |
| 33 | `printf` |  | `-v` |  |  |  |
| 34 | `realpath` |  |  |  |  |  |
| 35 | `readlink` |  |  |  |  |  |
| 36 | `basename` |  |  |  |  |  |
| 37 | `dirname` |  |  |  |  |  |
| 38 | `tree` |  | `-o` `-R` |  |  |  |
| 39 | `rg` |  | `--pre` `--hostname-bin` |  |  |  |
| 40 | `grep` |  |  |  |  |  |
| 41 | `egrep` |  |  |  |  |  |
| 42 | `fgrep` |  |  |  |  |  |
| 43 | `diff` |  |  |  |  |  |
| 44 | `cmp` |  |  |  |  |  |
| 45 | `sort` |  | `-o` `--output` `--compress-program` `--files0-from` |  |  |  |
| 46 | `uniq` |  |  |  | 1 |  |
| 47 | `cut` |  |  |  |  |  |
| 48 | `tr` |  |  |  |  |  |
| 49 | `column` |  |  |  |  |  |
| 50 | `jq` |  | `-i` `--in-place` `env` `ENV` |  |  |  |
| 51 | `ps` |  | `e` `-e` |  |  |  |
| 52 | `ps` | `-e\|-ef\|-eF\|-ely\|-eLf\|-ejH` |  |  | 0 | 0 |
| 53 | `pgrep` |  |  |  |  |  |
| 54 | `ss` |  | `-K` `--kill` `-D` `--diag` |  |  |  |
| 55 | `ip` | `addr\|address\|a` |  |  | 0 |  |
| 56 | `ip` | `addr\|address\|a` `show\|list` |  |  |  |  |
| 57 | `ip` | `route\|r` |  |  | 0 |  |
| 58 | `ip` | `route\|r` `show\|list` |  |  |  |  |
| 59 | `ip` | `link\|l` |  |  | 0 |  |
| 60 | `ip` | `link\|l` `show\|list` |  |  |  |  |
| 61 | `journalctl` |  | `--vacuum*` `--rotate` `--flush` `--sync` `--relinquish-var` `--smart-relinquish-var` `--setup-keys` `--update-catalog` `--cursor-file` `-f` `--follow` |  |  |  |
| 62 | `systemctl` | `status\|list-units\|list-unit-files\|is-active\|is-enabled\|is-failed\|cat` | `-H` `--host` |  |  |  |
| 63 | `systemctl` | `--user` `status\|list-units\|list-unit-files\|is-active\|is-enabled\|is-failed\|cat` | `-H` `--host` |  |  |  |
| 64 | `systemctl` | `show` | `-H` `--host` `-p` `-P` `--property` | 1 |  |  |
| 65 | `systemctl` | `--user` `show` | `-H` `--host` `-p` `-P` `--property` | 1 |  |  |
| 66 | `git` | `status\|diff\|log\|show\|rev-parse\|ls-files\|blame` | `--output` |  |  |  |
| 67 | `git` | `--no-pager` `status\|diff\|log\|show\|rev-parse\|ls-files\|blame` | `--output` |  |  |  |
| 68 | `git` | `branch` | `-d` `-D` `--delete` `-m` `-M` `--move` `-c` `-C` `--copy` `-f` `--force` `-u` `--set-upstream-to` `--unset-upstream` `--edit-description` `-t` `--track` `--no-track` `--create-reflog` |  | 0 |  |
| 69 | `git` | `--no-pager` `branch` | `-d` `-D` `--delete` `-m` `-M` `--move` `-c` `-C` `--copy` `-f` `--force` `-u` `--set-upstream-to` `--unset-upstream` `--edit-description` `-t` `--track` `--no-track` `--create-reflog` |  | 0 |  |
| 70 | `git` | `remote` |  |  | 0 |  |
| 71 | `git` | `--no-pager` `remote` |  |  | 0 |  |
| 72 | `pacman` | `-Q*\|--query` |  |  |  |  |
| 73 | `pacman` | `-Ss\|-Ssq\|-Sqs\|-Si\|-Sii` | `-y` `-u` `-w` `-c` `--refresh` `--sysupgrade` `--downloadonly` `--clean` |  |  |  |
| 74 | `lspci` |  |  |  |  |  |
| 75 | `lsusb` |  |  |  |  |  |
| 76 | `sensors` |  | `-s` `--set` |  |  |  |
| 77 | `nproc` |  |  |  |  |  |
| 78 | `find` |  | `-exec` `-execdir` `-ok` `-okdir` `-delete` `-fprint*` `-fls` |  |  |  |

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
The UNIX forms `ps -e`, `ps -ef`, `ps -eF`, `ps -ely`, `ps -eLf` and `ps -ejH` run
alone, with nothing after them. A `jq` program may not use `env` or `$ENV`, and
`systemctl show` must name a unit (without one it shows the service manager's
environment).

`crates/efr-permissions/src/policy/defaults.rs` holds the table as data. A test keeps
this file, the table in the crate and the data equal.

## How the engine reads a command line

A command rule judges one simple command at a time. The engine splits a line on `;`,
`&&`, `||`, `|`, `|&` and newlines. It reads single quotes, double quotes and
backslashes as zsh reads them, so `rg 'a|b' src` is one command. It allows a line only
when it allows every simple command in it: `git status && git push` asks for
`git push`. The reason names the part that asks.

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
  is judged by its parts, and its glob reads everything below `.` (see below);
- a builtin that runs a string as commands or changes how later commands run: `eval`,
  `exec`, `source`, `.`, `alias`, `export`, `set`, `trap`, `builtin` and more;
- an unclosed quote, a backslash at the end of the line, a control character, a
  non-ASCII character outside quotes, or a `#`, `^`, `=` or `~` at the start of a
  word (`~` and `~/...` are fine, `~root` is not).

`sudo`, `sudoedit`, `doas`, `su`, `pkexec` and `run0` always ask. No command rule
matches them, also behind a wrapper such as `env` or `nice`. If a rule for every
command line allows them, the engine still asks.

## Which paths a command reads

A read-only command still reads files, and the path rules judge what it reads. The
shell tool declares the paths that a line names, resolved against the hidden shell's
directory:

- every operand of every program is a read, and so is an option value that looks like
  a path, such as `--file=x` or `-f/x`, and the path in `HEAD:path`. `echo`, `printf`,
  `basename`, `dirname`, `which`, `type` and a few more declare nothing, because their
  arguments are not files. `$HOME` at the start of a word counts as `~`;
- `ls` with no path lists the working directory, which is a read of it;
- `rg`, `grep -r`, `find`, `du`, `tree`, `ls -R` and `diff` read everything below their
  paths. With no path, they read the working directory. An option that the shell tool
  does not know makes it assume the worst: every operand is a path, and the working
  directory is read too;
- a glob reads everything below its fixed directory: `cat ~/.ss*/id*` reads all of `~`;
- `< file` is a read, and `> file` or `>> file` is a write;
- the commands inside `$(...)`, backquotes and groups declare their paths too;
- a `cd` in the line adds its target as one more directory where the rest of the line
  may run. After `cd -`, `popd` or `cd $DIR`, a relative path may be anywhere below `/`.

So `cat ~/.ssh/id_ed25519` is denied, although `cat` runs freely. A read of everything
below a directory asks when a secret lies below it, unless a rule allows that secret:
`rg TOKEN ~/.aws` asks, because `~/.aws/credentials` lies below `~/.aws`. Naming a
secret denies the call without asking you, unless one of your rules opens it.

What the text cannot show, the engine cannot judge. The shell tool resolves paths
lexically, so it cannot follow a symbolic link that an earlier approved command made.
A rule trusts that a program name means what it says: an alias or a function of the
same name in your startup files changes what runs, and so can a repository's own
configuration for `git`. A script or a build reads files that the line does not name.

## Your rules

Put your rules in `$XDG_CONFIG_HOME/efr/config.toml`. Each rule has an `action`
(`any`, `read`, `write`, `execute` or `network`), a `resource` and an `effect`
(`allow`, `ask` or `deny`). The resource is one of:

- `"any"`: every path, command line and network access;
- `{ class = "user_config" }`: every path of one class (`scratch`, `user_config`,
  `user_data`, `system`, `secrets`);
- `{ under = "~/.config/nvim" }`: a path and everything below it, absolute or under
  `~`;
- `"project"`: every path inside the root of the turn's registered project;
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
  - `under`: a directory where the command must run, or below it. It is the hidden
    shell's directory when the line starts. After a `cd`, `pushd` or `popd` earlier in
    the line, the directory is unknown and the rule matches nothing.

Your rules come after the built-in rules, so the last one of yours that matches wins.
Your rules are the only rules that can open a secret. A rule that does not have the
shape of a rule, or that names a relative path, a program that is not one word, or an
action that its resource never matches, stops the daemon at start. The error names
the rule as `permissions.rules[N]`, counted from 0. `efrd --print-config` shows your
rules.

### Example: allow `cargo test` in one project

```toml
[[permissions.rules]]
action = "execute"
resource = { command = { program = "cargo", args = ["test"], under = "~/p/app" } }
effect = "allow"
```

`cargo test` and `cargo test --workspace` run without approval when the hidden shell is
in `~/p/app` or below it. In `~/p/other`, and after a `cd` in the same line, they ask.
`cargo test` builds and runs the project's own code, so allow it only in projects whose
code you trust.

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
