# The sandbox

<!-- sources: README.md "The auto mode"; docs/sandbox.md "What auto does", "The envelope", "Write roots", "Tool caches", "Read masks", "Exits", "Always-allow rules", "When the sandbox cannot run", "Commands"; docs/config.md [sandbox], [classifier]; crates/efr-cli/src/commands/sandbox.rs; crates/efr-cli/src/format/sandbox.rs, card.rs; crates/efr-daemon/src/sandbox/explain.rs, probe.rs; crates/efr-sbx/src/self_test.rs; docs/adr/0006-auto-sandbox.md, 0011-netview-and-mount-view.md, 0013-bus-proxy-and-always-allow.md -->

In the `auto` mode, each shell command of the agent runs at once in a kernel sandbox.
The sandbox has three layers: bubblewrap (namespaces and mounts), Landlock (file and
socket rights) and seccomp (system calls that fail). It holds the command and every
program that the command starts: build scripts, tests, git hooks and package install
scripts.

The sandbox does not judge the command. The kernel blocks the effects that are not
allowed. A command that tries such an effect fails, usually with `Read-only file
system`, `Permission denied` or `Network is unreachable`.

## Before you start

1. Check that the sandbox can run on your machine:

   ```sh
   efr sandbox check
   ```

   `auto` needs Linux 7.1 or newer (Landlock ABI 9) and bubblewrap 0.10.0 or newer,
   not setuid. The last line says `auto: ready` when every check passed.
2. Register the project, so the sandbox can write it:

   ```sh
   cd ~/p/app
   efr project add
   ```

   Without a registered project, the agent writes only in `$SCRATCH`.
3. Choose the mode: `,mode auto` for this terminal, or `efr config set
   permissions.mode auto` for every terminal.

## What a command can do

| What | In the sandbox |
|---|---|
| Writes | only in the write roots (below). Everything else is read-only. |
| Reads | as for you, except secrets and the read masks (below). |
| Network | the hosts of your allow list through efr's network proxy. See [The network proxy](network.md). Loopback works, so a test can start a server and talk to it. |
| Sockets and D-Bus | no connection to a socket that the call did not make: no `docker.sock`, no session bus, no `ssh-agent`. Read-only system queries get a filtered bus. |
| More rights | none. `sudo` fails with `The "no new privileges" flag is set`. |
| Other processes | no signals to your processes. `ps` still shows them. |
| Background processes | `&`, `nohup` and `setsid` stop when the call ends. A server that must run on goes in a [background job](jobs.md). |
| `/tmp` | private to the conversation. Share files through `$SCRATCH`. |

The lines that you type in your own shell never run in the sandbox.

### Write roots

A command can write here:

- the turn's project, when the turn runs in a registered project;
- the registered projects that the command names, such as a path or a `cd` target of
  the line (`sandbox.write_projects = "named"`, the default);
- `$SCRATCH` of the conversation;
- the private `/tmp` and `/var/tmp`;
- private copies of the tool caches;
- the paths of `sandbox.write_roots`;
- exactly the path of an approved exit, for one call.

Your home directory is never a write root. A turn in a project at `~` runs as
`cautious`, with the reason `auto cannot use your home directory as a project`.
Register a narrower project, such as `efr project add ~/dotfiles`.

Some paths stay read-only also inside a write root: your shell startup files, efr's
config, and the project's git config and hooks.

### Tool caches

The sandbox reads your tool caches (`~/.cargo`, `~/.rustup`, `~/.cache`, `~/.npm`
and the others of `sandbox.caches`) through an overlay. Its writes go to a private
layer, and your own builds never see them. With `sandbox.cache_mode = "tmp"` (the
default), the writes go away after each call. `sandbox.keep_downloads = true` keeps the
downloads of a conversation, so a second `cargo build` does not download the crates
again. Each call then takes about 3 to 4 ms more.

### Read masks

