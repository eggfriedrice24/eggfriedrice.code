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

Every turn runs in one of four modes. The mode is the built-in policy of the turn,
and your `[[permissions.rules]]` come after it, so your rules win where they match in
every mode.

| Mode | What runs without a question |
|---|---|
| `manual` | Nothing, except what your rules allow. Every read, write, command and network access asks; secrets are denied. |
| `cautious` | The default: reading outside secrets, writing in `$SCRATCH` and in the turn's registered project, and the read-only commands of the table below. |
| `auto` | Every shell command, in a kernel sandbox: it writes only in the turn's project, the registered projects that it names, `$SCRATCH`, a private `/tmp` and private tool caches, and it has no network. An action that leaves the sandbox (an exit) asks. The file tools follow `cautious`. See "The auto sandbox". |
| `full` | Every command and file tool call, as you, with no sandbox and no question, after you allow the mode with `permissions.allow_full = true`. efr still reads each line: it denies secrets and its own config, and asks before `sudo`, before a process that outlives the turn and before a delete of `~`, `/`, a system dir or a project. A script or a program that a line starts is not read. See "Full mode". |

`permissions.mode` in `config.toml` sets the mode of a turn when the prompt names none.
It can be `full` only next to `permissions.allow_full = true`; see "Full mode". A turn
keeps its mode until it ends. In `auto`, the kernel holds what a command does,
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

A turn from the phone runs with at most `cautious`: `auto` and `full` there count as
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

Some floors hold in every mode, whatever the rules say. In full mode most of them hold
only for what the command line shows; see "Full mode":

- a call that can wait for input at the terminal, such as `sudo`, asks, except in
  full mode, where only the privileged programs of the next item ask. A line that may
  ask for a password through ssh (`rsync` to another host, `scp`, `sftp`, `sshfs`,
  `git` over ssh and the others of "A command left running", below) asks nothing for
  that alone. Once either runs outside the sandbox, it may run past the model's
  timeout while a terminal that can type your answer follows the turn, up to
  `interactive_timeout_minutes` under `[shell]` (60 by default), whoever approved it:
  you, your rule, the reviewer or full mode. Any command that waits for input when its
  timeout passes goes on the same way. A call in the sandbox never gets this limit;
- a program that runs commands as another user (`sudo`, `doas`, `su`, `pkexec`,
  `run0`) asks, and only your own machine answers the question, never a phone;
- a turn from the phone runs with at most `cautious` and asks outside `$SCRATCH`;
- in full mode, a delete of `~`, `/`, a system dir, a project or a dir that holds
  efr's files, a secret or more than one project asks you at your own machine, and
  the question waits at most 2 minutes; see "Full mode";
- full mode never runs while efrd runs as root, while `snapshot.enabled` or
  `undo.enabled` is `false`, or when the hidden shell is no zsh;
- secrets are denied unless a rule of yours names them, and the daemon's own
  `secrets/` is denied even then;
- the rules of a conversation only make a decision stricter;
- no tool writes efr's config; see "Config protection";
- a change of efr's settings through the settings tool asks, and a turn from the
  phone cannot make one; see "The settings tool";
- a background job asks before it starts in `manual` and `cautious`, and its question
  is local only; in `auto` and full mode it starts with no question; privilege, masked
  reads, persistence, desktop commands that type or start programs, the bus and
  container engine or service manager sockets never run as a job; see "Background
  jobs";
