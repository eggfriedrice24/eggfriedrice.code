# The auto sandbox

In the `auto` mode, each shell command of the model runs in a kernel sandbox. A
command that stays in the sandbox runs at once, with no question. An action that
leaves the sandbox is an exit, and you answer each exit. This file tells what the
sandbox allows, what it blocks, how an exit asks, what happens when the sandbox cannot
run, and the limits that stay. [`docs/permissions.md`](permissions.md) has the rules
of the engine. [`docs/adr/0006-auto-sandbox.md`](adr/0006-auto-sandbox.md) records the
decision.

## What auto does

- A shell command of the model runs in the sandbox. Every program that it starts runs
  there too: build scripts, tests, git hooks and package install scripts.
- The sandbox does not judge the command. The kernel blocks the effects that are not
  allowed. A command that tries such an effect fails, usually with `Read-only file
  system`, `Permission denied` or `Network is unreachable`.
- efr reads the command line before it runs. When the line names an action outside
  the sandbox, such as `sudo`, `git push` or a write to `~/.zshrc`, efr asks you first.
  This is an exit.
- After a failure, the model can call the shell again with `needs`, for example a
  path to write or the network. That call is an exit too, and it asks you.
- `read_file`, `write_file` and `edit` run in efrd, not in the sandbox. In `auto` they
  follow the rules of `cautious`, plus two: a read of a sandbox mask asks, and a write
  to a floor path asks.
- The lines that you type in your own shell never run in the sandbox.
- A turn from the phone runs as `cautious`, as before.
- `nested_shell` is not available in `auto`, because no shell outlives a contained
  call. A command for `sudo` or `ssh` is an exit instead.

## The envelope

The sandbox has three layers: bubblewrap (namespaces and mounts), Landlock (ABI 9 or
higher, file and socket rights) and seccomp (a list of system calls that fail).

| What | In the sandbox |
|---|---|
| Writes | only in the write roots (below). Everything else is read-only. |
| Secrets | the paths of the read masks read as empty. Secret-like environment variables are removed. |
| Network | none. Only loopback works, so a test can start a server and talk to it. |
| Sockets and D-Bus | no connection to a socket that the call did not make: no `docker.sock`, no session bus, no system bus, no `ssh-agent`, no desktop sockets. |
| More rights | none. `sudo` fails with `The "no new privileges" flag is set`. No new user namespaces. |
| Other processes | no signals to your processes; no `/proc/<pid>/environ`, `fd` or `root` of them. `ps` still shows them. |
| Your terminal | the call keeps its terminal. It cannot type into your shell (`TIOCSTI` fails), and efr does not apply its terminal marks. |
| Background jobs | they stop when the call ends. Start a server and its test in one command. |
| `/tmp` | private to the conversation; you do not see it. Share files through `$SCRATCH`. |

Some tools show the sandbox's view: `tty` prints `/dev/console`, `df` shows the
sandbox's mounts, and `$$` differs from `/proc/self`.

## Write roots

| Root | When |
|---|---|
| the turn's project | when the turn runs in a registered project |
| registered projects that the command names | per call: a path or a `cd` target of the line lies in the project (`sandbox.write_projects = "named"`, the default) |
| the git dir and the common dir of a worktree or submodule project | from the record that efrd makes when you register the project |
| `$SCRATCH` of the conversation | always |
| the private `/tmp` and `/var/tmp` | always |
| private tool caches | each cache of `sandbox.caches` that exists, as an overlay |
| `sandbox.write_roots` | always; floors still apply |
| a grant | exactly the path of an approved exit, for one call |

- Your home directory is never a write root. A turn in a project at `~` runs as
  `cautious`, with the reason "auto cannot use your home directory as a project".
  Register a narrower project instead, such as `efr project add ~/dotfiles`.
- Write roots never come from git output, from terminal marks, or from files that the
  sandbox can write.