A masked directory reads as empty, and a masked file reads as an empty file. The masks
hold secrets (`~/.ssh`, `~/.gnupg`, password stores, cloud and registry credentials),
browser and messenger profiles, shell histories, efr's own state, your runtime
directory, and the `.env` files of each write root. Secret-like environment variables,
such as names with `API_KEY`, are removed. `sandbox.mask` adds your own paths.

## Exits

An action that leaves the sandbox is an exit. Examples are a write outside the write
roots, a host that is not on your allow list, a socket, a device, a masked file such
as a project `.env`, `sudo`, `git push` and a change that runs later, such as
`~/.zshrc` or `crontab`.

Your request is the permission. A reviewer model decides most exits from your
messages, so you see few questions:

- It allows an action that fits what you asked.
- It refuses an action that nothing you said relates to.
- It hands an unsure case to you.

While it works, the status row shows `reviewing: <command>`. Its answer stays as one
dim line, such as:

```text
reviewer allowed: write ~/.local/share/app for this call (low risk; you asked to run the generator)
```

An allow opens exactly that path, host, socket or device for one call. The next call
asks again, unless an always rule covers it. After the reviewer refused 3 actions in a
row, or 10 in one turn, it pauses, and efr asks you for the rest of the turn.

Each review is one more request to your provider.
`classifier.enabled = false` asks you about every exit instead.

### What asks only you

Some exits never go to the reviewer: `sudo` and other ways to more rights, a masked
file, a synced folder such as `~/Dropbox`, a desktop command that types or starts
programs, and the whole session bus. An upload, a delete above the project or a
change that runs later asks you when no message of yours names it.

The question shows the whole line and every program:

```text
? allow outside the sandbox
│ sudo ./scripts/setup.sh
│ runs with your full rights: files, secrets, network
│ why: sudo (you may need to type your password)
│ programs: sudo, ./scripts/setup.sh ~/p/app/scripts/setup.sh (untrusted: written in the sandbox; in a write root, changed this turn)
│ efr: only you can allow this
│ y allow · n deny
```

A program that the sandbox wrote is marked `untrusted`: it can do anything with your
rights. Lines that start with `efr:` are facts that efr found itself.

### What efr refuses