- a delete or a move of a file through `apply_patch` asks in `manual` and `cautious`;
  in `auto` and full mode it runs when efr's snapshot holds each file, so `efr undo`
  brings it back; see "Edits with apply_patch".

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
  write`. The reviewer (below) decides most exits; the others ask you. After an allow,
  the call runs in the sandbox with exactly that path, socket, device or the network
  opened for this one call, or, for privilege, persistence, upload and some writes,
  outside the sandbox. The model can also ask for an exit with the shell tool's
  `needs`.
- A line that the engine cannot read gets only the exits that it can find. It runs in
  the sandbox, which holds it.
- The floors hold, as in every mode: a privileged program asks, and only you can
  approve it; a secret is denied unless a rule of yours names it; efr's config, efr's
  own programs (`efrd`, `efr`, `efr-sbx` and its copy) and efr's own dirs
  (`$XDG_RUNTIME_DIR/efr`, `~/.local/state/efr`, and `~/.local/share/efr` except
  `$SCRATCH` and efr's trash) are denied; a change of the settings asks you.
- `nested_shell` is denied in `auto`.
- `read_file`, `write_file`, `edit` and `apply_patch` run in efrd, outside the
  sandbox. They follow the rules of `cautious`, plus these: a read of a sandbox mask,
  such as a project `.env`, is a `masked_read` exit; a write outside the turn's project
  and `$SCRATCH` that a built-in rule asks about is an exit of its kind (`write`,
  `synced_write`, `above_root` or `persistence`), as a shell write there is; and a
  write to a floor path inside a write root, such as `.git/hooks`, is a `persistence`
  exit. A rule of yours keeps its effect and makes no exit.

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
the host's real `/tmp` and caches. So rule 0 makes such a write an exit, which the
reviewer decides or you decide, as for the same write of a shell call.

**Exits.** An action that leaves the sandbox is an exit. The engine reads the line,
the paths that the tool declares and the facts that the daemon collects, and gives
each exit its own reason and the one who may approve it: the reviewer, the reviewer
when a message of yours names the target, or only you. `secret` and `config` are
denied. Each exit has a kind:

| Kind | Examples |
|---|---|
| `write` | a write outside the write roots: `echo x > ~/notes.txt`, `git worktree add ../wt`. A write to a node that the sandbox has, such as `2>/dev/null` or `>/dev/stderr`, is not an exit. A path below such a node, such as `/dev/fd/3/x`, goes through an open file to a place that efr cannot see, so it is a write that only you approve |
| `host` | with `network.mode = "none"`: every network need, such as `curl`, `git fetch`, `npm ci` or `checkupdates`, with the whole network for the call. With the proxy (`network.mode = "proxy"`): a host that the line or a git remote shows and that is not on the allow list, with a grant of that one host and port (`curl https://example.com/x`); a raw network program (`nslookup`, `dig`, `host`, `ping`, `traceroute`, `nc`, `socat`, `telnet`, `ftp` and the like) and a `needs.hosts` entry of `localhost`, a private or link-local address, or a port other than 443 and 80, with the whole network. A network program whose hosts are all on the allow list, or that names no host (`cargo build`, `npm ci`), makes no exit: the proxy decides. A host that the deny list holds makes no exit either; the proxy refuses it. In both modes, a URL on `localhost`, `127.0.0.1` or `[::1]` whose port efr forwards for a running job of the conversation, and such a `needs.hosts` entry, make no exit: a contained call reaches that port in the sandbox (docs/sandbox.md, "Ports of a job") |
| `host_view` | `iw`, `ifconfig`, `netstat`; `ss` and `ip` in a line that also does other work (`ss -tln; make test`), in a form that changes something (`ss -K`, `ip link set`), or with the network view off. A view line has no exit ("Views", below) |
| `outside` | `df`, `findmnt` only when efrd runs in a mount namespace of its own, such as a hardened unit; else they run in the sandbox with the mount view and no exit (docs/sandbox.md, "The host's mounts"); the model asks to run outside (the reviewer; only you when a program word of the line changed in this turn or lies in a write root, or when the line runs an interpreter on code that efr cannot read, such as `python3 -c`, `sh -c` or `node build.js`); `rustup toolchain install`, `rustup install`, `rustup update`, `rustup target add`, `rustup component add` and `rustup self update` (the reviewer for a release channel such as `stable`, `beta`, `nightly` or `1.90`, with a date and a target if you like, when a message of yours names `rustup`, the toolchain or the channel; only you for a line that sets `RUSTUP_*` or `CARGO_HOME`, names a toolchain path or a custom toolchain, or runs `rustup toolchain link`, because the install puts programs on your `PATH`); with the proxy, `ssh`, `mosh`, `autossh`, and `git fetch`, `git pull` or `git clone` from an ssh remote (the record names the host; the reviewer needs a message of yours that names the host, or a remote of your project; a destination that efr cannot read, such as `$HOST`, an ssh URL of `.gitmodules`, and a line that can send ssh to another host first or run a command on your machine, such as `ssh -J`, `ssh -F`, `ssh -o ProxyCommand=...`, `mosh --ssh`, `git -c core.sshCommand=...` or `GIT_SSH_COMMAND=...`, ask only you) |
| `socket`, `bus` | a Unix socket; `busctl`, `resolvectl`, `gdbus`, `coredumpctl`, `notify-send --wait` and the `systemctl` forms that are not read forms, such as `systemctl get-default`; `hostnamectl`, `systemctl --failed`, the other read forms and `notify-send` only while the bus filter is off ("Views", below); `nmcli` and `iwctl` (the reviewer for a read form, such as `nmcli device status` or `iwctl station wlan0 show`; only you for every other form, such as `nmcli radio wifi off`, because the bus filter has no rules for NetworkManager and iwd). A `bus` exit of the session bus asks only you, never the reviewer, because the whole session bus can start programs outside the sandbox. A socket inside a secret, such as `~/.gnupg/S.gpg-agent`, is a `secret` floor, and one in efr's own dirs a `config` floor. Only you approve the socket of a credential agent (`$SSH_AUTH_SOCK`, gpg-agent, a keyring, a password manager's agent or git's credential cache) and a socket under a mask. A service manager's socket, such as `$XDG_RUNTIME_DIR/systemd/private`, is a `privilege` exit. A socket grant to a bus, a service manager's socket or a desktop socket fails at the start of the call, also through a link |
| `desktop_ipc` | the read and reload forms `hyprctl clients`, `hyprctl reload`, `swaymsg -t get_*`, `i3-msg -t get_*`, `xrandr` without a change, `wlr-randr`, `niri msg` (the reviewer); a form that types or starts programs, such as `hyprctl dispatch exec` or `xdotool type`, an unknown form and a desktop socket that the model names (only you approve). The call gets the one control socket of the program's desktop and its variable (docs/sandbox.md, "Desktop sockets") |
| `device` | a node below `/dev` that the sandbox does not have: `nvme smart-log /dev/nvme0n1`, `cat x > /dev/sda` |
| `masked_read` | a read of a masked path: `.env`, `~/.zsh_history`, a browser profile (only you approve) |
| `destructive` | `git reset --hard`, `git clean -f`, `git checkout -- P`, `dd of=`, `shred`, `: > F`, `rm -r` of tracked files (no question and no review when efr's snapshot holds every file that the part can change and `sandbox.ask_destructive = "unless_undoable"`, the default, so `efr undo` brings it back; `shred`, `git clean -x` and `-X`, `git reflog expire` and `git gc --prune` are never covered; else the reviewer when every target lies in the project, `$SCRATCH` or `sandbox.write_roots`; else only you; docs/sandbox.md, "Undo") |
| `privilege` | `sudo`, `doas`, `pkexec`, `run0`, `yay`, `paru`, `docker`, `podman`, a changing `systemctl`, `busctl call` (only you approve) |
| `persistence` | an edit of a floor file, such as `~/.zshrc` or `~/.config/hypr/hyprland.conf` (the reviewer when a message of yours names the file, and efr copies it first, so `efr undo` reverts it; with `snapshot.enabled = false` only you); a file tool edit of such a file sends its text to the reviewer, and a text over 8 KiB asks you; a named unit change (`systemctl --user enable`, `mask` and the like of a unit that was in a unit dir before the turn), a link into a `*.wants` dir or a copy into a floor dir from a file outside every write root, `crontab <file>`, `crontab -` fed a literal, `crontab -r`, `loginctl enable-linger` and `disable-linger` (the reviewer when a message of yours names the unit, cron or linger, and efr copies the links, the crontab or the setting first and sends the installed text to the reviewer; else only you); `systemctl --user link`, `edit` and `revert`, `crontab -e`, `at`, `systemd-run --user`, another link, a dir, `.git/hooks` and `.git/config`, and `sandbox.protect` (only you approve) |
| `upload` | `git push`, `gh pr create`, `scp`, `rsync host:`, `curl -d`, `-F`, `-T` or `-X POST` (the reviewer when a message of yours names it, such as "push this branch"; else only you). The record of `git push` names every host that git pushes to behind the remote name; a `git push` whose host efr cannot tell (`--git-dir`, `-c`, a `GIT_*` variable, a `cd` first, a local path) or in a line that sets a `GIT_*` or `SSH_*` variable asks only you. Before a git line that reaches a remote (also `git send-email`) or a gh command that runs git (`gh pr create`, `pr merge`, `pr checkout`, `repo sync`, `repo create --push`, `repo fork --clone`) runs outside the sandbox, efrd checks the git code of the repository and of its submodules: the hooks, the config keys that run programs and the programs that they run. When a sandboxed command can write one of them, or efrd could not check them, only you approve, and the question names the file; a git option that chooses the code that git runs (`-c`, `--config-env`, `--exec-path`, `--git-dir`, `--work-tree`, `--namespace`, `--receive-pack`, `--exec`, `--upload-pack`, `--template`, and the program options of `send-email`) asks only you too, also on an `outside` git line, and so does a git or gh line that sets a `GIT_*`, `SSH_*`, `HOME`, `XDG_CONFIG_HOME` or `PATH` variable (docs/sandbox.md, "Git settings"). `npm publish` (and `pnpm`, `yarn`, `bun`), `cargo publish` (also with `--no-verify`), `cargo yank` and `cargo owner` ask only you: they run project code with your rights or read `.cargo/config.toml`, which can name a credential program, and the question says which. An upload always runs outside the sandbox, never through the proxy |
| `synced_write` | a write to `~/Dropbox` and the other `sandbox.synced_dirs` (only you approve) |
| `above_root` | a write at or above a write root or efr's config: `rm -rf ..`, `rm -rf ~/p/old-app` (the reviewer for one path that a message of yours names and that holds no floor; a project's own `.git` and protected names do not count; else only you, also for `~`, `/`, a system dir and a dir that holds more than one project) |
| `secret`, `config` | a read or write of a secret, a write of efr's config, of efr's own programs or of efr's own dirs: denied |