- A registered project counts from the next call. No restart is necessary.
- For a worktree or a submodule, `.git` is a file. efrd reads it when you run
  `efr project add` and keeps the paths. When the file changes later, calls run
  without the git dir roots, and the tool result says: run `efr project add` again.

## Tool caches

The sandbox reads your caches (`~/.cargo`, `~/.rustup`, `~/.cache`, `~/.npm` and the
others of `sandbox.caches`) through an overlay. Its writes go to a private layer.
Your own builds never see them. `bin/`, `config.toml` and `env` of `~/.cargo` and
`settings.toml` of `~/.rustup` stay read-only.

`sandbox.cache_mode` selects the mode: `tmp` (the default: writes go away after each
call), `overlay` (writes stay in a layer of the conversation) or `readonly`. `tmp` is
the default because the `overlay` mode failed the launch gate of phase 1: with all
the cache overlays, a call cost more than 10 ms at p95. The kernel writes an
`overlay` layer to the disk at the end of each call, and that costs time. efrd
deletes the layers of a conversation after `sandbox.cache_days` days without a call,
and the oldest layers first when all layers pass `sandbox.cache_max_gib`. Bin
directories on your `PATH` are never an overlay; they stay read-only.

efr mounts the cache overlays itself, not bubblewrap, because bubblewrap cannot set
the options of an overlay:

1. bubblewrap makes the sandbox with every mask and every read-only path, but with no
   overlay. Then the call waits.
2. The helper `efr-sbx layers` enters the namespaces of the call. It mounts each
   overlay with `index=off` and `xino=off`, and it moves the masks and read-only paths
   inside the cache onto the overlay, so they still apply.
3. The call starts. When the helper fails, the call does not run, and the model gets
   the reason.

With these options the kernel logs nothing for an overlay. Without them, the kernel
logged two lines for each cache in each call, and an upper layer stayed busy for some
milliseconds after a call, so the next call of the conversation could fail to start.

## Read masks

A masked directory reads as empty, and a masked file reads as an empty file that a
call cannot write. A mask covers the real target of a link too.

| Group | Paths |
|---|---|
| secrets | the engine's secret paths: `~/.ssh`, `~/.gnupg`, password stores, `~/.netrc`, cloud and registry credentials, and `permissions.secret_paths` |
| more credential stores | `~/.aws`, `~/.kube`, `~/.docker`, `~/.codex`, password managers and other token files |
| browser and messenger profiles | Firefox, Chromium, Chrome, Brave, Signal, Slack, Discord and others |
| shell histories | `~/.zsh_history`, `$HISTFILE`, `~/.bash_history` and others |
| efr's own state | efr's data, state and runtime roots; only the conversation's `$SCRATCH` comes back |
| your runtime directory | `$XDG_RUNTIME_DIR`: every socket and the session bus |
| project `.env` files | `.env` and `.env.*` in each write root, to depth 3 (`sandbox.mask_globs`); not `.env.example`, `.env.sample` or `.env.template` |
| your additions | `sandbox.mask` |

A read of a mask is a `masked_read` exit. Only you can approve it, because the content
then goes to the model provider. A secret of the engine never opens.

## The floor

A floor path stays read-only, also inside a write root and after a grant. A mask wins
over a floor at the same path.

| Floor | Paths |
|---|---|
| efr's config | the config directory and every link target in it |
| shell startup | `.zshenv`, `.zprofile`, `.zshrc`, `.zlogin`, `.zlogout` (also in `$ZDOTDIR`), `.bashrc`, `.bash_profile`, `.profile`, the fish config, `environment.d`, `.pam_environment`, `.xprofile`, `.xinitrc` |
| autostart and services | `~/.config/systemd`, `~/.config/autostart`, `~/.local/share/systemd`, `~/.local/share/applications`, the Hyprland, sway and i3 configs |
| programs on `PATH` | each directory of the hidden shell's `PATH` that lies in a write root |
| tool configs that run code | `~/.gitconfig`, `~/.config/git`, `~/.cargo/config.toml`, `~/.cargo/bin`, `~/.local/bin`, `~/.config/nvim`, `~/.vimrc`, `~/.config/direnv` |
| dotfile link targets | the target of each link in `~`, `~/.config` and `~/.local/bin` that lies in a write root |
| efr itself | `efrd`, `efr`, `efr-sbx` |
| the git surface | below |
| agent and editor configs in each write root | `.mcp.json`, `.claude/`, `.codex/`, `.agents/`, `.cursor/`, `.opencode/`, `opencode.json`, `opencode.jsonc`, `.vscode/`, `.envrc`, `.direnv/`, `.efr/` |
| your additions | `sandbox.protect` |