A secret, a write of efr's config, of efr's own programs or of efr's own dirs is denied
with no question. The call's result says why, such as `✗ refused: efr's config
(floor)`.

### Deletes and undo

A delete in the project, such as `git reset --hard` or `rm -r build`, runs at once
when efr's snapshot holds every file that it can touch, because `efr undo` brings them
back. At the end, the turn says `changed 4 files (+12 -3) in <project>; efr undo
reverts this turn`. `shred`, `git clean -x` and a file that the snapshot left out
still go to the reviewer. See [Undo and diff](undo-and-diff.md).

### Files that run code later

When a command plants git settings that run programs, efr moves them to quarantine and
asks `keep it? y = yes, n = no` before the next command. At the end of the turn, efr
names the files that run code later outside the sandbox, such as a `Makefile` or
`build.rs`. Check them before you run the project yourself.

## Always-allow rules

At some exit questions, `a` saves a standing answer. The question shows the rule in
words, such as `y allow · n deny · a always: write under ~/notes`. After `a`, efr asks
`save rule "write under ~/notes"? y/n`, and only `y` saves the rule and runs the call.

The rule goes to `[[sandbox.allow]]` at the end of `config.toml`:

```toml
[[sandbox.allow]]
kind = "write"
under = "~/notes"
project = "~/p/app"
```

There are three kinds: `host` (one host and port through the proxy), `write` (a
directory and, if you want, only in one project) and `socket` (one socket). A later call
that a rule covers runs with no question and no review, and shows a dim line such as
`allowed by your rule sandbox.allow[2]: write under ~/notes`.

A rule never covers `sudo`, an upload, a change that runs later, a masked read, the
bus or a file tool. A rule that crosses a floor stays in the file but is disabled.

```sh
efr sandbox allow           # list the rules with their index
efr sandbox allow remove 2  # remove rule 2 after a question
```

## When the sandbox cannot run

`auto` needs every part. When one part is missing, an `auto` turn runs as `cautious`.
There is no partial sandbox, and efr never runs a contained call outside the sandbox.

- `,mode auto` says at once: `efr: auto needs the sandbox; turns run as cautious:
  Landlock ABI 6 found; auto needs 9 (Linux 7.1). efr sandbox check shows more.`
- Each turn that asked for `auto` starts with a dim line, such as `auto is not
  available here; this turn runs as cautious: Landlock ABI 6 found`.
- `efr status` shows the `sandbox` line, such as `unavailable: bubblewrap is not
  installed; auto runs as cautious`.

Common causes and fixes:

| Reason | Fix |
|---|---|
| `bubblewrap is not installed` | Arch: `pacman -S bubblewrap`; Debian and Ubuntu: `apt install bubblewrap`; Fedora: `dnf install bubblewrap` |
| `bubblewrap is setuid; efr needs the unprivileged build` | install the build that is not setuid |
| `Landlock ABI N found; auto needs 9 (Linux 7.1)` | a newer kernel |
| `Landlock is not enabled` | add `landlock` to the `lsm=` boot parameter |
| `AppArmor blocks user namespaces for bwrap (...)` | an AppArmor profile for bwrap with `userns,`, or set the sysctl to 0 |
| `efr-sbx is not installed next to efrd` | install `efr-sbx` next to `efrd` or in `../lib/efr/` |
| `auto cannot use your home directory as a project` | register a narrower project |
| `sandbox.enabled = false` | set `sandbox.enabled = true` |

efrd runs its probe again after a change of `[sandbox]` or of the projects, after a
call whose sandbox could not start, and before an `auto` prompt while the last probe
failed. So a fix, such as a new package or a sysctl, needs no restart.
[The auto sandbox](../sandbox.md#when-the-sandbox-cannot-run) lists every reason and
what to expect on some systems.

## Commands

`efr sandbox check` runs the probe now and prints each check. A failed check has a
`fix:` line below it, and the command exits with 1 when the sandbox is not available.
The end of the output looks like this:

```text
ok    self-test: 16 checks passed
launch 3.6 ms
netview yes
network proxy
auto: ready
```

`efr sandbox explain PATH` tells what a contained command can do with one path, from
the current directory:

```text
$ efr sandbox explain ~/.zshrc
~/.zshrc, from ~/p/app in auto:
  read   yes
  write  no: a shell startup file (floor); a write goes to the reviewer when a message of yours names the file, and efr copies the file first, so efr undo reverts it; else you decide; a write is a persistence exit
```

At the end of a turn in which a call ran in the sandbox, one dim line says where the
sandbox could write, such as `sandbox: writes in the project, $SCRATCH, private /tmp;
network through efr's proxy`. With `network.mode = "none"`, it ends with `no network`. `efr history --verbose` shows the record of each exit and who judged it.

## Settings

| Key | Default | What it does |
|---|---|---|
| `sandbox.enabled` | `true` | `false` makes `auto` run as `cautious` |
| `sandbox.write_projects` | `"named"` | which registered projects a call may write: `turn`, `named` or `all` |
| `sandbox.write_roots` | `[]` | more write roots for every call |
| `sandbox.cache_mode` | `"tmp"` | `tmp`, `overlay` or `readonly` |
| `sandbox.keep_downloads` | `false` | keep the downloads of a conversation in the `tmp` mode |
| `sandbox.mask` | `[]` | more paths that read as empty |
| `sandbox.protect` | `[]` | more paths that stay read-only in a write root |
| `sandbox.allow` | `[]` | the always-allow rules |
| `classifier.enabled` | `true` | `false` asks you about every exit |

[config.toml reference](../config.md#sandbox) lists every key.
[The auto sandbox](../sandbox.md) has the full details, and
[ADR 0006](https://github.com/eggfriedrice24/eggfriedrice.code/blob/main/docs/adr/0006-auto-sandbox.md) records the decision.