**Views.** A view is a read-only view of the host that a contained call gets with no
exit, no review and no question. The engine gives it as a reason of its own
(`Subject::View`), never as an exit. There are two views. The first is the host's
network view (`Grant::NetView`). A line gets it when each simple command is one of these: `ss` (not
`-K`, `-D` or `-N`), a read form of `ip` (not `-batch`, `-netns` or `-all`), `nstat`
(not `-d`), a read of a file below `/proc/net`, or a simple text helper such as
`grep`, `sort` or `sleep`. `watch`, `timeout` and a `while` loop around them are
allowed. No interpreter, `find`, `git` or `xargs` can be part of such a line. The call
then runs in the sandbox in your network namespace, where seccomp allows only Unix and
netlink sockets (docs/sandbox.md, "The host's network view"). This rule only picks the
lines that fit. It is not a security layer. With `network.netview = false`, or when
the probe's self-test of the view failed, the same lines are `host_view` exits. When
the line also has an exit, the launch keeps the view only beside `write` and `unmask`
grants. With every other grant, such as the network, a device or a bus, the view
drops, and the call gets only that grant.

The second is the filtered bus (`Grant::BusFilter { bus }`, phase E). With
`dbus.proxy = true` and a filter that passed its self-test, a line gets it when at
least one simple command, in any place of the line, is a read form of `systemctl`,
`systemctl --user`, `loginctl`, `hostnamectl`, `timedatectl` or `localectl` with no
`-H`, `--host`, `-M`, `--machine`, `--root`, `--image` or `--image-policy`, or a
`notify-send` that does not wait (no `-w`, `--wait`, `-A` or `--action`).
docs/sandbox.md, "The bus filter", lists the forms and the methods that the filter lets
through. The rest of the line gets the same filtered bus, so this rule too only picks
the lines that fit. The view drops beside the network view and beside a `bus` exit of
the same bus, which gives the whole bus. With the filter off, the same forms are `bus`
exits. A background job with a read form gets its own filter for its whole life; the
whole bus stays refused for jobs.

The engine reads every line, also one with a substitution or a group, and finds the
exits that it can see. Text can only add a question; the sandbox holds what the text
does not show. When a fact is missing, the engine assumes the stricter case: a target
exists, and a directory holds tracked files.

After a command fails in the sandbox, the model can call `shell` again with `needs`:
paths to write, hosts, sockets, a bus, a device, masked paths to read, or `outside`.
Each entry is an exit, which the reviewer decides or which asks you. Other modes
ignore `needs`. With the proxy, efr reads each host of `needs.hosts` as `host:port`:
`http://host` means port 80 (plain `GET` and `HEAD`), else the port of the word, else
443. A host on the allow list for that port drops out with a note, because the proxy
already allows it; a host on the deny list drops out with the note `the user's config
denies <host>`; a name that is not ASCII drops out with the note "write the host in
its ASCII (xn--) form". `localhost`, a loopback, private or link-local address, and a
port other than 443 and 80 give the whole network for the call. Every other host
gives a grant of that one host and port.

**The reviewer.** In `auto`, the user's request is the permission. A reviewer model
decides most exits, so you see far fewer questions. It reads your messages and the
facts that efr collects (the line, the targets, the hosts, the programs and counts),
and never the output of a command, a file of the project or the model's reason. It
allows an action that fits what you asked, refuses an action that nothing you said
relates to, and hands an unsure case to you. efr then applies two rules of its own:

- A `critical` risk never allows, and a `high` risk allows only when a message of
  yours clearly authorizes it.
- An allow of a write needs a message of yours that names each target, unless the
  target is in a tool cache, in build output below a registered project or in efr's
  trash, which a tool or efr can make again. An allow of a host that is not on the
  allow list needs a message that names the host, except a package fetch of a request
  to build, install or update. Without the name, the allow goes to you. A host is on
  the allow list (`on_allow_list` in the record) only with the proxy, only for an exit
  whose grant for that host is a grant of that one host, and only on the exit's own
  port; an exit with the whole network, an `upload` and an `outside` exit always need
  the name. efr knows a host from the line (the host of a URL, of a remote operand of
  `scp`, `sftp` or `rsync`, the destination of `ssh`), and from the git remote facts
  below. A host that a script connects to has nothing to check until the proxy refuses
  it.

A message of yours names a target, a host or an upload only when it is in the current
turn or in your turn before it. An older message does not count, so a "now do it" three
turns after "commit and push" asks you. After another terminal picked up the
conversation, or you resumed it, only your messages since then count. In a turn that a
job notice started, your newest turn before it and the turn before that count. The
reviewer still reads every message of the conversation.