A write to a floor path is a `persistence` exit. Only you can approve it, and it runs
outside the sandbox.

## Git settings

Git runs programs that a repository's config names (`core.fsmonitor`, hooks, filters).
So the sandbox protects the git dir of each project:

- `.git` is a mount point: a call cannot rename or remove it.
- `config`, `config.worktree` and `hooks/` are read-only, also in `modules/*` and
  `worktrees/*`, and so are the files that `include.path` names and the directory that
  `core.hooksPath` names inside a write root.
- `git add`, `git commit`, `git stash`, new branches and a local `git fetch` work.
  `git config` fails.

A call can still add new files to a git dir, such as `commondir`. After each call, the
surface guard checks the git dirs. It moves a planted setting that runs programs to
quarantine, where nothing reads it, and the turn asks you before its next call:

```
question: the last command changed git settings that run programs
  ~/p/app/.git/commondir (core.fsmonitor); moved to quarantine
keep it? y = yes, n = no
```

`y` moves the change back; `n` leaves it in quarantine. When nobody answers, or the
turn is interrupted, the change stays in quarantine. This question is not an approval
of a tool call: the call already ended.

## Code that runs later

The model can write a `Makefile`, a `build.rs`, a `package.json` script or a project
`.cargo/config.toml`. Such code runs when you run the project yourself, outside the
sandbox. efr cannot judge it. At the end of an `auto` turn that changed such files,
efr names them:

```
efr: this turn changed files that run code later outside the sandbox:
  build.rs, package.json (scripts), .envrc, sub/.git/config (core.fsmonitor)
  check them before you run the project yourself
```

`sandbox.surface_files` lists the patterns. A changed `.cargo/config*` names the keys
that run programs, such as `build.rustc-wrapper`.

## What returns to the hidden shell

Only two things return from a call to the conversation's hidden shell:

- the directory of a `cd`, unless it is masked or in the private `/tmp`;
- exported variables that `sandbox.promote_env` lists, such as `RUST_LOG`. A name such
  as `PATH`, `LD_PRELOAD` or `GIT_DIR` never returns. A value with a relative path,
  such as `node_modules/.bin` or `Europe/Paris`, or with a path into a place that the
  sandbox can write, does not return either.

Functions, aliases and other exports stay in the sandbox's own state. The next
contained call of the conversation sees them, so `source .venv/bin/activate` works
across calls. An exit that runs outside the sandbox does not see them.

## Exits