**Git remote facts.** For `git fetch`, `git pull`, `git ls-remote`, `git remote
update`, `git push` and `git submodule update`, efrd reads the remotes of the
repository with one hardened `git config` run (`remote.*.url`, `remote.*.pushurl`,
`url.*.insteadOf`, `url.*.pushInsteadOf` and `branch.*.remote`; git applies your
global files and includes), and the URLs of `.gitmodules` for `submodule update`. The
remote is the one that the line names, else the remote of the current branch, else
`origin`. Every fetch and push URL of it adds its host to the record of the line's
`host`, `upload` and `outside` exits. Under the proxy, a fetch from an https remote
that is not listed is a `host` exit for that one host, and a fetch from an ssh remote
is an `outside` exit with the host.

- `project_remote`: the host belongs to a remote of a git dir that the plan pins (the
  top `.git` of a registered project, or a registered git dir), whose URLs are the
  ones that efrd saw at the conversation's first call. Such a host counts as named by
  you for `host`, `upload` and `outside` exits; an `upload` still needs its verb, such
  as "push", in a message of yours. A remote of a repository that a command made, a
  nested repository and a URL of `.gitmodules` never count.
- `refused_by_proxy`: the proxy refused the host in the conversation's last command or
  in one of its jobs. This is a fact for the record and the question, never a name:
  it shows only that a command tried the host, never that you want it.

The reviewer is the small model of your provider, then the turn's model, then you:
[`docs/providers.md`](providers.md) tells which models and what they cost. A missing
model, an error, a timeout or an answer that does not fit goes to the next step, and
never allows. The `[classifier]` keys turn the reviewer off, choose the model, the
effort and the timeout. While it works, the status row shows `reviewing: <command>`,
and its answer shows as one line, `reviewer allowed: ...` or `reviewer refused: ...`.

After the reviewer refused 3 actions in a row, or 10 in one turn, it pauses for the
rest of the turn, and each later exit asks you. An error or an expired question is no
refusal; an allow or your answer starts the count in a row again. When you are away,
the turn stops instead.

**Away.** efr counts you away from a conversation after `presence.away_after_minutes`
(10) without input, or after `efr away` or `,away`. Your next input there ends it.
While you are away, a question waits `presence.away_wait_secs` (300); then the call
does not run, and the model goes on without it and names it in its answer. A question
that already waits when you leave gets the same wait, from the moment efr sees that you
are away (it looks every 15 seconds).

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

## Always-allow rules

In `auto`, an `a` (always) answer at an exit question saves a standing rule in
`[[sandbox.allow]]` of `config.toml`, after a confirm (`a`, then `y`). The card offers
`a` only when a rule may cover the exit (`ExitInfo.always`), and only from a terminal on
your machine. A later shell call that the rules cover runs with no review and no
question, with the same grants that a "yes" gives, also while you are away, in a job
and in a turn that a job notice started. docs/sandbox.md, "Always-allow rules", shows
the confirm, the lines and the commands.

The rules sit below every floor and above the reviewer. The order is: the floors, your
`deny` and `ask` rules, the always rules, the reviewer, you. A rule has one of three
kinds:

- `host`: one host and port through the proxy. It never covers the whole network, so
  with `network.mode = "none"` it does nothing. `network.deny` still wins.
- `write`: one dir below your home; it covers a contained write whose binds all lie at
  or below it. With `project`, only in turns of that project.
- `socket`: one socket, never a link, never in a write root, and never a service,
  container, bus, desktop or credential socket.

A rule covers a call only when it covers every exit of the call, each exit is one that
the reviewer may judge and that runs in the sandbox with narrow grants, each path
matches by its real path (efrd resolves the links before the call; a path with no such
fact is not covered), no floor, synced dir or project lies at or below a dir that the
call opens (also a floor that does not exist yet), and no `deny` or `ask` rule of yours
names a path at or below a grant. So a rule never covers a privilege, persistence,
upload, desktop, masked-read, synced-folder, bus or device exit, `outside`, a delete in
a cache or a file tool.

efrd checks each rule against the floors when it loads the file, and before it saves
one. A rule of a bad shape stops efrd at start and makes a reload fail. A rule that
crosses a floor stays in the file, disabled with its reason: efrd logs one warning,
`efr sandbox check` shows `sandbox.allow[N] is disabled: <reason>`, and
`efr sandbox allow` shows `disabled: <reason>`. A reload whose only change is in `[[sandbox.allow]]` sends
the new rules to the engine and starts no probe. `efr config check` checks only the
shape. `efr sandbox allow remove N` takes a rule out.

There are two allow lists. `[[sandbox.allow]]` covers shell calls in `auto`.
`[[permissions.rules]]` with `write allow` (below, "Your rules") covers `write_file`
and `edit` in every mode, also in `auto`.

## Full mode

In full mode, the model's commands run as you, with no sandbox and no questions. Read
this section before you turn it on. Where `auto` can run, `auto` is the safe choice:
its sandbox holds every program that a command starts, and full mode holds none.

**What full mode promises.** Full mode is a protection against mistakes, not against a
hostile model or a prompt injection. efr reads each command line before it runs. A
line that reads a secret, writes efr's config or dirs, uses `sudo`, starts a process
that outlives the turn, or deletes a tree that full mode guards stops at a floor or a
question. A program that the line starts (a script, `python3 -c`, a build step, a
program that the model wrote) is not read: it can do what you can do, also answer
efr's own questions on its socket, write `config.toml`, or send an API key from the
environment to a host. efr records and can undo only what it saw. Full mode offers no
protection against a prompt injection or a mistake in a script.

### Turn it on

1. Set `permissions.allow_full = true` in `config.toml` yourself: `efr config edit`,
   or `efr config set permissions.allow_full true`. efrd reads the change at once. The
   settings tool of the model never changes this key.
2. Choose full mode where you want it:
   - `,mode full` for the prompts of one terminal, `,mode default` to stop;
   - `efr send --mode full` (or `--mode=full`) for one prompt;
   - `permissions.mode = "full"` next to `permissions.allow_full = true` for every
     turn that names no mode. Without the gate key, `mode = "full"` is an error: efrd
     does not start, a reload keeps the old settings, and the error says to set
     `permissions.allow_full = true` or another `permissions.mode`. With full mode as the default, efrd writes a
     warning to its log at start and at each reload that makes full the default,
     `efr status` shows `default mode full`, and `,mode cautious` or `,mode auto`
     still moves one terminal down. To turn full mode off while it is the default,
     set `permissions.mode` to another mode first, or in the same edit: the gate key
     alone cannot go while `mode = "full"` stays.

`,mode full` asks efrd first. When `config.toml` does not allow full mode, or efrd
refuses it, it fails with the reason, and the terminal keeps its old mode. Else it
prints three lines:

- `efr: full mode: commands run with your rights, with no sandbox and no questions.`
- `efr: efr reads each line: it denies secrets and its own config, and asks for sudo,
  for a process that outlives the turn and for a delete of ~, / or a project.`
- `efr: a script or a program that a line starts is not read; it can do what you can.`

`efr status` shows `full mode allowed`, `full mode off (permissions.allow_full is
false)` or `full mode refused (<reason>)`. The zsh plugin does not take `full` from an
`EFR_MODE` of the environment when it loads; it says so on one line. `,mode full` is
the only way to make a terminal full. A script that runs `efr send` with
`EFR_MODE=full` chose full mode for that prompt.

**The gate is a config value, not a lock.** efr's tools never change it, and in
`manual`, `cautious` and `auto` no command writes `config.toml` without your yes. Once
the gate is on, a command of a full turn can write the file, and a process that a
full turn left behind can turn the gate on again after you turned it off.

### Where full mode never runs

- A turn from the phone runs with at most `cautious`, with no error. A steer from the
  phone never goes into a running full turn: it fails, or it waits as a prompt of its
  own that runs with at most `cautious`.
- A turn that a job notice starts runs with at most `auto` (as `cautious` when the
  sandbox cannot run), and records the fallback `a turn that a job notice starts runs
  with at most auto`.
- efrd refuses full mode while it runs as root, while `snapshot.enabled` or
  `undo.enabled` is `false` (then `efr undo` cannot revert the turn), and when
  `shell.program` is no zsh (then efr cannot clear sudo's cached password before a
  call). A prompt that names full mode then fails with the reason, such as `full mode
  cannot run: efrd runs as root`. With full mode as the default, a turn that names no
  mode runs as `auto` and records why.
- When you turn the gate off, or turn undo or the snapshots off, while a full turn
  runs, the next call of that turn does not run, and the turn stops with `full mode
  was turned off while this turn ran; send the prompt again`. A full prompt that waits
  in the queue fails when it starts. efrd also stops each background job that a full
  call started, as `efr jobs stop` does, right after it loads the new settings. A job
  of another mode runs on. `efr jobs` and the notice line say `stopped: full mode was
  turned off`. Such a stop starts no job notice turn: the notice goes to the model
  with your next prompt.

### What still holds

A floor is "hard" when it holds for every process. It is "text" when it holds only for
what the command line shows: a script or a program that the line starts gets past it.

| Floor | Kind | What happens |
|---|---|---|
| Secrets, efr's config, efr's own programs and dirs | hard for the file tools, text for a shell line | denied. A shell line that names such a path is denied. A link that the same line makes is not resolved, so a later path below it may be anywhere: `ln -s ~/.config/efr c && cp x c/config.toml` asks, as a write of a dir with efr's config below it |
| A read or write of a whole dir with a secret or efr's config below it | text | asks, as in `cautious`, because no sandbox masks the secret or keeps the config read-only: `rg TOKEN ~/.aws`, `grep -r KEY ~`, `du ~` and `cp -r x/. ~/.config/` ask |
| A delete that full mode guards | text | asks, at your own machine, at most 2 minutes ("The delete guard", below) |
| `sudo` and the other programs that give more rights | text | asks, and only your own machine answers. One question covers the whole line: a yes runs every command of the line, and the card says so |
| A process that outlives the turn | text | asks: `systemd-run --user`, `at`, `batch`, `crontab`, `systemctl --user enable` and the other `persistence` forms, and a write of a file that runs code later, such as `~/.zshrc`. A phone can answer it, as in `auto` |
| A delete that `efr undo` cannot revert | text for a shell line | asks, as in `cautious`, when the snapshot does not hold every file of the delete (over its limits, or a file that it leaves out). A job refuses it |
| A change of efr's settings through the settings tool | hard | asks, as in every mode |
| Your own `ask` and `deny` rules | text for a shell line | keep their effect. A rule `[[permissions.rules]]` with `action = "execute"`, `resource = { command = { program = "git", args = ["push"] } }` and `effect = "ask"` keeps a question for `git push`, also `git -C <dir> push` |
| The answer to efr's questions | text | "only your own machine" means a local process, not a person: any process that runs as you on this machine can answer, also one that a full turn left behind |
| The environment of the hidden shell | text | the same as in `cautious`: a command can read an API key from it and send it out, with no record. efr does not clean it in full mode |
| Undo | hard for the project, `$SCRATCH` and the named projects; text outside them | efr takes its snapshot before each call that can write, and copies a file outside the roots first when the line names it. A file that a script writes outside the roots has no copy |
| The record | text | `exit_judged` rows with the judge `full` list what efr saw. A script's actions are not in them |

### What full mode lets through

- Every action that `auto` would hold in its sandbox, and every exit that a built-in
  rule asks about, runs with no review and no question: a write outside the project,
  an upload such as `git push` or `cat .env | curl -T - host`, a synced folder, a
  masked read, the network and the bus. efr records each exit that it saw with the
  judge `full`, and copies a file outside the roots first where `auto` makes a copy.
- No sandbox means no write roots, no masks, no private `/tmp`, no private caches, no
  network proxy and no bus filter. `network.*`, `[[sandbox.allow]]`, `[[dbus.allow]]`
  and `dbus.proxy` do not apply: a command uses your real network and your real bus.
  `systemctl --user start` of a unit that exists is a recorded `bus` exit, and it
  starts a process that outlives the turn.
- No reviewer and no always rule reads a full call.
- A call that waits for input at the terminal and needs no more rights, such as `ssh`
  that asks for a password, runs with no question. While a terminal that can type
  your answer follows the turn, it runs past the model's timeout up to
  `shell.interactive_timeout_minutes`, as in the other modes. Full mode is no
  approval of the line by a person: when the command still runs in the hidden shell
  after its call answered and waits for a password while nobody can answer, efr stops
  it ("A command left running").
- A background job starts with no question; see "Background jobs".

### The delete guard