| Kind | Example | After a yes | Who can approve |
|---|---|---|---|
| `write` | `cp report.pdf ~/Documents/` | the sandbox, with that one path writable | you |
| `host` | `curl -LO https://example.com/x.tar.gz`, `npm ci` | the sandbox, with full network for this call | you |
| `host_view` | `ss -tlnp`, `ip addr` | the sandbox, with the host's network | you |
| `socket` | a tool's own Unix socket | the sandbox, with that one socket | you |
| `desktop_ipc` | `xrandr`, `swaymsg`, `hyprctl` | the sandbox, with that one socket | only you |
| `bus` | `hostnamectl`, `systemctl --failed` | the sandbox, with the bus socket | you |
| `device` | `nvme smart-log /dev/nvme0n1` | the sandbox, with that one device | you |
| `masked_read` | read a project `.env` | the sandbox, with that one mask removed | only you |
| `destructive` | `git reset --hard`, `git clean -fd`, `rm -r` of tracked files | the same sandbox | you |
| `outside` | the model asks to run outside, such as `gdb` or git over ssh | outside the sandbox | you |
| `privilege` | `sudo`, `doas`, `pkexec`, `yay`, `docker`, `podman` | outside the sandbox, and you can type your password | only you |
| `persistence` | `~/.zshrc`, `crontab`, `systemctl --user enable`, a floor path | outside the sandbox | only you |
| `upload` | `git push`, `npm publish`, `cargo publish`, `curl -d` | outside the sandbox | only you |
| `synced_write` | `~/Dropbox` and the others of `sandbox.synced_dirs` | outside the sandbox | only you |
| `above_root` | `rm -rf ..`, `rm -rf ~/.config` | outside the sandbox | only you |
| `secret` | a read or write of a secret | never: denied | nobody |
| `config` | a write of efr's config | never: denied | nobody |

A grant lives for one call. The next call asks again.

An exit that runs outside the sandbox runs with your full rights: your files, your
secrets and the network. So efr accepts only a narrow line for it. One simple command
carries the exit, and every other command of the line must be a read-only helper that
`cautious` allows, such as `echo`. `sudo -v && ./helper` gets no question; the model
reads "an approved command outside the sandbox must run alone; run the other parts in
a separate call". The processes of such a run stop when the call ends.

The question shows the whole line, what leaves the sandbox, and how the call runs:

```
approval needed: shell: run "sudo ./scripts/setup.sh"
runs outside the sandbox: sudo (you may need to type your password)
the whole line runs with your full rights (files, secrets, network)
programs: sudo /usr/bin/sudo; ./scripts/setup.sh ~/p/app/scripts/setup.sh in a write root, changed this turn (untrusted: written in the sandbox)
efr: only you can allow this
allow? y = yes, n = no
```

```
approval needed: shell: run "npm ci"
leaves the sandbox: network; runs in the sandbox with full network for this call
allow? y = yes, n = no
```

- A command of several lines shows each line on its own, numbered:

  ```
  approval needed: shell: run 2 lines:
    1  systemctl --failed --no-pager
    2  journalctl -b -n 20 --no-pager
  ```

  The `shell:` line of a running call shows only the first line and how many follow,
  such as `shell: cd src (and 3 more lines)`.
- `programs:` names every program word with the path it runs. A word that the shell
  runs itself, such as `:` or `cd`, shows as `(builtin)`. A program in a write
  root, or one that changed in this turn, is marked `untrusted: written in the
  sandbox`: the sandbox wrote it, so it can do anything with your rights.
- Lines that start with `efr:` are facts that efr found itself.
- `the model says:` is the model's reason from `needs`. It is the model's text; treat
  it as a claim, not as a fact.
- A refused exit does not run. The model reads that you denied it.
- A floor refuses a call with no question, and the call's line says why, such as
  `shell refused: efr's config (floor)`.
- Three refusals in a row without a person (by a floor) stop the turn.

`efr history --verbose` shows the record of each exit: the line, its targets, hosts and
programs, and how it was judged. Without a conversation id, it shows the newest
conversation of this terminal, else the newest of all, and says which.

## When the sandbox cannot run

At start, efrd runs a probe: one real launch with a self-test inside. It runs the probe
again after a reload that changes `[sandbox]`, after a launch fails, and before an
`auto` turn when the last probe failed. So a fix needs no restart.

`auto` needs every part. When one part is missing, an `auto` turn runs as `cautious`.
There is no partial sandbox. A contained call that fails to start returns an error to
the model; efr never runs it outside the sandbox.

What you see:

- `,mode auto` says at once: `efr: auto needs the sandbox; turns run as cautious:
  Landlock ABI 6 found; auto needs 9 (Linux 7.1). efr sandbox check shows more.`
- Each turn that asked for `auto` starts with a dim line: `auto is not available here;
  this turn runs as cautious: <reason>`.