Before a full call runs, efr reads its line for deletes of a tree that only you may
risk. It reads `rm -r`, a pattern of a plain `rm`, `find` with `-delete` or an
`-exec` of `rm`, `shred`, `srm` or `wipe`, the sources of `mv`, the destination of
`rsync --del*`, `chmod -R`, `chown -R`, `chgrp -R`, `setfacl -R`, `shred -u`, `srm`,
`wipe`, and `git clean -fd`, also through an alias that the line sets, such as
`git -c alias.c=clean c -fd`. An alias that efr cannot read, such as a shell command
with `!`, counts as a `git clean` of the whole work tree. It looks behind `sudo`, `env`, `nice`, `timeout`,
`setsid`, `nohup` and `xargs`, into each substitution, into a branch that never runs
(`false && rm -rf ~`), and one level into the literal string of `sh -c`, `bash -c`,
`zsh -c` and `eval`.

A delete asks when its target is:

- `/`, `~` or a dir above `~`;
- a system dir: a dir directly below `/`, a dir such as `/usr/lib` or `/etc/x`, or a
  disk below `/run/media` or `/media`. `/tmp/x` and `/var/tmp/x` do not ask;
- the turn's project root or a registered project root;
- a dir that holds efr's config or files, a secret, a file that runs code later, or
  more than one project, such as `rm -rf ~/.config` or `rm -rf ~/p`;
- a target that efr cannot read: a word with a variable or a substitution (`$HOME`
  and `${HOME}` at its start read as `~`), a pattern of a tree form whose dir is empty
  or not known (`rm -rf *`, `"$X"/*`), or the input of `xargs`, unless a `find` with
  plain roots that do not ask gives it.

`rm -rf build`, `rm -rf ~/x`, `rm -rf build/*`, `rm *.o` and `rm -rf ~/.cache/pip`
run. A plain `rm` with a pattern asks only in `/`, `~`, a dir above `~` or a system
dir. `git clean` in the turn's own project root runs: the project snapshot covers it.
A delete of efr's own dirs, such as `rm -rf ~/.local/share/efr`, is denied, not asked.
No rule of yours lifts the question, but your `deny` rule wins.

The question shows `full mode still asks for this: delete of ~ (your home directory)`
on its first row and `expires in 2 min`. Only your own machine answers it, and no `a`
(always) is offered. When nobody answers in 120 seconds, the delete does not run, and
the model reads `efr did not run this delete of ~: the user did not answer in 120
seconds. Do not try it another way. Delete narrower paths that you can name, or ask
the user to run it.` When you are away, the delete does not run at once. After 3 such
questions expired in one turn, each later guarded delete of the turn is refused with
no question, and 3 refusals in a row stop the turn. Your answer does not reset the
count. A background job refuses a guarded delete.

The guard reads only the line. These limits are known:

- A delete inside a script or a program, such as `./clean.sh` or `python3 -c`, is not
  seen, and a `sh -c` string inside another `sh -c` string is not read.
- Some deletes that are safe ask: `find . -name '*.o' -delete` and `find . -delete`
  in the project root (the target is the project), `mv *.txt dir/`, `rm -rf *`,
  `rm -rf "$TMPDIR"/x` and `rm -rf $(mktemp -d)`. Name the paths, such as
  `rm -rf build/*.o`, and the delete runs.

### sudo

Every `sudo` line asks, and only your own machine answers. For a full call, efr
forgets sudo's cached password before and after the call, whatever `shell.sudo_cache`
says, so a password from an earlier call or from your own terminal is gone before the
line runs. This needs efr's zsh integration in the hidden shell: a hidden shell that is
no zsh refuses full mode, and a zsh whose integration did not load does not run a full
call. Limits:

- When your sudoers shares the cache across terminals (`timestamp_type=global` or
  `!tty_tickets`), a full call also clears your own terminal's cache.
- A `NOPASSWD` rule, or a polkit rule with no authentication, lets a script that a
  full call starts get root with no question.
- One yes runs every command of the line: `sudo a; sudo rm -rf /etc` is one question.
- The forget covers the calls that run in the hidden shell. A background job that a
  full call starts does not run there, and it needs no forget: efr starts it with
  `no_new_privs`, so `sudo` and every other setuid program get no rights in it. A
  script in such a job cannot use sudo's cached password, also when your sudoers
  shares the cache. `no_new_privs` does not stop a polkit rule with no authentication
  (the line above): `run0` and `systemd-run --uid=0` ask a service outside the job.

### The config hold

A command of a full turn can write `config.toml` with a script, and efrd reloads the
file by itself. While a full turn or a job that it started runs, efrd does not load a
change of `config.toml` that loosens permissions, such as a new `allow` rule,
`permissions.allow_full` turned on or `shell.sudo_cache` set to `keep`. efrd keeps the
old settings and writes one warning to its log. Your terminal shows one line about it,
`efr status` shows `config held` with the keys that loosen, and `efr config reload`
says why: `config.toml changed while a full turn ran, and the change loosens
permissions.rules. If you did not make this change, take it out of the file. Then run
efr config reload.`

- The hold counts every reload: the file watcher, `systemctl --user reload efrd`,
  `efr project add` and `remove`, and also a change of the settings tool and the save
  of an always rule, because their reload reads the whole file.
- The hold stays after the full work ends: each later automatic reload holds again
  while the file still loosens the running settings. A change that does not loosen
  applies as usual and clears the hold.
- The hold stays after a restart of efrd, such as a reboot or `systemctl --user
  restart efrd`. While a change is held, efrd keeps its running settings in
  `held-config.toml` in its state dir, and it starts with them while `config.toml`
  still loosens them. A change that turns full mode off still applies at the start.
- `efr config reload` applies a held change only when no full work runs. While full
  work runs it fails with `efr does not load a change that loosens permissions while a
  full turn or its job runs; run efr config reload again after it ends`.
- The hold also holds a change that you make in your editor while a full turn runs.
  A change that turns full mode off or down still applies at once, also while the
  hold keeps the rest: `permissions.allow_full = false`, a lower `permissions.mode`,
  `undo.enabled = false` and `snapshot.enabled = false`. So you can always stop a
  full turn from the file.
- The hold makes a script's change visible and stops its silent apply. It does not
  stop a process that runs as you: such a process can run `efr config reload` after
  the turn ends.

### What you see

- A full turn starts with one red line: `full mode: no sandbox, no questions; efr undo
  reverts the project and the files efr saw`. The status row shows `full` in red, and
  so does the row of a running call.
- At its end, one dim line lists what efr saw full mode let through, such as `efr saw 2
  actions that full mode let through: write ~/notes.txt, upload (git push)`. A turn
  with no such action shows no line.
- `efr history --verbose` shows `exit allowed by full mode` for each recorded exit.
- A turn that asked for full mode and runs with less starts with `this turn runs as
  auto: <reason>`.
- In sticky mode, the zsh plugin shows the mark 🔓 next to the robot while the
  terminal's prompts run in full mode, such as `🤖🔓 app › `. It reads `config.toml`
  for a default of full. `EFR_FULL_MARK` sets another mark, and an empty value shows
  none. Outside sticky mode the plugin shows no mark: the red first line of
  the turn says full mode. `efr_prompt_info` shows the mark too.

`crates/efr-permissions/src/full.rs` holds the settle pass of a full call,
`crates/efr-permissions/src/exits/critical.rs` the delete guard, and
`docs/adr/0015-full-mode.md` the decision.

## Background jobs

A `shell` call with `background: true` starts a background job: the command runs on
after its call and its turn. [`docs/sandbox.md`](sandbox.md) tells how a job runs and
ends. The engine decides a job as follows.

- Jobs start in every mode. In `manual` and `cautious`, each job asks you first. The
  question names the command, the time limit and the lifetime of the grants: `runs as
  a job for up to 60 minutes; these grants stay open until it ends`. No reviewer
  decides it, and no rule of the mode skips the question. An approved job runs outside
  the sandbox, with your rights.
- In full mode, a job starts with no question and runs with your rights, outside any
  sandbox. The kinds below are still refused, and so are a delete that full mode
  guards, a delete that undo cannot revert, and every use of the bus: with no sandbox,
  efr cannot hold a job to the bus filter. While a job of a full turn runs, the config
  hold of "Full mode" holds.
- In `auto`, the job's line goes through the exits like any line. A line with no exit
  runs at once in the sandbox. The record of an exit has the facts of the job:
  `background`, `max_minutes` (always the time limit that efr uses) and `watch`. The
  reviewer judges the line as it can run for `max_minutes`, not for its first minute,
  and an allow opens the grants for the whole job.
- A covered delete in a job is not routine: the start call answers at once, and the
  undo of the start turn needs `--force`. So a delete in a job goes to the reviewer.
- In every mode, the engine refuses these kinds as a job: `privilege`, `masked_read`,
  `persistence` (also a change that a message of yours names), a desktop command that
  types or starts programs, `bus`, and a `socket` of a container engine or a service
  manager. The model gets `this cannot run as a job; run that part in the foreground,
  and the rest as a job`. Your yes does not change this. The engine reads only the
  line: outside `auto`, a program that an approved job starts runs with your rights
  and can still do these things.
- A job's question is local only: a phone never answers it. While you are away, it
  expires, and no job starts. A `n` starts no job.
- You can move the agent's running `shell` call into a job with Ctrl+B in the turn
  view ([`docs/sandbox.md`](sandbox.md), "Moving a call to the background"). The
  engine decides the line again as a job, with the mode and the settings of the key
  press. A `Deny` refuses the move, so the kinds above never become a job this way
  either, and neither does a line with the hard `interactive` flag. A call in the
  sandbox moves with no question: it keeps its sandbox, so the move allows nothing
  new, and the key press is your answer, also in `cautious` and `manual`. A call
  outside the sandbox (an approved exit, a call in `cautious` or `manual`, or full
  mode) moves only after one `y/n` question that names the exits of the job decision.
  In `cautious` and `manual`, that `y` is your answer to the job question of the mode.
  The record of the job keeps the exits that your `y` covered. Only a terminal of
  yours can move a call, never a phone.
- A job in the exit child cannot gain rights through a setuid program
  (`no_new_privs`).
- The tools `jobs` and `job_stop` need no question in any mode for a turn of this
  machine. They read and stop only the jobs of their conversation.
- A job notice is never your input. It is not in the messages that the reviewer
  reads, and the naming rule never finds a name in it. In a turn that a notice
  started, the record says `wake_turn`, and the reviewer allows only what a message of
  yours from before that turn asks for.
- A process of a job reaches efr's socket with `read` only, also after its launcher is
  gone: it cannot answer a question, stop a job or change the settings. A process of an
  exit child job that left the job's session (`setsid`) is the exception: see
  "Known limits" in `docs/sandbox.md`.

## A command left running