- `efr status` shows the `sandbox` line always: `ready (...)` or `unavailable:
  <reason>; auto runs as cautious`, with the fix.

| Failure | Reason | Fix |
|---|---|---|
| no bubblewrap | `bubblewrap is not installed` | Arch: `pacman -S bubblewrap`; Debian and Ubuntu: `apt install bubblewrap`; Fedora: `dnf install bubblewrap` |
| old bubblewrap | `bubblewrap 0.6.2 found; it lacks --bind-fd` | update the package |
| setuid bubblewrap | `bubblewrap is setuid; efr needs the unprivileged build` | install the build that is not setuid |
| user namespaces off | `unprivileged user namespaces are off (kernel.unprivileged_userns_clone = 0)` | `sysctl kernel.unprivileged_userns_clone=1` (Debian kernels) |
| AppArmor | `AppArmor blocks user namespaces for bwrap (kernel.apparmor_restrict_unprivileged_userns = 1)` | an AppArmor profile for bwrap with `userns,`, or set the sysctl to 0 |
| Landlock off | `Landlock is not enabled` | add `landlock` to the `lsm=` boot parameter |
| Landlock too old | `Landlock ABI N found; auto needs 9 (Linux 7.1)` | a newer kernel |
| kernel without the fix | `the kernel lacks the Landlock fix for disconnected directories` | a newer kernel |
| launcher in a project | `efr-sbx lies in a writable project (target/debug)` | install efr, or run efrd from outside the project |
| no launcher | `efr-sbx is not installed next to efrd` | install `efr-sbx` next to `efrd` or in `../lib/efr/` (`just install` puts it in `~/.local/lib/efr/`) |
| launcher changed | `the launcher copy does not match the installed efr-sbx` | restart efrd, so it copies the launcher again |
| probe without a report | `the sandbox launcher's probe failed: ...` | run `efr sandbox check`; reinstall efr when it fails again |
| project at `~` | `auto cannot use your home directory as a project` (per turn) | register a narrower project, such as `efr project add ~/dotfiles` |
| a dir below `/tmp` | `efr's state dir /tmp/... lies below /tmp or /var/tmp, which the sandbox replaces with its own` (also for `XDG_RUNTIME_DIR`) | keep efr's state dir (`EFR_STATE_DIR` or `EFR_HOME`) and `XDG_RUNTIME_DIR` outside `/tmp` and `/var/tmp` |
| self-test | `the sandbox let a test through: ...` | file an issue; `efr sandbox check` has the details |
| turned off | `sandbox.enabled = false` | set `sandbox.enabled = true` |

What to expect on some systems (the efr project has tested only the first row):

| System | Kernel and Landlock | User namespaces | Result |
|---|---|---|---|
| Arch Linux | 7.2, ABI 10 | allowed; bubblewrap 0.13.0, not setuid | ready |
| Ubuntu 24.04 LTS | 6.8, ABI 4 | AppArmor restricts them by default | unavailable: the Landlock ABI |
| Ubuntu 26.04 LTS | not known yet | as 24.04 | unavailable unless the kernel is 7.1 or newer |
| Debian 13 | 6.12, ABI 6 | allowed | unavailable: the Landlock ABI |
| Fedora | needs 7.1 or newer and `landlock` in the LSM list | allowed by default | ready only on 7.1 or newer |
| WSL1 | no Landlock | - | unavailable |

## Commands

`efr sandbox check` runs the probe now and prints each check:

```
$ efr sandbox check
ok    sandbox.enabled = true
ok    launcher copy /run/user/1000/efr/bin/efr-sbx (from /home/u/.local/lib/efr/efr-sbx, sha256 ok)
ok    platform linux x86_64
ok    Landlock ABI 10, errata 0xf
ok    bubblewrap 0.13.0 at /usr/bin/bwrap, not setuid
ok    launcher /run/user/1000/efr/bin/efr-sbx
ok    zsh /usr/bin/zsh
ok    every PATH entry is absolute
ok    user namespaces: the probe sandbox starts
ok    self-test: 16 checks passed
warn  ~/dotfiles/bin is on PATH and inside the registered project ~/dotfiles; it stays read-only in the sandbox
warn  the registered project ~ is your home directory; auto runs its turns as cautious
launch 3.6 ms
auto: ready
```