A shell call answers the model at its timeout. When its command still runs then, the
command goes on in the conversation's hidden shell, and the shell takes no other call
until the command ends. [A busy hidden shell](use/hidden-shell.md#a-busy-hidden-shell)
shows what you see. The
engine and the shell keep these rules:

- A line that may ask for a password through ssh has a soft flag: `rsync` with a
  remote operand (`host:path`, `rsync://`, `host::module`) or a remote shell (`-e`,
  `--rsh`, `RSYNC_RSH`), `scp`, `sftp`, `sshfs`, `ssh-copy-id`, `mosh`, `autossh`,
  `ssh-add`, `ssh-keygen` without `-N`, `git` over the network (also past `-C`, `-c`
  and the other git options that take a value, and `remote update`, `archive
  --remote` and `send-email`), and `git` with `GIT_SSH_COMMAND`, `GIT_SSH` or
  `core.sshCommand`. The soft flag adds no question and no reason, and it does not
  refuse a job. It gives the long limit of a call outside the sandbox, and the words
  `you may need to type your password` in its question. The hard flag of `sudo`,
  `ssh` and the other programs that read the terminal does not change.
- efr records who approved each call: you, your rule (an always rule or a rule of
  your config), the reviewer, or full mode.
- While a terminal that can type your answer follows the conversation, a wait for a
  password of a command outside the sandbox stays: you answer it in the turn view or
  after `,shell`.
- While nobody can answer, such a wait stays only for a line with the hard or the soft
  flag that you or your rule approved. Every other such wait stops at once: a line
  that the reviewer or full mode let run, and a line with no flag. So a line of the
  model that fakes a `sudo` prompt never waits for you at a later prompt.
- A command in the sandbox never gets an answer that may be a password
  (docs/sandbox.md, "Input in the sandbox"). Its wait for one stops at once.
- A command left running that waits for input stops `interactive_timeout_minutes`
  after its start: efr sends SIGINT, and when the command is still there after a short
  grace, efr closes the hidden shell. The next call starts a new shell. A command that
  waits for no input, such as a slow build, has no limit.
- An answer reaches the command only from a terminal of yours, never from the phone,
  with the same checks as for a running call: the kind of the wait, the process group
  that waits and the terminal's modes.
- The next call of the conversation fails at once while the command waits for input,
  and the model reads which command holds the shell and that you answer or stop it with
  `,shell`. It never tells you to type at your own prompt.

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
  may run. After `cd -`, `popd` or `cd $DIR`, a relative path may be anywhere below `/`.
  The same is true when the line may run in more than 32 directories: each relative
  `cd` can double that number, so `cd a && cd b && ...` with six of them passes it;
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
file you can; that is why such a command asks. In full mode every command runs so,
with no question: the line cannot name the file, but a script or a program that the
line starts can write it. The config hold of "Full mode" makes such a change visible.

You change the file yourself, in your editor or with `efr config`, or you approve a
change of the settings tool. The daemon reads the links in the directory each time it
builds the engine.

## Edits with apply_patch

The model edits files with its `apply_patch` tool: one patch adds, changes, deletes
or moves one or more files. The patch changes all its files or none. When a hunk does
not match, when a write fails, or when you interrupt the turn while the patch writes,
no file stays changed.

- Every file of the patch counts as a write, both paths of a move too. The path rules
  decide them as they decide a `write_file`: a change or a new file in `$SCRATCH` or
  in the turn's registered project runs without a question in `cautious` and in
  `auto`, and a change elsewhere asks. A secret or efr's config is denied.
- A delete or a move asks in `manual` and `cautious`, from every surface, and also
  when a rule of yours allows the write. No rule can turn the question into an allow.
  The question says `delete or move files`. In `auto` and full mode, a delete or a move
  runs with no question when efr's snapshot holds each deleted or moved path and the
  destination of
  a move that exists (outside the roots, the home store copy), so `efr undo` brings it
  back. Else it asks, as in the other modes.
- The question shows the unified diff of every file of the patch. A line
  `delete <path>` or `move <from> -> <to>` comes before the diff of a delete or a move.
- `apply_patch` runs in the daemon, outside the `auto` sandbox, as `read_file` and
  `write_file` do. It refuses a path through a symbolic link and names the real path,
  and it refuses a file over 16 MiB, a binary file and anything that is not a regular
  file.

A Claude model gets the `edit` tool in place of `apply_patch`: it replaces exact text
in one file that exists, once or at every match. It never makes, deletes or moves a
file, so it never asks for that reason. Its file counts as a write and the path rules
decide it as they decide a `write_file`. The question shows the unified diff of the
file. It runs in the daemon and refuses the same paths and files as `apply_patch`. A
model never gets both tools, and a call of the tool that the model did not get does
not run.

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
  `per_call` to `keep`, `network.netview` from `false` to `true`, or a removed secret
  path.
- The tool refuses every rule that names secrets (the class `secrets`, or an `under`
  path at or below a secret location), to add and to remove. You write such rules by
  hand.
- The tool never changes `permissions.allow_full` and never sets `permissions.mode` to
  `full`. Nothing is asked or written, and the model reads that you set them in your
  own terminal (`efr config edit` or `efr config set`).
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
  - `program`: the first word, such as `cargo`. For an `allow` rule, a path such as
    `/usr/bin/cargo` is a different program. An `ask` or `deny` rule also matches the
    base name of a path, so `/usr/bin/cargo publish` meets a rule for `cargo`, and the
    command behind a wrapper (`env`, `nice`, `nohup`, `time`, `timeout`, `command`,
    `builtin`, `exec`, `stdbuf`, `ionice`, `chrt`, `taskset`, `setsid`, `flock`,
    `xargs`, `watch`, `unbuffer`, `noglob`, `nocorrect` and zsh's `-`), so `env git push` and `timeout 60 git push` meet a rule for
    `git push`. efr does not know which words a wrapper takes as values, so such a
    rule may also ask for a line where the program is only a word after the wrapper;
  - `args`: the words that must follow the program, in order. A word may list
    alternatives, as in `"status|diff"`, and may end in `*` to match every word that
    starts with it, as `"-Q*"` matches `-Qi`. For `git`, efr first skips git's own
    options before the subcommand (see "Rules for git" below);
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
A rule that matches only through a wrapper, the base name of a path or git's own
options is different: it can keep the effect of the rules before it or make it
stricter, but never less strict. So `deny` for `nice`, then `ask` for `git push`,
still denies `nice git push`.
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

### Rules for git

Before efr compares the `args` of a rule for `git`, it skips git's own options that
come before the subcommand, in the form with `=` and in the form with a separate
word: `-C <dir>`, `-c <key>=<value>`, `--git-dir`, `--work-tree`, `--namespace`,
`--config-env`, `--attr-source`, `--shallow-file`, `--exec-path=`, `--no-pager`,
`-P`, `-p`, `--paginate`, `--bare`, `--no-replace-objects`, `--no-lazy-fetch`,
`--literal-pathspecs`, `--glob-pathspecs`, `--noglob-pathspecs`,
`--icase-pathspecs`, `--no-optional-locks` and `--no-advice`. So an `ask` or `deny`
rule with `args = ["push"]` also matches `git -C ~/p/app push origin HEAD`. An
option that efr does not know counts both ways, so the rule asks rather than misses.

An `allow` rule skips only the options that change nothing: `--no-pager`, `-P`,
`--no-optional-locks`, `--no-advice`, `--no-lazy-fetch` and the pathspec options.
`-C`, `-c` and the others can point git at another repository or make it run other
code, so `git -C ~/other status` still asks under an `allow` rule for
`git status`. A rule that names such an option in its own `args`, such as
`["-C", "/home/u/p/app", "push"]`, matches that exact form. The built-in read-only
rows follow the same rule, so `git -P log` runs as `git log` does.

For an `ask` or `deny` rule with `under`, the dir of `-C` and of `--work-tree`
counts too. A `-c alias.<name>=<value>` on the line counts as the command of the
alias. When efr cannot read the alias (a shell command with `!`, a quote, or a value
from `--config-env`), an `ask` or `deny` rule for `git` matches. An alias in your git
config is not on the line: write a rule for its name too, such as `args = ["p"]`. An
`ask` or `deny` rule also reads git behind a wrapper or by its path, such as
`env git -C ~/p/app push` or `/usr/bin/git push`. A rule for another program still
compares the words right after the program.

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