A failed check has a `fix:` line below it. The command exits with 1 when the sandbox
is not available.

`efr sandbox explain PATH` tells what a contained command can do with one path, from
the current directory:

```
$ efr sandbox explain ~/.zshrc
~/.zshrc, from ~/p/eggfriedrice.code in auto:
  read   yes
  write  no: a shell startup file (floor); a write is a persistence exit, user only
```

`efr paths` shows the launcher (`$XDG_RUNTIME_DIR/efr/bin/efr-sbx`, copied from the
installed `efr-sbx` and checked by SHA-256), bubblewrap, and the sandbox's state and
runtime directories. [`docs/storage.md`](storage.md) lists the files. efrd finds the
installed launcher next to its own program, else in `../lib/efr/` (so
`~/.local/lib/efr/efr-sbx` for `~/.local/bin/efrd`); it is never on `PATH`.

efrd runs the probe at start, after a change of `[sandbox]` or of the projects, after
a call whose sandbox could not start, and before an `auto` prompt while the last probe
failed. So a fix such as a new package or a sysctl needs no restart. When the result
changes to unavailable, the event log records it once (`sandbox_unavailable`).

A process that an approved command leaves behind, such as a double-forked one, can
still reach efrd's socket, because it runs outside the sandbox. efrd gives a process
that descends from a hidden shell, or that shares a hidden shell's session, only the
`read` scope: it cannot send a prompt, approve, answer input, change the config or
register a project.

## Settings

The `[sandbox]` table of `config.toml` holds every key; [`docs/config.md`](config.md)
lists them with their defaults. Each key applies from the next call or turn. A change
of efr's settings through the settings tool still asks, in every mode.

## Known limits

| Limit | What can happen | What efr does |
|---|---|---|
| a bug in the kernel, bubblewrap or overlayfs | an escape | three layers; bubblewrap from your distribution; the probe at every start |
| code that the model writes and you run later (a `Makefile`, a `build.rs`, a new dependency, a project `.cargo/config.toml`) | that code runs with your rights | the turn-end list of such files; `efr sandbox check` warnings |
| a hard link that existed before, from a secret or an rc file into a write root | a write in the project changes the file outside | `efr sandbox check` warns about such files |
| command lines of other processes | the sandbox reads them, as `ps` does in `cautious` | none; do not put secrets in command lines |
| a fork bomb or a full disk | the machine slows down or fills up | none yet |
| a line that you approved in another mode changes the hidden shell | that line has your rights | `auto` calls never run model text in the hidden shell itself; the wrapper check catches a changed wrapper |
| the private `/tmp` | wrong paths for you or the model | the prompt text says so; share files through `$SCRATCH` |
| a large rc file | slower calls | efr compiles large snapshots of your shell |
| the surface guard checks known places only | a deep nested repository with hooks shows only at the end of the turn | the turn-end check and list |
| a `bus` or `host` grant opens more than the line needs | the approved call can use the whole bus or the whole network | the question says so |
| `df`, `findmnt`, `tty` and `$$` show the sandbox's view | confusing output for system work | the prompt text says so; `host_view` exits for `ss` and `ip` |
| an approved exit outside the sandbox has your full rights | it can read secrets, use the network, or start a service through `systemd-run --user` that the call end cannot stop | one command per exit; the question shows the whole line, each program and the untrusted mark; privilege, upload and persistence are yours only |
| changes to your caches while an overlay uses them | old or wrong cache content in the sandbox | `sandbox.cache_mode = "tmp"` |
| no network | a build that must download fails | the network is an exit; `sandbox.offline_hints` tells package managers to use the cache |
| MCP servers | they run outside the sandbox | none yet |
