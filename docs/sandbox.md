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
  the sandbox, such as `sudo`, `git push` or a write to `~/.zshrc`, that action is an
  exit. A reviewer model decides most exits from your messages; the others, such as
  `sudo`, ask you (see "Exits").
- The model tries what you asked first. After a failure, it can call the shell again
  with `needs`, for example a path to write or the network. That call is an exit too.
  efr tells the model to ask for the narrowest need: a write grant for one file, the
  bus for `systemctl`, and `outside` only when nothing narrower fits.
- Reads work as they do for you, except for secrets. So `journalctl` shows in the
  sandbox the journal that you can read, with no grant.
- Read-only system queries, such as `hostnamectl`, `systemctl --failed` and
  `loginctl list-sessions`, run in the sandbox with no question. They reach the system
  or the session bus only through efr's bus filter, which lets through only read
  methods (see "The bus filter").
- An `a` (always) answer at some exit questions saves a rule, so the same exit asks
  nothing again (see "Always-allow rules").
- `read_file`, `write_file`, `edit` and `apply_patch` run in efrd, not in the sandbox.
  In `auto` they follow the rules of `cautious`, plus exits: a read of a sandbox mask,
  a write outside the project and `$SCRATCH`, and a write to a floor path are exits,
  as for a shell command.
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
| Network | with `network.mode = "proxy"`, only the hosts of your allow list, through efr's network proxy: the package registries and GitHub by default (see "The network proxy"). With `network.mode = "none"`, none. Loopback works in both modes, so a test can start a server and talk to it. A line that only reads your network, such as `ss -tln`, runs with the host's network view instead, with no network socket (see "The host's network view"). |
| Sockets and D-Bus | no connection to a socket that the call did not make: no `docker.sock`, no session bus, no system bus, no `ssh-agent`, no desktop sockets. A read-only system query gets a filtered bus (see "The bus filter"). An approved exit opens one socket for one call (see "Desktop sockets"); a socket grant never opens a bus, a service manager's socket or a desktop socket, also through a link. |
| More rights | none. `sudo` fails with `The "no new privileges" flag is set`. No new user namespaces. |
| Other processes | no signals to your processes; no `/proc/<pid>/environ`, `fd` or `root` of them. `ps` still shows them. |
| Your terminal | the call keeps its terminal. It cannot type into your shell (`TIOCSTI` fails), and efr does not apply its terminal marks. |
| Background processes | `&`, `nohup` and `setsid` stop when the call ends. Start a server and its test in one command, or start the server as a background job (see "Background jobs"). |
| `/tmp` | private to the conversation; you do not see it. Share files through `$SCRATCH`. |

Some tools show the sandbox's view: `tty` prints `/dev/console`, and `$$` differs from
`/proc/self`. `df` and `findmnt` show your real mounts, and `ss` and `ip` your real
network (see "The host's network view and mounts").

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
directories on your `PATH` are never an overlay; they stay read-only. A cache inside
another cache (`~/.cache/pip` and `~/.cache`, or `~/.cargo` as a link into `~/.cache`)
gets no overlay of its own: the outer overlay holds it, and its read-only paths still
apply. The order of `sandbox.caches` does not change this.

`sandbox.keep_downloads` (default `false`) keeps the downloads of a conversation in the
`tmp` mode: the download caches (`~/.cargo`, `~/go/pkg/mod`, `~/.npm`,
`~/.bun/install/cache` and `~/.local/share/pnpm/store`) keep the private layer of the
conversation, as in the `overlay` mode, and the other caches stay `tmp`. So with the
proxy, a second `cargo build` in the conversation does not download the crates again.
Your own caches do not change. It is off by default because it failed the launch gate:
the kernel syncs the upper layer to the disk at the end of each call, and a call then
costs about 3 to 4 ms more (about 12 ms at p95, against the gate of 10 ms). The kept
caches share the layer lock of the conversation (below) and the clean-up of
`sandbox.cache_days` and `sandbox.cache_max_gib`. The key does not loosen the sandbox:
it keeps a private layer and opens nothing of your machine.

efr mounts the cache overlays itself, not bubblewrap, because bubblewrap cannot set
the options of an overlay:

1. bubblewrap makes the sandbox with every mask and every read-only path, but with no
   overlay. Then the call waits. bubblewrap starts in `/`, not in the start dir of the
   call, so no process of bubblewrap keeps a directory below an overlay.
2. The helper `efr-sbx layers` enters the namespaces of the call. It mounts each
   overlay with `index=off` and `xino=off`, and it moves the masks and read-only paths
   inside the cache onto the overlay, so they still apply.
3. The call starts. When the helper fails, the call does not run, and the model gets
   the reason.

With these options the kernel logs nothing for an overlay. Without them, the kernel
logged two lines for each cache in each call, and an upper layer stayed busy for some
milliseconds after a call, so the next call of the conversation could fail to start.

There is a trade-off. With `index=on`, the kernel refuses a second overlay on an upper
layer that an overlay uses already. With `index=off`, it does not check this, and two
overlays on one upper layer give undefined results. So in the `overlay` mode each call
locks the layer dir of its conversation (`flock` on `$SBX/cache`) before bubblewrap
starts. It keeps the lock until its overlays are gone. A second call of the same
conversation waits for the lock. When the lock stays held for 5 seconds, the call does
not run, and the model gets the reason. The `tmp` mode needs no lock: each call has
its own layers.

**Deletes that reach the real cache.** A delete in a cache overlay deletes only in the
private layer, so `go clean -cache` or "free some disk space" would free nothing. So
after a contained call, the launcher lists the files and dirs that the call deleted
in each cache layer (the whiteouts of the layer, at most 4096 paths and 256 KiB of
names). Each list is an exit (`write`, from the source `cache_delete`) that the
reviewer decides; it needs no message that names the paths, because the tool can fill
its cache again. After an allow, efrd checks each path again against its own caches,
the pins, the masks and the secrets, and deletes the paths in your real cache. One bad
path refuses the whole list. The call then shows the freed space, such as `freed 1.2
GB in ~/.cache/go-build (12 paths)`. efr never deletes below a pin (`~/.cargo/bin`,
the cargo config, `~/.rustup/settings.toml`), a mask or a secret. A refused list stays
private, the model reads so, and efr does not raise the same paths again in the
conversation. A dir that the call deleted and made again, a name that is not UTF-8
and the paths over the limits only count; efr does not delete them.

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
| efr itself | `efrd`, `efr`, `efr-sbx` and its copy in `$XDG_RUNTIME_DIR/efr/bin`; efr's own dirs `$XDG_RUNTIME_DIR/efr`, `~/.local/state/efr` and `~/.local/share/efr` except `$SCRATCH` and efr's trash. A write there is a `config` exit: denied in every mode |
| the git surface | below |
| agent and editor configs in each write root | `.mcp.json`, `.claude/`, `.codex/`, `.agents/`, `.cursor/`, `.opencode/`, `opencode.json`, `opencode.jsonc`, `.vscode/`, `.envrc`, `.direnv/`, `.efr/` |
| your additions | `sandbox.protect` |

A write to a floor path is a `persistence` exit, and it runs outside the sandbox (in
efrd for a file tool). For a floor FILE of the built-in groups above (shell startup,
autostart and services, programs on `PATH`, tool configs, dotfile link targets, agent
and editor configs), the reviewer decides when a message of yours names the file,
such as "add an alias to my zshrc" or "change the gap in my hyprland config". A
program on `PATH`, a unit, a desktop entry and an agent or editor config have common
names, so their bare name names nothing: write the path (`~/.local/bin/git`), the name
with its suffix (`backup.service`, `tasks.json`) or, for a unit, "the backup unit". A
path names a file only as whole parts of the path: `bin/git` does not name
`src/bin/github.rs`. A link target takes the group and the names of its link. A
`write_file`, `edit` or `apply_patch` of `~/.zshrc` that links into a dotfiles dir also
writes the real file there; it takes the names of the link too, so "my zshrc" names
both, also when the dotfiles dir is no registered project. Before the write, efr copies the
file into its home store (below). Only you approve a dir, a link, the git surface and
`sandbox.protect`, and a floor file that no message of yours names, except the named
changes of units, cron and linger below.

**Named changes of units, cron and linger.** These change no file that the line names,
or they install the bytes of another file. The reviewer decides one only when a message
of yours names it, efr can copy what it changes first, so `efr undo` brings it back,
and the reviewer can read what it installs:

| Form | The reviewer decides when | efr copies first |
|---|---|---|
| `systemctl --user enable`, `reenable`, `preset`, `mask`, `add-wants`, `add-requires` of a unit by name | a message names the unit, and its unit file was in a unit dir before the turn, outside every write root, unchanged in the turn (also a link to it) and at most 8 KiB; its text and the text of each unit of its `Also=` go to the reviewer | the whole `~/.config/systemd/user` within the dir limits, and each link that the unit's `[Install]` section names |
| `ln -s <src>` into a `*.wants` or `*.requires` dir; `cp <src>` into a floor dir such as `~/.config/autostart/` | a message names the target, and `<src>` holds the rule of a unit file; its text goes to the reviewer | the target, or the dir with what it holds |
| `crontab <file>`, `crontab -` fed a literal by `echo` or `printf`, `crontab -r` | a message names cron or the crontab; the file holds the rule of a unit file | the text of the crontab (`crontab -l`) |
| `loginctl enable-linger`, `loginctl disable-linger`, with no user or with your own user (by name or uid) | a message names linger | the linger setting |

Only you approve `systemctl --user link`, `edit`, `revert`, `enable <path>`,
`set-environment`, `set-default`, `preset-all`, a unit that was not in a unit dir before
the turn, a template unit (`foo@.service`), `crontab -e`, `crontab -` with any other
input, `at`, `batch` and `systemd-run --user`. efr reads and sets the crontab and linger
only with a `crontab` and a `loginctl` that it found when it started, in a dir that
root owns and only root can write, outside your home dir and efr's dirs. With no such
program, these forms stay with you. `crontab -l` only reads your crontab: it is an
`outside` exit, so the reviewer can let the model read the crontab before it writes all
of it again with `crontab -`. `efr undo` compares the crontab or the linger
setting with the one after the turn: when you changed it after the turn, undo leaves it
as it is. After undo removes a unit link, run `systemctl --user daemon-reload`; a unit
that started stays running.

**Copies before a write outside the roots.** In `auto`, before efrd runs an approved
write outside the project and `$SCRATCH`, it copies each file that the call can change
into its home store, `~/.local/share/efr/snapshots/home.git`. This is so for a write
grant, `write_file`, `edit` and `apply_patch`, a floor file edit, an `above_root` or a
directory delete, an `outside` line that declares its write paths, and a delete in a
real tool cache. It happens for every approver: the reviewer or you. After the call,
efrd records the state after the call too, so `efr undo` can tell a later change of
yours from the call's change.

- efrd reads each file through its own file access, with `RESOLVE_NO_SYMLINKS` and
  `O_NOFOLLOW`, and streams the bytes into the store. git never reads a home path. A
  link stays a link, and the real file behind it gets its own copy.
- A write grant that opens the whole parent dir of a file (the normal case for a file
  that exists) can change any file there. efrd copies the dir too, while it holds at
  most `snapshot.home_max_files` files (2000) and `snapshot.home_max_mib` MiB (256).
  After the call it lists the dir again, and a new file in it is "missing" before.
  Over the limits it copies the target only, and the turn says `not covered: <dir>
  (over the copy limits)`. A file in the dir that no copy holds, such as a file over
  16 MiB, shows only when the call changed it.
- A copy holds at most 16 MiB per file. efrd never copies a mask or a secret. A masked
  file that a message of yours names, such as `~/.aws/config`, runs with no copy, and
  the turn says `changed ~/.aws/config; efr undo cannot revert it (masked)`.
- When efrd cannot read a file, or cannot copy a floor or persistence target, the call
  does not run, and the model gets the reason.
- In `manual` and `cautious`, a file tool write outside the roots gets no copy.
- The turn ends with one dim line for each changed floor file, such as `changed
  ~/.zshrc; efr undo reverts it`, and the line of the turn counts the other files,
  such as `changed 1 file in ~; efr undo reverts this turn`. A file behind a dotfile
  link keeps the path through the link and names its real file too.
- The collector keeps the copies of the newest `snapshot.keep_turns` turns of each
  conversation, for at most `snapshot.max_age_days` days. efr writes nothing new to
  `~/.local/share/efr/file-backups/` (phase A), and deletes the old copies there after
  `snapshot.max_age_days` days.

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
| `write` | `cp report.pdf ~/Documents/`, `write_file ~/notes.md`, a delete in a real cache | the sandbox, with that one path writable | the reviewer, else you |
| `host` | with the proxy: `curl -LO https://example.com/x.tar.gz` for a host that is not on the allow list; `nslookup`, `dig`, `ping`, `nc`; `needs.hosts` of `localhost:5173`. Without the proxy: every network need, such as `npm ci` | the sandbox, with that one host and port through the proxy; or with the whole network for a raw network program, `localhost`, a private host, another port, and every network need without the proxy | the reviewer, else you |
| `host_view` | `iw dev`, `ifconfig`, `ip link set lo mtu 1400`, `ss -tln; make test`, and `ss` or `ip` while the network view is off | the sandbox, with the host's network | the reviewer, else you |
| `socket` | a tool's own Unix socket | the sandbox, with that one socket | the reviewer, else you; only you for a credential agent's socket (`$SSH_AUTH_SOCK`, gpg-agent, a keyring, a password manager's agent); refused inside a secret or efr's own dirs. A service manager's socket, such as `$XDG_RUNTIME_DIR/systemd/private`, is a `privilege` exit. A socket grant to a bus, a service manager's socket or a desktop socket, also through a link, makes the call fail to start |
| `desktop_ipc` | `hyprctl clients`, `hyprctl reload`, `swaymsg -t get_tree`, `i3-msg -t get_tree`; `hyprctl dispatch exec`, `xdotool type` | the sandbox, with the one control socket of that desktop and its variable | the reviewer for a read or reload form; only you for the rest, and for a desktop socket that the model names |
| `bus` | `notify-send`, `busctl`, `resolvectl`, `systemctl get-default`, `nmcli device status`, `iwctl station wlan0 show`; `nmcli radio wifi off`; `hostnamectl` and `systemctl --failed` only while the bus filter is off (see "The bus filter"); `needs.bus` | the sandbox, with the whole bus socket | the reviewer, else you; only you for a form of `nmcli` or `iwctl` that changes something, because the bus filter has no rules for NetworkManager and iwd |
| `device` | `nvme smart-log /dev/nvme0n1` | the sandbox, with that one device | the reviewer, else you |
| `masked_read` | read a project `.env` | the sandbox, with that one mask removed | only you |
| `destructive` | `git reset --hard`, `git clean -fd`, `rm -r` of tracked files | the same sandbox | in `auto`, nobody when efr's snapshot holds every file that the part can change ([Undo](#undo)); else the reviewer when every target lies in a write root; else only you |
| `outside` | `crontab -l`, `df` and `findmnt` while the mount view is off or behind a path or a wrapper, `rustup toolchain install stable` and the other `rustup` installs, or the model asks to run outside, such as `gdb`; with the proxy also `ssh`, `mosh`, `autossh`, and `git fetch` or `git clone` from an ssh remote | outside the sandbox | the reviewer; only you when a program of the line changed in this turn or lies in a write root, or when the line runs an interpreter on code that efr cannot read (`python3 -c`, `sh -c`, `node build.js`). An ssh line needs a message of yours that names the host, or a remote of your project; an ssh submodule URL, a `rustup` line that sets `RUSTUP_*` or `CARGO_HOME`, names a toolchain path or a custom toolchain, or runs `rustup toolchain link` asks only you |
| `privilege` | `sudo`, `doas`, `pkexec`, `yay`, `docker`, `podman` | outside the sandbox, and you can type your password | only you |
| `persistence` | `~/.zshrc`, a Hyprland config; `crontab`, `systemctl --user enable` | outside the sandbox | the reviewer for a floor file, a unit, cron or linger that a message of yours names, after a copy into the home store ("Named changes of units, cron and linger"); else only you |
| `upload` | `git push`, `npm publish`, `cargo publish`, `curl -d` | outside the sandbox | the reviewer when a message of yours names it; else only you |
| `synced_write` | `~/Dropbox` and the others of `sandbox.synced_dirs` | outside the sandbox | only you |
| `above_root` | `rm -rf ..`, `rm -rf ~/p/old-app` | outside the sandbox | the reviewer for one path that a message of yours names and that holds no floor; else only you |
| `secret` | a read or write of a secret | never: denied | nobody |
| `config` | a write of efr's config, of efr's own programs or of efr's own dirs | never: denied | nobody |

"Else you" means that the reviewer handed the question to you, or that it is off
(`classifier.enabled = false`), paused, or failed at each step of its chain.

A grant lives for one call. The next call asks again, unless an always rule covers it
(see "Always-allow rules").

**Desktop sockets.** A `desktop_ipc` exit opens the one control socket of the desktop
that its program talks to, and sets the one variable that names it:

| Program | Desktop | Socket | Variable |
|---|---|---|---|
| `hyprctl` | Hyprland | `$XDG_RUNTIME_DIR/hypr/<signature>/.socket.sock` | `HYPRLAND_INSTANCE_SIGNATURE` |
| `swaymsg` | sway | `$SWAYSOCK`, a `sway-ipc.*.sock` in `$XDG_RUNTIME_DIR` | `SWAYSOCK` |
| `i3-msg` | i3 | `$I3SOCK`, an `ipc-socket.*` in `$XDG_RUNTIME_DIR/i3` | `I3SOCK` |
| `niri msg` | niri | `$NIRI_SOCKET`, a `niri.*.sock` in `$XDG_RUNTIME_DIR` | `NIRI_SOCKET` |
| `wlr-randr`, `wtype`, `wl-copy`, `wl-paste` | Wayland | `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY`, a `wayland-*` | `WAYLAND_DISPLAY` |
| `xrandr`, `xdotool`, `wmctrl`, `xprop`, `xdpyinfo`, `xclip`, `xsel` | X11 | `/tmp/.X11-unix/X<n>` for a local `:<n>` | `DISPLAY` |

efr reads these variables from the terminal of your prompt, because efrd's own
environment goes stale when the compositor restarts. Only when the prompt sends none
does efrd use its own environment. efrd checks the socket at each call: it must be a
socket of yours with the place and the name of the table (an X server's socket in
`/tmp/.X11-unix` can also be root's), with no link on the way. So a variable that
project tooling such as direnv sets cannot name another socket of yours, such as the
session bus or a credential agent. The call never gets another socket of the
desktop, such as Hyprland's event socket `.socket2.sock`, and never its dir or its
log. When efr finds no such socket, the call does not run, and the model reads "efr
found no Hyprland session in the terminal of this prompt". An X server that needs an
X cookie (`$XAUTHORITY` in your terminal or in efrd's environment, or
`~/.Xauthority`) is refused for now: the cookie file stays
masked. `ydotool` talks to its own daemon, so its exit opens nothing.

**The reviewer.** In `auto`, the user's request is the permission. A reviewer model
decides most exits before any question: it allows an action that fits what you asked,
refuses an action that nothing you said relates to, and hands an unsure case to you.
It reads your messages and efr's facts about the exit (the line, the targets with
their class and whether the sandbox hides them as a credential store, the hosts, the programs, the counts, and for a file tool edit of a floor
file the new text, at most 8 KiB), and never the output of a command, a project file
or the model's reason. A longer edit asks you, so the reviewer never allows what it
saw only in part. efr applies its own rules to each allow: a `critical` risk never
allows, a `high` risk needs a message of yours that clearly authorizes it, a write
needs a message that names each target unless the target is a tool cache, build
output below a registered project or efr's trash, and a host that is not on the allow
list needs a message that names it, except a package fetch of a request to build,
install or update. The text of the line never changes this. A host is on the allow
list only with the proxy, only for a grant of that one host, and only on its port; a
grant of the whole network always needs the name. efr knows a host from the line (a
URL, a remote operand, the destination of `ssh`) and, for `git fetch`, `git pull`,
`git push` and `git submodule update`, from the remotes of the repository: every
fetch and push URL behind the remote name. A remote of your registered project's own
git dir that did not change since the conversation started counts as named by you
(`project_remote`); a repository that a command made does not count. A host that the
proxy refused in the last command or a job is a fact for the reviewer
(`refused_by_proxy`), never a name. A host that a script connects to has a fact only
after the proxy refused it.

The reviewer is the small model of your provider (`classifier.model` picks another),
then the turn's model at a low effort (`classifier.fallback_to_turn_model`), then you.
A model that is missing or refuses is skipped for the rest of the conversation; a
timeout (`classifier.timeout_secs`, 20 s for each step) is tried again at the next
exit. An error never allows and never counts as a refusal. Within a turn, the same
exit gets the same verdict without a second request.

```
reviewing: ./scripts/gen.sh                                (status row)
reviewer allowed: write ~/.local/share/app for this call (low risk; you asked to run the generator)
· $ ./scripts/gen.sh
  ✓ 1.4s
```

- While it works, the status row shows `reviewing: <command>`.
- An allow or a refusal is one dim line before the call. A refused call does not run,
  and the model reads that the reviewer refused it and must not work around it.
- An unsure verdict asks you. The question then has the dim row `reviewer: unsure;
  <reason>`.
- The reason is the reviewer model's text. efr shows it safely, as it shows every text
  of a model.
- After the reviewer refused 3 actions in a row, or 10 in one turn, it pauses for the
  rest of the turn: `reviewer paused for this turn: it refused 3 actions in a row; efr
  asks you for the rest`. Each later exit asks you. An allow or your answer starts
  the count in a row again. When you are away, the turn stops instead: `auto stopped
  this turn: the reviewer refused 3 actions in a row and nobody could decide`.
- `efr history` shows each verdict, and `efr history --verbose` adds the model, the
  step of the chain, the time and the models that the chain skipped.

**When you are away.** efr counts you away from a conversation after
`presence.away_after_minutes` (10) without input, or after `efr away` or `,away`. Input
is a prompt, a steer, an answer or keys for the hidden shell; keys in the input row
count only when you send them. The status row of a running turn shows `away`. Your
next input ends it. While you are away, a question waits only `presence.away_wait_secs`
(300): `approval expired: nobody answered while you were away; the call did not run`.
A question that already waits when you leave gets the same wait, from the moment efr
sees that you are away (it looks every 15 seconds).
The model goes on without the call and names it in its answer. An expired question is
no refusal.

An exit that runs outside the sandbox runs with your full rights: your files, your
secrets and the network. So efr accepts only a narrow line for it. One simple command
carries the exit, and every other command of the line must be a read-only helper that
`cautious` allows, such as `echo`, or a `sudo -k` (also `-K`, `--reset-timestamp`)
that only forgets the cached password, so `sudo -k; sudo true` asks once.
`sudo -v && ./helper` gets no question; the model reads "an approved command outside
the sandbox must run alone; run the other parts in a separate call". The processes of
such a run stop when the call ends.

The question shows the whole line, what leaves the sandbox, and how the call runs:

```
? allow outside the sandbox
│ sudo ./scripts/setup.sh
│ runs with your full rights: files, secrets, network
│ why: sudo (you may need to type your password)
│ programs: sudo, ./scripts/setup.sh ~/p/app/scripts/setup.sh (untrusted: written in the sandbox; in a write root, changed this turn)
│ efr: only you can allow this
│ y allow · n deny
```

```
? allow full network for this call
│ nslookup example.com
│ why: network
│ y allow · n deny
```

- The title says what a "yes" allows. A command of several lines shows each line on
  its own row. A line wider than the screen goes on in the next row, so nothing of
  what runs is cut: after a space the row ends in a `\` and the next row is indented;
  inside a word the row ends in a `↩` and the word goes on at the same column.
- When you answer, the question gives its place to one line, `✓ allowed` or `✗
  denied`, and the call shows once, in its own block, also when you denied it.
- Only the facts that matter most stand out in the `warning` colour: the title, the
  full-rights line and the `untrusted` mark. What runs is in the `code` colour. The
  other facts are dim.
- `programs:` names every program word. A program in a write root, or one that changed
  in this turn, also shows the path it runs and is marked `untrusted: written in the
  sandbox`: the sandbox wrote it, so it can do anything with your rights. A word that
  runs no program shows `(not found)`.
- Lines that start with `efr:` are facts that efr found itself.
- `the model says:` is the model's reason from `needs`. It is the model's text; treat
  it as a claim, not as a fact.
- When the model asks to run outside the sandbox, but each other exit that efr finds
  in the line can run in the sandbox with a grant, a fact says so, for example `efr: a
  grant of the system bus covers what the line shows; the model asked for outside`.
  The question still asks about what the model asked for. efr reads only the line, so
  a script or a build can need more than the line shows.
- A refused exit does not run. The model reads that you denied it.
- A floor refuses a call with no question, and the call's result says why, such as
  `✗ refused: efr's config (floor)`.
- A call that fails in the sandbox ends with its exit code and its time, such as `✗
  exit 2 · 1.5s`, and no word about the sandbox: every shell call of `auto` runs
  there. A turn in which a call ran in the sandbox ends with one dim line that says
  where the sandbox can write, such as `sandbox: writes in the project, $SCRATCH,
  private /tmp; no network`. When a call of the turn ran with the proxy, the line says
  `network through efr's proxy` in place of `no network`. When a call of the turn had
  the network view, the line ends with `; host network view (no sending)`.
- Three refusals in a row without a person (by a floor) stop the turn. A refusal of
  the reviewer does not count here; it has its own pause (above).

`efr history --verbose` shows the record of each exit: the line, its targets, hosts and
programs, and how it was judged (by you, a floor or the reviewer). Without a conversation id, it shows the newest
conversation of this terminal, else the newest of all, and says which.

## The host's network view and mounts

Some system work only reads your machine: what listens on a port, the traffic
counters, how full the disks are. In the sandbox these tools would show the
sandbox's own network and mounts. So efr gives such lines a view of the host, with no
exit and no question. [`docs/adr/0011-netview-and-mount-view.md`](adr/0011-netview-and-mount-view.md)
records the decision.

### The host's network view

A line that only reads your network runs in the sandbox with `Grant::NetView`. The
call keeps your network namespace, so `ss`, `ip` and `/proc/net` show your real
sockets, links, counters and routes. The call cannot send anything:

| What | In a network view call |
|---|---|
| Sockets | seccomp allows only `AF_UNIX` and `AF_NETLINK`. Every other family fails at `socket()` with `EAFNOSUPPORT`: TCP, UDP, IPv6, packet, raw, Bluetooth, vsock and each family that a later kernel adds. So a DNS query fails too. |
| Netlink | only `NETLINK_ROUTE` (for `ip`) and `NETLINK_SOCK_DIAG` (for `ss`). Every other protocol fails with `EPROTONOSUPPORT`, so `iw` stays a `host_view` exit. Without a capability, these two protocols cannot change a link, an address or a route. |
| `listen`, `pidfd_getfd` | fail with `EPERM`, so a call cannot wait for a host program on an abstract socket name, or take a socket from another process. |
| TCP | Landlock also refuses TCP bind and connect, a second layer behind seccomp. |
| Unix sockets | as in every call: no connect to a socket path that the call did not make, and no connect or send to an abstract name outside the sandbox. |
| `ss -p` | names only the processes of the call. Landlock hides the file descriptors of every other process. |
| Package managers | get the offline hints, as in every call without the network or the proxy. |

Which lines get the view (`efr-permissions/src/exits/view.rs`): each simple command
of the line is one of these:

- `ss`, but not with `-K`, `--kill`, `-D`, `--diag`, `-N` or `--net`;
- a read form of `ip` (`addr`, `link`, `route`, `neigh`, `rule`, `maddr`,
  `addrlabel`, `netconf`, with no verb or `show`, `list`, `ls`, `lst`, `get`), but not with `-batch`,
  `-netns` or `-all`, or a prefix of them;
- `nstat`, but not with `-d` or `--scan`;
- `cat`, `head`, `tail`, `grep` or `wc` of a file below `/proc/net` (also
  `/proc/self/net` and `/proc/<pid>/net`);
- a text helper: `grep`, `head`, `tail`, `wc`, `sort`, `uniq`, `cut`, `tr`, `column`,
  `cat`, `echo`, `printf`, `date`, `sleep`, `:`, `true`, `tee`.

`watch`, `timeout` and a `while` loop around these are allowed, so `watch -n 5 ss -ti`
and `while :; do ss -ti; sleep 5; done` get the view. No interpreter (`awk`, `python`),
no `find`, `git` or `xargs`, and no `netstat`. A line that also does other work, such
as `ss -tln; make test`, keeps the `host_view` exit.

This rule only picks the lines that fit. It is not a security layer. A call with the
view can still run any code: a function or an alias named `ss` that an earlier call
defined, a zsh module such as `zsh/net/tcp`, a bug in a helper. The hidden shell
drops a function of your rc files that has the name of a program that a rule trusts,
such as `ss`, but the sandbox state of an earlier call can define one. seccomp,
Landlock and the refusal of `listen` carry the whole load. The rule keeps lines that
start local servers out of your network namespace, because such a call loses the
sandbox's private loopback.

When the line also has an exit, the launch gets the view only beside `write` and
`unmask` grants, which do not depend on the network namespace. With every other
grant, the view drops: the network grant needs sockets that the view refuses, and a
device such as `/dev/net/tun` could send frames with no `socket()` call. So
`ss -tln; curl https://x` is a `host` exit with the full network, and
`cat /proc/net/dev /dev/net/tun` is a `device` exit with no view.

`network.netview = false` turns the view off: these lines are `host_view` exits again.
The view is also off when the probe's self-test of it failed (see "When the sandbox
cannot run"). `efr status` shows `netview yes`, `netview no` with the reason, or
`netview off (network.netview = false)` when you turned the key off. A
background job keeps its view up to its time limit, also after you turn the key off or
a later probe fails: its seccomp filter is in place, and the kernel does not change
under it. `job_stop` or `efr jobs stop` ends it.

`nmcli` and `iwctl` talk to NetworkManager and iwd over the system bus, not through
netlink. They are `bus` exits: the reviewer decides a read form, such as
`nmcli device status`, `nmcli general status` or `iwctl station wlan0 show`, and only
you allow every other form, such as `nmcli radio wifi off` or
`iwctl station wlan0 connect X`. A background job with `nmcli` or `iwctl` is refused,
as every job that needs the whole bus is.

### The host's mounts

A line that runs `df` or `findmnt` by its bare name, where the shell itself finds the
program (`df -h`, `LC_ALL=C df`, `time findmnt`), gets a snapshot of your mounts with
no exit. efrd runs in your mount namespace, so it reads its own
`/proc/self/mountinfo` and measures each mount outside the sandbox. It writes two
files into the call's dir, `mounts` (the mount table) and `mounts.json` (the sizes),
and the sandbox binds them read-only next to the launcher. In the call:

- `df` with options only prints the snapshot in the format of GNU `df`. It knows `-a`,
  `-B`, `-h`, `-H`, `-i`, `-k`, `-l`, `-P`, `-T`, `-t`, `-x`, `--total`, `--output`,
  `--sync` and `--no-sync`. Any other option, `--`, or a path runs the real `df`,
  which shows the sandbox's file system: `df -h /tmp` shows the call's private `/tmp`,
  which is where a write really goes. The rows follow upstream GNU `df`, which lists
  `devtmpfs`. Some builds hide it: on Ubuntu 24.04, `df` in a call shows one more row,
  `/dev`, than `df` outside.
- `findmnt` reads the snapshot as its table (`--tab-file`). `findmnt -D` and `-I`
  print the snapshot's sizes. With an option that picks a table of its own (`-F`,
  `-N`, `-k`, `-s`, `-p`, `-x`, `-m`), it runs the real `findmnt`.
- A `df` or `findmnt` function of your shell wins over these. They exist only on the
  line itself: a script or `sh -c 'df'` shows the sandbox's mounts.
- A path or a wrapper runs the real program, which shows the sandbox's mounts. So
  `/bin/df`, `command df`, `exec df`, `timeout 10 df`, `env LC_ALL=C df`, `watch df`
  and `find -exec df` stay `outside` exits, which show the true view after a review.
- `lsblk` shows your real devices; its `MOUNTPOINTS` column shows the sandbox's.

The rules of the snapshot:

- No sizes for the dummy file systems (`proc`, `sysfs`, `cgroup2`, `autofs` and the
  others), so a `df` never mounts an automount share.
- No sizes for the mounts of another user (below `/run/user/<uid>`, or a FUSE mount
  with another `user_id`), and none for a mount at or below a mask or a secret of the
  call, such as `$XDG_RUNTIME_DIR/doc` or an encrypted store. An `unmask` grant shows
  the sizes of its path.
- One helper process, `efr-sbx statvfs`, measures every other mount, one thread per
  mount. After one second, efrd kills it and does not wait for it, so a hung NFS or
  FUSE mount never holds `df`. Such a mount shows `-`, and efrd does not measure it
  again for a minute. At most one helper runs at a time, and a snapshot serves calls
  for two seconds.
- A background job gets the snapshot of its start and keeps it.
- When the snapshot fails, the call runs with the sandbox's mounts and the model reads
  `the mount view failed; df shows the sandbox's mounts`.

At start, efrd compares its mount namespace with the one of its parent, the systemd
user manager. When they differ, for example because a unit drop-in sets
`PrivateTmp`, `ProtectHome` or `ProtectSystem`, efrd's view of the mounts is not
yours: the mount view is then off, and `df` and `findmnt` are `outside` exits, which
show the true view after a review. A test of efr fails when `systemd/efrd.service`
gets such a key.

Every contained call could already list every mount of the machine through
`/proc/<pid>/mountinfo`, also the mounts of other users and containers. A new procfs or
`hidepid` would hide them, and both hide `ps`. The mount view adds the sizes of your
own mounts.

## The network proxy

With `network.mode = "proxy"` (the default), a contained command reaches the hosts of
your allow list through efr's network proxy, with no review: the package registries and GitHub
by default. So `cargo build` with a new dependency, `npm install` and `pip install`
run at once. Every other route stays closed. With `network.mode = "none"`, a contained
command has no network, as before phase D2.
[`docs/adr/0012-network-proxy.md`](adr/0012-network-proxy.md) records the decision.

### The path of a connection

1. The call keeps its own network namespace, with only loopback. Before the command
   starts, the launcher starts a small bridge in the call, `efr-bridge`, which listens
   on `127.0.0.1:3128`.
2. efr sets the proxy variables of the call to `http://127.0.0.1:3128`: `HTTPS_PROXY`,
   `HTTP_PROXY`, `https_proxy`, `http_proxy`, `ALL_PROXY`, `all_proxy`,
   `CARGO_HTTP_PROXY`, `npm_config_proxy`, `npm_config_https_proxy`,
   `YARN_HTTP_PROXY`, `YARN_HTTPS_PROXY` and `PIP_PROXY`. `NO_PROXY` and `no_proxy`
   are `localhost,127.0.0.1,::1`, so a server that a test starts in the call stays
   direct. These variables win over a proxy in your own tool config, such as
   `~/.cargo/config.toml`. Node's `fetch` uses them only with `NODE_USE_ENV_PROXY=1`,
   which efr sets, so Node prints an "experimental" warning on stderr. curl, cargo,
   git over https, npm, pip, uv, go, bun and pnpm were tested on 2026-10-13. A tool
   that does not read these variables fails closed: it cannot resolve the host.
3. The bridge copies each connection to the call's proxy socket. efrd binds this Unix
   socket for the call (`$CALL/proxy.sock`, mode 0600), and the launcher binds it
   read-only into the call at `$XRT/efr-proxy.sock`. The command itself cannot connect
   to that socket: Landlock gives `EACCES`. Only the bridge can.
4. efrd's proxy reads the request, checks it, resolves the host, connects, checks the
   TLS server name, and then copies the bytes both ways.

The bridge starts with two plain process spawns, a starter and a server, and runs in
its own session. Before it serves, the server closes every other file descriptor,
names itself `efr-bridge`, turns off core dumps and `/proc` access to it, and applies
its own Landlock domain (no write right, a connect only to the proxy socket, no
signal out) and its own seccomp filter (only Unix sockets; no `bind`, `listen`,
`execve` or `ptrace`). Then it tells the starter that it is ready. When a step fails,
the bridge does not serve, and the call fails to start with a setup error. The bridge
adds about 0.6 ms to each call. The bridge is no job of the call's shell, so the end
of a call does not list it as a stopped background process, and the stop of a
background job spares a process with its name. The end of the call's pid namespace
ends it.

### What the proxy checks

For each `CONNECT` request, in this order:

1. The request head must come within 10 seconds and 8 KiB. A control character gives
   400.
2. An IP address as the host gives 403, unless `[[network.hosts]]` lists it.
3. When the call's upload budget is spent, a new tunnel gets 403.
4. Port 80 means plain HTTP: a tunnel to a port that only an `http` entry or a port 80
   grant allows gets 403.
5. The lists: the deny list wins, then the allow list, then the host grants of the
   call. A host that is not on a list gets 403, with no DNS lookup. So a name that is
   not listed never reaches a DNS server.
6. The proxy resolves the name (5 seconds). Every address of the name must be a
   global address: not loopback, private, shared (CGNAT), link-local, unique local,
   multicast, documentation or another special-purpose block, and not an address of
   your machine or of one of its networks. The proxy unwraps the IPv4-mapped,
   IPv4-compatible and NAT64 forms first. A `private = true` entry may resolve to an
   address of your LAN, never to loopback, link-local or your machine.
7. The proxy connects only to an address that it checked: at most 4 addresses, 10
   seconds each and 15 seconds in all.
8. After `200 Connection established`, the first bytes must be a TLS ClientHello
   (16 KiB, 10 seconds), and its server name must be the host. A hello with no server
   name, bytes that are not TLS, and a hello that asks for Encrypted Client Hello cut
   the tunnel before any byte of the client leaves.
9. Then the proxy copies the bytes, under the upload budget.

Plain `http://` works only for an http host: an entry with `http = true`, an
`http://` pacman mirror, or a host grant on port 80. It allows only `GET` and `HEAD`.
The proxy writes a new request head with a fixed list of headers, and reads no byte
after the head, so one connection carries one request. When the server cannot be
reached, the command gets 502 with a short reason, such as `no such host`, `refused`
or `timed out`.

The upload budget (`network.max_upload_bytes`, 4 MiB) counts every byte that the
command sends after each `CONNECT` head, the TLS hello included, over every connection
of the call. A background job counts for its whole life. When the budget is spent, the
proxy cuts the connection and refuses new ones.

The proxy checks only the first TLS hello. A second hello after a HelloRetryRequest, a
TLS 1.2 renegotiation and the `:authority` of HTTP/2 are not checked.

### The allow list

`network.allow` holds presets, host names and `*.suffix` entries. The default holds the
six presets:

| Preset | Hosts |
|---|---|
| `@crates` | `crates.io`, `index.crates.io`, `static.crates.io` |
| `@npm` | `registry.npmjs.org`, `registry.yarnpkg.com` |
| `@pypi` | `pypi.org`, `files.pythonhosted.org` |
| `@go` | `proxy.golang.org`, `sum.golang.org` |
| `@github-read` | `github.com`, `codeload.github.com`, `objects.githubusercontent.com`, `raw.githubusercontent.com`, `release-assets.githubusercontent.com` |
| `@pacman-mirrors` | the hosts of the `Server =` lines of `/etc/pacman.d/mirrorlist` and of each file that an `Include =` line of `/etc/pacman.conf` names; empty when the files do not exist |

An entry of `allow` allows port 443. Write a name in lower case and in its ASCII
(`xn--`) form; `*` alone and a suffix of one label, such as `*.com`, are refused. efrd
reads the pacman files at start, at a reload, and again when one of them changed (at
most once a minute). `network.deny` holds entries that no command reaches; it wins
over the allow list and over every host grant. `[[network.hosts]]` adds a host with
options, and only an editor changes it:

```toml
[[network.hosts]]
host = "mirror.lan"
port = 8080
http = true       # plain GET and HEAD too
private = true    # it may resolve to an address of your LAN
```

To add a company registry or a `GOPROXY` host, add its host to `network.allow` (or to
`[[network.hosts]]` for another port), and set the registry in the tool's own config
as usual. The proxy variables of the call stay in front of it.

A change of `allow`, `deny` or `[[network.hosts]]` reaches every running call at once.
A change of `mode` applies to the next call; a running background job keeps its
proxy until it ends. `network.mode = "none"` and a running job: `efr jobs stop` ends
it.

### Hosts that are not on the list

- A host that the line shows, such as the URL of `curl` or `wget`, or the host of an
  https git remote, is a `host` exit with a grant of that one host and port for that
  one call. The reviewer may allow it only when a message of yours names the host. The
  next call asks again.
- A host that the line does not show, such as a host in a script, gets 403. The
  command's output then has a note, such as `[efr: the network proxy refused
  other.test:443: not on the allow list. To reach a host, call shell again with
  needs.hosts.]`, and the model can ask for the host with `needs.hosts`. The record of
  that next exit says that the proxy refused the host (`refused_by_proxy`). This is a
  fact, never your permission.
- Programs that do not use HTTP keep the whole network after a review: `nslookup`,
  `dig`, `host`, `drill`, `ping`, `traceroute`, `tracepath`, `mtr`, `nc`, `ncat`,
  `netcat`, `socat`, `telnet` and `ftp`. So does a `needs.hosts` entry of
  `localhost`, a private or link-local address, or a port other than 443 and 80. This
  is the route for a dev server that your browser must reach, a service on your
  machine or your LAN, and a client that does not read the proxy variables, such as
  `psql`, `redis-cli` or Java. Such a host never counts as on the allow list, so the
  reviewer needs a message of yours that names it.
- `ssh`, `mosh`, `autossh` and git over ssh (a fetch or clone from an ssh remote) run
  outside the sandbox with your ssh keys. The record names the host, and the reviewer
  may allow it only when a message of yours names the host, or when the remote belongs
  to your registered project and did not change since the conversation started. An ssh
  URL of `.gitmodules`, which a command can write, always asks you. A line that can
  send ssh to another host first or run a command on your machine always asks you too:
  `ssh -F`, `-J` or `-W`; `ssh -o` with `ProxyCommand`, `ProxyJump`, `HostName`,
  `LocalCommand`, `PermitLocalCommand`, `KnownHostsCommand`, `Match` or `Include`;
  `mosh --ssh` or `--client`; `git -c`, `--config-env` or `--exec-path`; `git clone
  -c`, `--config`, `--template` or `--recurse-submodules`; and a line that sets a
  `GIT_*`, `SSH_*` or `AUTOSSH_*` variable, such as `GIT_SSH_COMMAND`.
- `git push` and the other uploads stay `upload` exits outside the sandbox, as before.
  The reviewer now sees every host behind the remote name. A `git push` whose host efr
  cannot tell, such as `git --git-dir=... push`, a push after a `cd`, or a push to a
  local path, asks you, and so does a push in a line that sets a `GIT_*` or `SSH_*`
  variable.
- A line that also reads your network, such as `ss -tln; curl https://x`, keeps the
  `host_view` exit with the whole network: the network view never joins a proxy call.

### Refusals

A command that the proxy refused shows a dim line under its result, such as
`network: blocked other.test:443 (not on the allow list)`, and the model reads a note
with the reason. A background job lists its refusals in the line of its end. The
proxy keeps at most 32 records for each call, one for each host, port and reason,
with a count; `and N more` counts the rest.

| Reason | Why |
|---|---|
| `not on the allow list` | the host is not on a list, or the deny list holds it, or efrd denies it for the call because a file holds a credential for it |
| `a private address` | the name resolves to a local or private address |
| `the TLS name differs from the host` | the server name of the TLS hello is not the host |
| `not TLS` | no TLS hello with a server name, or a hello that asks for Encrypted Client Hello |
| `ip address` | the command named an IP address |
| `a method that is not allowed` | plain HTTP other than `GET` and `HEAD`, or a tunnel to port 80 or to a port that only an `http` entry allows |
| `bad request` | a malformed request head |
| `over the upload limit` | the call sent more than `max_upload_bytes` |
| `too many connections` | more than 64 open connections in the call |

### Credentials

The proxy lets a command reach an allowed host, so efr keeps credentials out of the
call:

- The read masks hide the credential files of git, the registries and the package
  tools (see "Read masks").
- The environment filter drops `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`,
  `GIT_CONFIG_KEY_*`, `GIT_CONFIG_VALUE_*`, names that end in `_AUTH`, and each
  variable whose value is a URL with a password, such as
  `PIP_INDEX_URL=https://user:token@...`. `sandbox.env_keep` can keep the last two
  kinds; `efr status` and `efr sandbox check` then warn about each kept name that looks
  like a secret.
- A token in a file that the sandbox must read cannot be hidden line by line. So when
  the project's `.npmrc`, `.yarnrc.yml`, `.yarnrc`, `bunfig.toml`, `.cargo/config.toml`
  or `.cargo/config`, or the git config of the project or of the command's directory
  (with `~/.gitconfig`, the XDG file and their includes, also outside a project) holds
  a password or a token for a host, efrd denies that host for the call. The output says why: `[efr: efr blocks registry.npmjs.org for
  this command: ~/p/app/.npmrc holds a credential for it. Move the credential to a file
  that the sandbox hides, such as ~/.npmrc.]`. A git `http.extraHeader` with no URL
  denies every remote host and the hosts of `@github-read`; a shell credential helper
  or a credential store in a write root denies the hosts of the remotes.

### When the proxy closes

The proxy socket of a foreground call closes when the command ends; open connections
get 200 ms to finish. A call that runs past its timeout keeps its socket until its
result comes or its dir goes (efrd looks at each prepare of the conversation and once
an hour). A background job keeps its socket until it ends. An efrd restart ends every
job; a foreground call that runs past its timeout loses its network then.

The probe checks the bridge at each start (see "When the sandbox cannot run"). When
the check fails, contained commands have no network, and `efr status` says so.

### What the proxy cannot stop

- The proxy cannot see inside TLS. A command can send data to a host on your allow
  list, with no credentials, up to the upload budget.
- The Go module proxy fetches a missing module from its origin host, so a request
  through `@go` can carry data to another host. A `*.suffix` entry lets a command pick
  a DNS name below the suffix, which the proxy resolves.
- A CDN that serves another site by the `Host` header inside TLS widens what an allowed
  host reaches. In the probe, Fastly answered 421 and Cloudflare 403.
- A socket grant that reaches the network goes around the proxy: systemd-resolved's
  varlink socket, nscd, the Docker and Podman sockets, tailscaled and a SOCKS socket.
- Tools that send an Encrypted Client Hello, also as a decoy, such as headless Chrome,
  Electron and Playwright, cannot fetch through the proxy.

## The bus filter

Read-only system queries run in the sandbox with no question. Examples:
`hostnamectl`, `timedatectl`, `localectl`, `systemctl --failed`,
`systemctl status nginx.service`, `systemctl --user --failed` and
`loginctl list-sessions`. Such a command reaches the system or the session bus only
through efr's bus filter, `xdg-dbus-proxy`, which lets through only a fixed list of
read methods. The filter runs while `dbus.proxy` is `true`, the default;
`dbus.proxy = false` turns it off, and these queries go to the reviewer again.
[`docs/adr/0013-bus-proxy-and-always-allow.md`](adr/0013-bus-proxy-and-always-allow.md)
records the decision.

### Which lines get the filter

A line gets the filtered bus when at least one of its simple commands is one of these
read forms, with no option that names another host, machine, root or image (`-H`,
`--host`, `-M`, `--machine`, `--root`, `--image`, `--image-policy`):

| Program | Read forms |
|---|---|
| `systemctl`, `systemctl --user` | no verb, `status`, `list-units`, `list-unit-files`, `list-sockets`, `list-timers`, `list-jobs`, `list-dependencies`, `is-active`, `is-failed`, `is-enabled`, `show`, `cat`, and `--failed` |
| `loginctl` | no verb, `list-sessions`, `list-users`, `list-seats`, `show-session`, `show-user`, `show-seat`, `session-status`, `user-status`, `seat-status` |
| `hostnamectl`, `timedatectl`, `localectl` | no verb, `status`, `show`; also `timedatectl timesync-status` and `show-timesync` |

The rule picks the lines that fit. It is not a security layer: in
`systemctl --failed; ./x.sh`, the script gets the same filtered bus. The filter, not
the rule, decides what a call can do on the bus. A `systemctl` form gets the system
bus, a `systemctl --user` form the session bus. Every other bus form keeps its `bus`
exit (see "Exits"), such as `systemctl get-default`, `notify-send`, `busctl`,
`resolvectl`, `coredumpctl`, `nmcli` and `iwctl`. A `systemctl` that changes
something, such as `systemctl restart nginx`, stays a `privilege` exit, and
`systemctl --user enable` a `persistence` exit.

### What the filter lets through

The built-in rules name each method by its full name and its path. These are the read
calls that efr recorded for the tools above:

| Bus | Service | Methods |
|---|---|---|
| system | `org.freedesktop.hostname1` | `Properties.Get` and `GetAll` at `/org/freedesktop/hostname1` |
| system | `org.freedesktop.systemd1` | `Properties.Get` and `GetAll`, and `Manager.ListUnits`, `ListUnitsByPatterns`, `ListUnitFiles`, `ListUnitFilesByPatterns`, `GetUnitFileState`, `GetUnit`, `GetUnitByPIDFD` and `ListJobs` at `/org/freedesktop/systemd1`; `Properties.Get` and `GetAll` at each unit |
| system | `org.freedesktop.login1` | `Manager.ListSessionsEx`, `ListUsers`, `ListSeats`, `GetSession`, `GetUser` and `GetSeat`; `Properties.Get` and `GetAll` below `/org/freedesktop/login1/` |
| system | `org.freedesktop.timedate1`, `locale1`, `timesync1` | `Properties.Get` and `GetAll` at the service's own path |
| session | `org.freedesktop.systemd1` | the same `Manager` methods, and `Properties.Get` and `GetAll` at each unit; no property of the manager itself, so its `Environment` stays hidden |

Every other method, name and path fails with `Access denied` (or `ServiceUnknown` for a
name that the filter hides), and the service never gets the call. So
`systemctl --user status` with no unit and `systemctl --user show-environment` fail.
`systemctl status <unit>` works without the process tree: `GetUnitProcesses` is not
in the list, because it shows the command lines of the processes of your machine,
which the sandbox hides. You can add it (see `[[dbus.allow]]` below).

`systemctl --user` reaches the session filter through a function of the sandbox's
shell. Before it runs `systemctl`, the function unsets `XDG_RUNTIME_DIR`, because
systemd otherwise looks only for the user manager's private socket. A script that runs
`systemctl --user` itself does not get the function, and fails.

### A call that the filter refuses

The command fails at that call, as it would with `Access denied` from the real bus.
The model reads one note with the refused call as data, such as `[efr: the bus filter
refused a call: bus=system name=org.freedesktop.systemd1
member=org.freedesktop.systemd1.Manager.StartUnit path=/org/freedesktop/systemd1. For
the whole bus for one command, call shell again with needs.bus.]`. While you follow the
turn, the view shows `bus: refused system org.freedesktop.systemd1
Manager.StartUnit`. The record of the call and the end of a job keep the refused
calls, at most 16 of them with a count.

When the model then asks for `needs.bus`, that is a `bus` exit for the whole bus, for
one call, and the reviewer or you decide. The next filtered call is filtered again.

### `[[dbus.allow]]`

`[[dbus.allow]]` adds methods to the built-in rules. It never replaces them. Only an
editor changes it; the settings tool and the `a` answer cannot.

```toml
[[dbus.allow]]
bus = "system"
name = "org.freedesktop.systemd1"
call = ["org.freedesktop.systemd1.Manager.GetDefaultTarget@/org/freedesktop/systemd1"]
```

Each `call` entry is `IFACE.MEMBER` or `IFACE.MEMBER@PATH`, and a PATH may end in
`/*`. efrd refuses an entry at start and at a reload, with its index, when it holds
`*`, `IFACE.*`, a path-only rule or an empty `call` list, when its name is not a
well-known bus name, a unique name (`:1.42`) or the bus driver, when a call is
`org.freedesktop.DBus.Properties.Set`, or when the name is a protected service:
secrets, a portal, polkit, Flatpak, kwallet or dconf. `efr config check` shows the
same errors.

Two more checks keep an entry to reads where efr can tell:

- For `org.freedesktop.systemd1`, `login1`, `hostname1`, `timedate1`, `locale1` and
  `timesync1`, an entry may add only `Properties.Get`, `Properties.GetAll`, or a
  member whose name starts with the word `Get`, `List`, `Can` or `Lookup`, on every
  interface.
- For every other service, efr refuses a member that starts with a word that changes
  the machine, such as `Set`, `Start`, `Stop`, `Restart`, `Kill`, `Enable`, `Mask`,
  `Reboot`, `PowerOff`, `Suspend`, `Lock`, `Inhibit`, `Mount`, `Create` or `Activate`,
  with or without capitals. This list is a floor, not a full guard: for these services
  efr cannot tell a read from a write. You own each method that you add.

An added method does not make a new read form: `systemctl get-default` still has its
`bus` exit. The method helps a script on a line that already has the filter.

### How the filter runs

- efrd starts one `xdg-dbus-proxy` for each filtered bus of a call, before the call
  starts, and ends it after the call. A call needs at most two, and most calls need
  none. A start takes about 1.5 ms. A background job keeps its filters for its whole
  life, with the rules of its start: a reload that removes a `[[dbus.allow]]` entry or
  sets `dbus.proxy = false` reaches only new calls and new jobs.
- The proxy runs under `efr-sbx dbus-proxy`, with its own Landlock domain, a seccomp
  filter and an fd limit: it reads only its program and its libraries, connects only
  to the two real buses, makes its sockets only in the call's `bus/` dir, and has no
  TCP. The call sees its socket at `/run/dbus/system_bus_socket` (over an empty
  `/run/dbus`) or at `$XDG_RUNTIME_DIR/bus`, with `DBUS_SYSTEM_BUS_ADDRESS` or
  `DBUS_SESSION_BUS_ADDRESS` set.
- A proxy that cannot start fails the call with a setup error. The call never gets the
  whole bus instead. A proxy that stops during a call gives the note `the bus filter
  stopped during the command`.
- On a machine with no session bus, a session read form runs with no bus, and the
  model reads `[efr: no session bus on this machine; the bus filter did not run.]`.
- efrd uses `xdg-dbus-proxy` only when root owns it and its directory, as for every
  program that efrd runs outside the sandbox. The Arch packages name it as an optional
  dependency.

### What the filter cannot do

- It checks no arguments. `Properties.Get` and `GetAll` on an allowed path read every
  property there. The properties of a session unit can show the environment of a
  transient unit, such as one that `systemd-run --user -E` started, which the sandbox
  hides.
- A client can send broadcast signals with no rule. The sender is the client's own
  unique name, so it cannot pose as a well-known name.
- The bus and polkit see the proxy, which runs in efrd's session, not your seat
  session. The built-in rules hold no method that polkit guards; `hostnamectl` asks for
  two such methods (the product UUID and the serial), and the filter refuses them, as
  the real bus does.
- A rule gives its name the right to be started by the bus. So the bus can start a
  listed service by activation. `~/.local/share/dbus-1` is a floor, so a sandboxed
  command cannot plant a service file there.
- The proxy itself runs outside the sandbox with an unfiltered connection to the real
  bus for each client. A memory bug in its parser reaches your full rights on that
  bus. The wrapper limits files, network and system calls, not what the proxy sends on
  the bus.

## Always-allow rules

At an exit question that offers it, `a` saves a standing answer. The card shows the
key with the rule in words, such as `a always: write under ~/notes`. `a` does not
answer at once: efr shows `save rule "write under ~/notes"? y/n`, and only `y` saves
the rule and runs the call. Any other key goes back to the question. A key that you
typed before the confirm line showed never answers it.

The rule goes to `[[sandbox.allow]]` at the end of `config.toml`, and efr keeps every
comment and blank line of the file. Then the call runs as with a "yes", and the view
shows `saved as sandbox.allow[2]: write under ~/notes`. When efr cannot save the rule
(the file is read-only, it changed three times while efr wrote it, or the rule fails a
floor check), the call still runs once, and the view shows `rule not saved: <reason>;
the command ran once`.

A later call that the rules cover runs with no question and no review, with the same
widening that a "yes" gives. The view shows a dim line, such as `allowed by your rule
sandbox.allow[2]: write under ~/notes`, and `efr history` shows `judge: always`. This
works also while you are away, in background jobs and in turns that a job notice
started: a rule is your standing answer and reads no message.

```toml
[[sandbox.allow]]
kind = "host"
host = "example.com"
port = 443

[[sandbox.allow]]
kind = "write"
under = "~/notes"
project = "~/p/app"

[[sandbox.allow]]
kind = "socket"
path = "~/.mpd/socket"
```

| Kind | Covers | Refused or disabled |
|---|---|---|
| `host` | a `host` exit whose grants are each that one host and port through the proxy (`port` is 443 when you leave it out) | a `*.suffix`, an IP address, `localhost`, a host of `network.deny`, a bad host name. A host rule never covers the whole network, so with `network.mode = "none"` it does nothing |
| `write` | a contained `write` exit whose binds all lie at or below `under`; with `project`, only in turns of that project | `under` outside your home dir, at or above `~` or a write root; at or below a floor, a mask, a secret, efr's own dirs, `projects.toml`, a synced dir or a `.git` dir; a dir that holds a built-in floor of your home, such as `~/.config`; a dir that holds more than one project; a link on the way |
| `socket` | a `socket` exit for that one socket, which is not a link | a service manager's socket, `docker.sock`, `podman.sock` and `containerd.sock`, a credential agent, a desktop socket, a bus socket, `$XDG_RUNTIME_DIR/systemd/*`, a path below a read mask, a path in efr's dirs or in a write root |

A rule covers a call only when it covers every exit of the call, and only when each of
these holds:

- The exit is one that the reviewer may judge. So a rule never covers a privilege,
  persistence, upload, desktop, masked-read, synced-folder, bus or device exit, a delete
  in a cache, `outside`, or a file tool such as `write_file`.
- The call runs in the sandbox, and each grant is narrow: one host and port, a
  contained write bind, one socket.
- Each path matches by its real path, after every link, as efrd finds it before the
  call. A path that efrd could not resolve is not covered, and neither is a socket that
  is a link.
- No floor, synced dir or project lies at or below a dir that the call would open,
  also a floor that does not exist yet. A grant of one file stays coverable.
- No `deny` or `ask` rule of yours (`[[permissions.rules]]`) names a path at or below a
  grant of the call, and `network.deny` still wins.

The order of the decision is: the floors, your `deny` and `ask` rules, the always
rules, the reviewer, you. The surface guard still checks the git dirs below a dir that
a rule opened, so a planted hook goes to the quarantine.

Only a terminal on your machine saves a rule. A phone gets `forbidden` for `a`, and
the question stays open. efr never writes a rule for the model, and the settings tool
cannot write `sandbox.allow`. Any program of yours that can write `config.toml` can
still add a rule: efrd reloads the file when it changes.

efrd checks each rule when it loads the file. A rule of a bad shape, such as
`under = 5`, stops efrd at start and makes a reload fail. A rule that fails a floor
check stays in the file but is disabled: efrd logs one warning, `efr sandbox check`
shows `sandbox.allow[1] is disabled: <reason>`, and the rule covers no call. Such a rule can
turn bad with no edit, for example when you register a new project below it.
`efr config check` checks only the shape.

`efr sandbox allow` lists the rules with their index, and `disabled: <reason>` for a
disabled one. `efr sandbox allow remove N` shows rule N in words, asks
`remove sandbox.allow[N]: <rule>? y/n`, and removes it only when entry N still holds
that rule. Then efrd reloads the file.

`[[sandbox.allow]]` covers shell commands in `auto`. `[[permissions.rules]]` with
`write allow` still covers `write_file` and `edit`; see
[`docs/permissions.md`](permissions.md), "Your rules".

## Background jobs

A background job is a command that runs on after its call and after its turn, such as
a dev server, a watcher or a slow build. The model starts one with `shell` and
`background: true`. The call answers at once with the job's id (`j1`, `j2` and so on
in each conversation) and its output file. `&`, `nohup` and `setsid` in a normal
command still stop when that command ends; only a job runs on.

### What the model asks for

- `max_minutes`: the time limit. The default is `jobs.default_minutes` (60). efr cuts a
  longer value to `jobs.max_minutes` (480), and the start result says so: `time limit:
  480 minutes (the most that efr allows)`.
- `watch`: a pattern (a regular expression of at most 200 characters) and
  `max_wakes`. Each output line that matches tells the model, at most `max_wakes`
  times, cut to `jobs.max_wakes_per_watch` (3).
- A job takes no `timeout_seconds` and no `nested_shell`, and a line that waits for
  input at the terminal cannot run as a job: its stdin is `/dev/null`.

### Modes

A job can start in every mode.

- In `auto`, the job's line goes through the exits like any line. A line with no exit
  starts at once in the sandbox. An exit goes to the reviewer or to you, as for a
  normal call, and the record tells the reviewer that the grants last for the whole
  job, up to its time limit. A covered delete in a job still goes to the reviewer,
  because undo of the start turn needs `--force`.
- In `cautious` and `manual`, efr asks you before each job starts. No reviewer
  decides it. An approved job runs in the exit child with your rights.
- In every mode, efr refuses a job whose line asks for one of these kinds: `sudo` and
  other ways to more rights, a read of a masked file, a change of a file that runs code later (also one
  that you name), a desktop command that types or starts programs, the whole D-Bus
  bus (a read-only query such as `systemctl --failed` runs with its own filtered bus
  for the whole job instead), and
  a socket of a container engine or a service manager (`docker.sock`,
  `podman.sock`, `systemd/private`). The model gets `this cannot run as a job; run that
  part in the foreground, and the rest as a job`. Your yes does not change this: a job
  has no terminal for a password, and a process that such a grant starts would outlive
  the job. efr reads only the line: outside `auto`, an approved job runs with your
  rights, so a program that it starts can still do these things. Only a setuid program
  cannot give it more rights.
- The question of a job names its lifetime: `runs as a job for up to 60 minutes; these
  grants stay open until it ends`. Only a terminal of yours answers it, never a phone.
  While you are away, it expires as other questions do, and no job starts.

### How a job runs

- efrd starts the launcher itself (`efr-sbx run --job`), not the hidden shell. The job
  has no terminal and its own session, and its stdin is `/dev/null`. It runs at nice
  `jobs.nice` (19) in the idle IO class, each file that it writes can grow to at most
  4 GiB, and the kernel's out-of-memory killer takes it before your programs. A job in
  the exit child cannot gain rights through a setuid program (`no_new_privs`).
- The job gets the environment of the hidden shell, which the shell writes to a file
  at each prompt where it changed, and starts in the hidden shell's directory.
- A contained job writes where the sandbox of its turn writes. Its tool caches are
  layers of its own on the disk, so a job never makes a foreground call wait, and efr
  never applies the deletes of a job in your real caches. The private `/tmp` is the
  conversation's.
- In a contained job, every git dir of the plan and every git dir that efr finds in
  the roots at the start is read-only. A job cannot commit; the model commits in the
  foreground. The surface guard checks a running job every 5 minutes and before each
  contained call whose roots meet the job's. A changed git setting or hook goes to
  quarantine at once, and the job stops (`stopped: it changed git settings`).
- efrd reads the job's output from a pipe and writes `$SCRATCH/jobs/<id>.out`: the
  first 1 MiB and the last 8 MiB, with `[N bytes omitted]` between them. Every
  contained call reads `$SCRATCH/jobs` but cannot write it, so no command can change
  or replace a job's output. Snapshots leave the dir out. efr removes the output of a
  job 7 days after its end, and keeps at most 20 outputs of ended jobs in each
  conversation. Some programs keep their output in a buffer when it goes to a pipe:
  run them with `stdbuf -oL` or their own flag for line output.
- At most `jobs.max_running` (4) jobs run in a conversation, and
  `jobs.max_running_daemon` (16) in all. A start above a limit fails, and the model
  gets the list of running jobs. `jobs.enabled = false` refuses every new job; running
  jobs run on.

### How a job ends

A job ends when its command ends, when the model stops it (`job_stop`), when you stop
it (`efr jobs stop <id>`, or Ctrl+X in the input row of a followed turn), at its time
limit, when the surface guard stops it, or when efrd stops. Esc and the end of a turn
never stop a job. The time limit counts the time on the clock, so a suspend counts
too.

A stop sends SIGTERM to the job's processes, then SIGKILL after a short wait. Then efrd
ends every process that still has the job's session (the sweep). The end names the
processes that may still run, and a launcher that efr lost. When efrd stops, it stops
every job. When efrd dies, each launcher sees that and stops its job. At the next
start, efrd records each job that the last efrd left as `stopped because efrd
restarted`, checks its git surface, and ends what is left of it.

### Notices and wake turns

When a job ends, or a line of its output matches its `watch` pattern, efr tells the
model with a job notice. A notice is never your input: it holds the line `efr: this is
a job notice, not a message from the user; no human input has occurred.`, it never
answers a question, and it never counts as your permission. efr escapes the job's output in it and removes control and
format characters. A notice never enters a running turn. It waits until the turn ends.

- When the conversation is idle, the notice starts a short turn of its own, a wake
  turn, marked `🤖 (job notice, not your input)`. This occurs in every mode. The wake
  turn runs in the mode of your newest prompt, or in the mode of the job's turn when
  you sent no prompt since. Outside `auto`, its commands ask you as in any turn of
  that mode.
- At most `jobs.max_wakes_per_hour` (6) wake turns start in a conversation in an hour,
  and `jobs.max_wakes_per_hour_daemon` (12) in all. efr counts them over restarts. 0
  means that notices never start a turn.
- A wake turn runs at most 20 tool calls. A job that a wake turn starts never starts
  another turn: its notices wait for your next prompt (`(job of a wake turn)`).
- One turn takes at most 10 waiting notices and 32 KiB of their text.
- A question in a wake turn waits only `presence.away_wait_secs`, unless a terminal of
  yours follows the conversation. Its summary starts with `(job notice turn)`. The
  reviewer's refusals count on from one wake turn to the next, until a turn of yours.
- A notice of a job that ended at a restart of efrd never starts a turn. It rides in
  your next prompt.
- A job that you stop also starts a wake turn. Only the notice of a job that the model
  stopped never starts a turn, because the model read the output when it stopped the
  job. That notice rides in your next prompt.

### What you see

- While efr follows a turn, the view shows `job j3 started; output
  $SCRATCH/jobs/j3.out; ends in 60 minutes` under the call, which shows `(background)`.
  It shows each end of a job of the conversation, such as `job j3 stopped by you after
  92 minutes; 1.2 MB output`, a matched line (`job j3 matched: <line>`) and a notice
  that did not start a turn (`job j3: no wake turn (wake limit reached)`).
- Between turns, your next prompt shows `efr: job j3 ended (exit 0): <title>`.
- `efr jobs` lists the jobs of this terminal's conversation; `--all` lists them all.
  `efr jobs stop j3` stops one and shows its last lines.
- `efr history` shows each start and end, and marks a wake turn; `--verbose` adds the
  last lines of the output and the text of each notice.

### Undo and jobs

`efr undo` refuses while a job of the conversation runs, or a job of another
conversation can write in a root of the undo, and names it. A job outside the sandbox
runs as you and can write everywhere, so it blocks every undo. The refusal says:
`job j3 runs; stop it first (efr jobs stop j3)`. When a job ends, its file changes join
the turn that started it, and the undo of that turn needs `--force`. A file that a later
call also changed stays with that call's turn. A job's copies of files outside the
project have no "after" copy, so undo lists them as `not covered (a job wrote it)`, and
`--force` brings back the state before the job.

A job's end starts a wake turn. When that wake turn changed nothing, `efr undo` with no
`--turn` passes over it and takes back the job's turn, and the wake turn does not count
as a newer turn that blocks the undo.

## Undo

`efr undo` takes back the newest turn of this terminal's conversation that is not
undone. It shows a preview and asks `undo? y = yes, n = no`:

```
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

- Undo acts only on the paths that the turn's own calls changed. A change that you
  made between two calls stays. A change that another conversation or you made while
  a call ran counts as the call's change, so its old bytes go to the undo trash.
- A path that changed after the turn stays, and the preview says `skip`. `--force`
  overwrites it. When only `--force` acts on any row of the preview, `efr undo`
  without it changes nothing, does not ask, and fails with a hint to use `--force`.
  A path that a snapshot left out, such as a large ignored file, is
  `not covered`: undo never writes, deletes or trashes it.
- Every file that undo overwrites or removes, also outside the project, goes to
  `~/.local/share/efr/undo-trash/<undo-id>/` first. A file that undo makes again gets
  its mode from the snapshot, without set-id bits; a `.env` with mode 0600 stays 0600.
  A directory that undo makes again gets mode 0700, because no snapshot records the
  mode of a directory.
- Undo restores the branches, tags, `refs/stash` and `HEAD` that the turn moved, from
  the mirror that efr took at the turn's first snapshot, by compare and swap. Your git
  index stays as it is. A turn that only moved refs, such as `git reset --soft`, ends
  with `moved 1 ref in <project>; efr undo reverts this turn`. When efr cannot keep
  the refs that an undo moves away from, the undo leaves those refs as they are, so a
  redo can always give them back.
- Undo is a stack: a turn cannot be undone while a newer turn of the conversation is
  not undone, unless `--force`. `--redo` takes back the newest undo. `--list` shows the
  turns and which are undone or stopped. `--turn` picks one, `--dry-run` only shows
  the preview, and `--yes` needs no question. Without a terminal, `efr undo` needs
  `--yes`.
- Undo refuses while a turn, a compaction or another undo of the conversation runs, and
  while a command of the conversation still runs after its timeout. A prompt that you
  send during an undo waits for it.
- An undo that stopped half way, such as after a stop of efrd, goes on at the next
  `efr undo`. Undo holds the plan lock of each project that it writes. An undo that
  stopped at a write that failed says `undo stopped: ...`, and `efr history` shows it.
  An undo that could write nothing, such as when each path changed again, leaves the
  turn as it is.
- A call that you interrupt, or a command that runs on after its timeout, can change
  files after efr's last snapshot of the call. The turn then says `not covered:
  <project> (a call did not finish)`, and `efr undo --force` takes back those files too. A
  home copy whose call did not finish shows as `not covered: <path> (the call did not
  finish)`.
- Undo works for a turn of every mode with a snapshot. The hint `efr undo reverts this
  turn` shows only after an `auto` turn. In `manual` and `cautious`, a line runs in
  your own shell, and writes outside the project are not covered; the preview of such
  a turn says so.
- A phone cannot undo. The model has no undo tool, and `undo.*` and `snapshot.*` are
  settings that only you change.

**Routine destructive lines.** In `auto`, with `sandbox.ask_destructive =
"unless_undoable"` (the default) and `undo.enabled = true`, a destructive part of a
line runs with no review and no question when efr's snapshot holds every file that it
can change. efr checks this before the decision and again under the plan lock, after
the call's own snapshot. When the check fails then, the call does not run, and the
model runs the line again.

| Form | Covered when |
|---|---|
| `rm -r <p>`, `dd of=<p>`, `truncate <p>`, `: > <p>`, `find <p> -delete`, an `apply_patch` delete or move | `<p>` lies below a project or `$SCRATCH` that has a snapshot of this turn, not in a git dir or a floor, and each file at or below it is in the snapshot |
| `git reset --hard`, `git checkout` that discards, `git restore`, `git switch --discard-changes` | every tracked file is in the snapshot, the index equals `HEAD`, and for `reset` and `switch` the mirror holds the refs |
| `git branch -D` | the mirror holds the refs |
| `git stash drop`, `git stash clear` | the mirror holds `refs/stash`, and the stash has one entry |
| `git clean -f` without `-x` and `-X` | every untracked file is in the snapshot |
| `shred`, `git clean -x` and `-X`, `git reflog expire`, `git gc --prune` | never |

Only the `destructive` exit of that part goes; every other exit of the line keeps its
route. In `manual` and `cautious`, efr still asks you for each delete.
`sandbox.ask_destructive = "always"` keeps the review. `undo.enabled = false` or
`snapshot.enabled = false` turns undo and the routine destructive lines off. With
`snapshot.enabled = false`, a named floor file edit asks you again, because efr cannot
copy it first.

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

After the self-test passed, the probe runs a second launch with the network view and
its own checks. Each check passes only on its exact error code. `tcp4` runs first: when
a TCP socket does not fail with `EAFNOSUPPORT`, the profile is not in place, and the
self-test stops before it opens any other socket. Then `tcp6`, `udp4`, `udp6`,
`packet` and other families (`EAFNOSUPPORT`), the `ROUTE` and `SOCK_DIAG` netlink
sockets (they must open), other netlink protocols (`EPROTONOSUPPORT`), `listen` on an
unnamed Unix socket and `pidfd_getfd` (`EPERM`), a connect and a send to the probe's
abstract sockets outside the sandbox (`EPERM` from Landlock), and a socket path
(`EACCES`). A failure here is a warning, never a failure of the sandbox: `auto` still
runs, the network view is off, and `ss` and `ip` go to the reviewer. The warning says
`the network view's self-test failed, so ss and ip need a review: <check>`. efrd
builds its engine again after each probe, so the view follows the newest result.

A third launch checks the proxy's bridge with a test socket of the probe, which
answers like the proxy. `proxy_tcp`: a TCP connect to `127.0.0.1:3128`, a `CONNECT
probe.test:443`, the answer and one echoed byte. `proxy_socket_direct`: a connect to
the bound socket gives `EACCES`. `proxy_tcp_other`: TCP to `192.0.2.1:9` gives
`ENETUNREACH`. `proxy_bridge_signal`: a signal to the bridge gives `EPERM`. A failure
here is a warning too: with `network.mode = "proxy"`, efrd falls back to no network,
logs one warning, and the first contained command after each start of efrd gets the
note `efr's network proxy is off: its self-test failed; run efr status`. `efr status`
then shows `network none (proxy self-test failed)`.

While `dbus.proxy` is `true`, efrd also checks the bus filter at start and after each
probe: `xdg-dbus-proxy --version` must say 0.1.5 or newer, and one filter with the
built-in rules must start under the wrapper, write its ready byte and end cleanly. A
failure here is a warning too: the filter is off, and the read-only queries of "The bus
filter" go to the reviewer as `bus` exits, which is stricter, never looser. `efr
status` shows `bus filter on`, or `bus filter off` with the reason.

These warnings show in `efr sandbox check` and in efrd's log; `auto` still runs:

| Warning | Meaning | Fix |
|---|---|---|
| `efr's bus filter is off: xdg-dbus-proxy is not installed; ...` | `dbus.proxy` is `true`, but efrd found no `xdg-dbus-proxy` that root owns | Arch: `pacman -S xdg-dbus-proxy`; Debian and Ubuntu: `apt install xdg-dbus-proxy` |
| `efr's bus filter is off: the installed xdg-dbus-proxy is older than the oldest version that efr tested; ...` | the program is older than 0.1.5 | update the package |
| `efr's bus filter is off: its self-test failed; ...` | the filter did not start or did not end cleanly | run `efr sandbox check`; efrd's log has the error |
| `sandbox.allow[1] is disabled: <reason>` | an always rule crosses a floor (see "Always-allow rules") | change or remove the rule: `efr sandbox allow remove 1` |

| Failure | Reason | Fix |
|---|---|---|
| no bubblewrap | `bubblewrap is not installed` | Arch: `pacman -S bubblewrap`; Debian and Ubuntu: `apt install bubblewrap`; Fedora: `dnf install bubblewrap` |
| old bubblewrap | `bubblewrap 0.6.2 found; it lacks --bind-fd` | update the package: efr needs bubblewrap 0.10.0 or newer, or a package with `--bind-fd`, such as Ubuntu 24.04's 0.9.0-1ubuntu0.3 |
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
netview yes
network proxy
auto: ready
```

A failed check has a `fix:` line below it. The command exits with 1 when the sandbox
is not available. `netview no (the warning above says why)` means that the self-test
of the network view failed; `auto` still works. `netview off (network.netview = false)`
means that you turned the view off. `network proxy` means that contained commands
reach the network through efr's proxy; `network none` that they have no network
(`network.mode = "none"`), and `network none (proxy self-test failed)` that the proxy
is off because its self-test failed. `bus filter on` means that read-only system
queries run through the bus filter; `bus filter off` names the reason, such as
`dbus.proxy = false` or a failed self-test.

`efr sandbox allow` lists the always rules of `config.toml` with their index, and
`disabled: <reason>` for a rule that crosses a floor. `efr sandbox allow remove N`
shows rule N, asks `remove sandbox.allow[N]: <rule>? y/n`, removes it and reloads the
config (see "Always-allow rules").

`efr sandbox explain PATH` tells what a contained command can do with one path, from
the current directory:

```
$ efr sandbox explain ~/.zshrc
~/.zshrc, from ~/p/eggfriedrice.code in auto:
  read   yes
  write  no: a shell startup file (floor); a write goes to the reviewer when a message of yours names the file, and efr copies the file first, so efr undo reverts it; else you decide; a write is a persistence exit
```

`, user only` follows the exit for a kind that only you approve, such as a synced
write.

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

## Where the time of a call goes

Each step of a call writes a debug line to the log of efrd. The line has the name of
the step (`phase`) and its time in milliseconds (`elapsed_ms`), in its text and as
fields. journald keeps the fields apart from the text, as `F_PHASE` and
`F_ELAPSED_MS`, so the text must hold them too. To see the lines of one prompt:

1. Set the log filter: `efr config set log 'info,efr_=debug'`. efrd applies it at
   once, with no restart.
2. Send the prompt.
3. Read the lines: `journalctl --user -u efrd --since -5min | grep phase=`. Under
   `just run`, efrd writes them to stderr.
4. Set the filter back: `efr config unset log`.

The steps, in the order of a call:

| Phase | What takes the time |
|---|---|
| `model_open`, `model_first_event`, `model_first_tool_call`, `model_answer` | the model: from the request to the open stream, to the first event, to the first tool call and to the end of the answer; each request has these lines. The `model_answer` line also names `stop_reason`, such as `end_turn`, `tool_use`, `max_tokens` or `refusal`, or `error` and `interrupted` for a call that did not complete |
| `provider_connect`, `provider_accepted`, `provider_first_event` | the transport of the request: a new WebSocket's handshake, then the time to the server's first event and to the first text, reasoning or tool call. The lines name the transport (`transport=http` or `websocket`), the connection (`new`, `reused`, or `pool` for HTTP) and the input (`full` or `incremental`), so you can compare the two transports |
| `requirements` | what the call needs: `real_paths` (the links of its paths) and, in `auto`, `facts` (the programs, the files and git of the line) |
| `engine`, `exit_prediction` | the permission engine, and the narrowest launch for the exits of the line |
| `record_started` | the event log records the start of the call |
| `approval_request`, `approval_wait` | the question, and the time until you answer it; only when efr asks |
| `tool_run` | the whole run of the tool, with the steps below |
| `shell_free_wait` | efr waits until the hidden shell is free |
| `sandbox_prepare` | the call's dir: `plan_lock`, `plan_build` and `call_dir_write` |
| `surface_before` | the record of the git settings and the files that run code, before the call |
| `sandboxed_run` | the run in the hidden shell, from the typed line to the end |
| `shell_typing` | zsh reads the typed line and starts it |
| `shell_command` | the wrapper in the hidden shell: `wrapper_snapshot` (the snapshot of your functions, aliases and options, when it is old), `wrapper_launcher` and `wrapper_apply` (the `cd` and the exports that come back) |
| `launcher_*` | the steps of the launcher: `prepare_dirs`, `plan`, `state_read`, `guard_before`, `layer_lock`, `launch_open`, `bwrap_setup`, `layers` (the cache overlays), `child` (the command), `teardown`, `records_and_state` and `guard_after` |
| `shell_check`, `shell_total` | efr reads the facts of the end of the run; the run from the typed line to its end |
| `call_dir_remove` | efr removes the call's dir |
| `record_completed` | the event log records the result |
| `tool_call` | the whole call, from the model's tool call to its result |

Then the next request to the model starts, with its own `model_*` lines.

The bench `shell_routine_calls_in_auto_cost_close_to_cautious` of efrd's tests runs
ten `true` calls in `auto` and in `cautious` with a model that answers at once. It
prints the time of each call and the time of each step per call. Run it in a release
build:

```sh
cargo build --release -p efr-sbx
EFR_TEST_SBX_BIN=$PWD/target/release/efr-sbx EFR_TEST_ZSH=1 cargo nextest run --release \
    -p efr-daemon --success-output immediate -E 'test(shell_routine_calls_in_auto)'
```

In a release build, it fails when a call in `auto` costs 40 ms more than a call in
`cautious`, or when a call costs more than 250 ms. The turns in `auto` from the
project, in `auto` from the home directory and in `cautious` take turns, so all see
the same load. A debug build compares the fastest call of each
mode, because load delays some calls but never makes one faster: it fails when the
fastest call in `auto`, from the project or from the home directory, costs 75 ms more
than the fastest call in `cautious`.

The bench `shell_call_phases_in_a_project` shows where the time goes in a project of
your choice. It runs 50 `true` calls in `auto` and prints the mean and the 95th
percentile of each step. `EFR_BENCH_PROJECT` names the project, as an absolute path;
without it, the bench makes a small git repository. It checks nothing, so it runs only
when you ask for it:

```sh
cargo build --release -p efr-sbx
EFR_TEST_SBX_BIN=$PWD/target/release/efr-sbx EFR_TEST_ZSH=1 EFR_BENCH_PROJECT=<dir> \
    cargo nextest run --release -p efr-daemon --run-ignored only \
    --success-output immediate -E 'test(shell_call_phases)'
```

Give it a copy of a project. The surface guard moves a git setting that changes while
a call runs to quarantine, also when another program changes it. The bench keeps its
runtime dir on the disk, not on tmpfs, so `call_dir_write` costs more there than in a
real efrd.

## Settings

The `[sandbox]` table of `config.toml` holds every key; [`docs/config.md`](config.md)
lists them with their defaults. Each key applies from the next call or turn. A change
of efr's settings through the settings tool still asks, in every mode.

`[classifier]` controls the reviewer: `enabled` (`false` asks you about every exit),
`model`, `fallback_to_turn_model`, `effort` and `timeout_secs`. `[presence]` controls
when efr counts you away: `away_after_minutes`, `away_wait_secs` and `push` (no phone
client exists yet, so it does nothing today). Each key applies from the next
question, and none needs a restart.

`[network]` holds `netview` (default `true`): `false` makes `ss`, `ip` read forms,
`nstat` and reads of `/proc/net` `host_view` exits for the reviewer again. It also
holds the keys of the proxy: `mode`, `allow`, `deny`, `[[network.hosts]]` and
`max_upload_bytes` (see "The network proxy"). Each applies from the next call, with no
restart; the allow and deny lists also reach the calls that run. The settings tool
marks each change that opens more as "loosens permissions": `netview` from `false` to
`true`, `mode` to `"proxy"`, a new `allow` entry, a removed `deny` entry, a new or wider
`[[network.hosts]]` entry, and a higher `max_upload_bytes`. Only an editor changes
`[[network.hosts]]`.

`[dbus]` holds `proxy` (default `true`), which turns the bus filter on, and
`[[dbus.allow]]`, more bus methods for the filter (see "The bus filter").
`[[sandbox.allow]]` holds the always rules (see "Always-allow rules"). Each applies from
the next call, with no restart; a running job keeps the bus rules of its start. The
settings tool marks `dbus.proxy` from `false` to `true` as "loosens permissions", and
it cannot write `dbus.allow` or `sandbox.allow`: only an editor, the `a` answer and
`efr sandbox allow remove` change those.

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
| a `bus` grant, or a `host` grant of the whole network, opens more than the line needs | the approved call can use the whole bus or the whole network | the question says so; with the proxy, a host that the line shows gets a grant of that one host; a read-only system query gets the bus filter instead |
| the bus filter checks no arguments | `Properties.Get` on an allowed path reads every property there, also the environment of a transient user unit | exact paths; no property of the user manager itself |
| the bus filter runs outside the sandbox with your rights on the bus | a memory bug in `xdg-dbus-proxy` reaches your full bus rights, on the session bus also `StartTransientUnit` | the distribution's program, owned by root; its own Landlock domain, seccomp filter and fd limit; one filter per call that ends with it |
| a script that runs `systemctl --user` itself | it fails, because systemd looks only for its private socket | run `systemctl --user` on the line, where the sandbox's shell gives it the session bus |
| a running job keeps the bus rules of its start | a removed `[[dbus.allow]]` entry or `dbus.proxy = false` does not reach it | `efr jobs stop` ends it |
| an always rule stays open for every later call | a message or a file that steers the model can use a host or a write dir that a rule opened | narrow kinds; the floors; the confirm step; `efr sandbox allow remove N` |
| a one-time "yes" for a write to a dir above a floor that does not exist yet, such as `~/.config` with no `~/.config/autostart` | the call can make that floor | always rules never cover such a dir; the reviewer sees the dir |
| `tty` and `$$` show the sandbox's view | confusing output for system work | the prompt text says so; `df`, `findmnt`, `ss` and `ip` show your machine through the views |
| a call with the network view runs in your network namespace and can run any code | a gap in the seccomp profile would let it send from your machine | only Unix and netlink sockets, and only two netlink protocols; `listen` and `pidfd_getfd` refused; Landlock refuses TCP; the probe checks each refusal at every start; the view only beside `write` and `unmask` grants |
| a host program can send a datagram to an abstract socket name that a network view call binds, also with a socket in it (`SCM_RIGHTS`) | the call could receive a connected socket from a host program and send through it | it needs a host program that sends to that name on purpose; `listen` fails, so no stream service; a job ends at its time limit |
| a call with the network view sees your sockets, peers and routes | the model can learn the sites that you visit | the purpose of the view; before, the same output came after a review |
| a running job keeps the network view after `network.netview = false` or a failed probe | the job reads your network until it ends | its filter is in place and the kernel does not change; `efr jobs stop` ends it |
| every contained call can list every mount of the machine through `/proc/<pid>/mountinfo` | the call learns your mounts, also those of other users and containers | none: a new procfs or `hidepid` would hide `ps` too; the mount view adds sizes only for your own mounts outside masks |
| the mount view shows how full your hidden tmpfs mounts are | the call learns numbers, not names or contents | no sizes below the masks and secrets of the call |
| the `df` and `findmnt` of the mount view work only on the line | a script or `sh -c 'df'` shows the sandbox's mounts | that output is safe; it shows the sandbox; a path or a wrapper, such as `watch df`, is an `outside` exit |
| a long job keeps the mount snapshot of its start | old sizes in a job's output | none |
| a hung NFS or FUSE mount, an automount | `df` could hang or mount a share | the helper ends after one second, the mount shows `-` for a minute; no `statvfs` for `autofs` and the other dummy types |
| an approved exit outside the sandbox has your full rights | it can read secrets, use the network, or start a service through `systemd-run --user` that the call end cannot stop | one command per exit; the question shows the whole line, each program and the untrusted mark; privilege, upload and persistence are yours only |
| changes to your caches while an overlay uses them | old or wrong cache content in the sandbox | `sandbox.cache_mode = "tmp"` |
| no network with `network.mode = "none"`, or a host that is not on the allow list | a build that must download fails | with the proxy, the registries are on the default list; a host that the line shows is a review for that one host; without the proxy, the network is an exit, and `sandbox.offline_hints` tells package managers to use the cache |
| the proxy cannot see inside TLS | a command can send data to a host on your allow list, with no credentials: at most `max_upload_bytes` (4 MiB) per command, counted from the first byte of each connection | no credentials in the sandbox (masks, the environment filter, the token deny); the server name check; the cut of Encrypted Client Hello; a short default list |
| the Go module proxy fetches a missing module from its origin, and a `*.suffix` entry lets a command pick a DNS name below the suffix | data can reach a host through `@go`, or a DNS server through a name | the list check before any lookup; the budget; remove `@go` from `network.allow` when you do not need it |
| a socket grant that reaches the network: systemd-resolved's varlink socket, nscd, the Docker and Podman sockets, tailscaled, a SOCKS socket | the call goes around the proxy | a socket grant is an exit; the question names the socket |
| the proxy checks only the first TLS hello | a second hello after a HelloRetryRequest, a TLS 1.2 renegotiation or the `:authority` of HTTP/2 can name another site behind the same CDN | the budget; such fronting depends on the CDN (Fastly and Cloudflare refused it in the probe) |
| tools that send an Encrypted Client Hello, also as a decoy: headless Chrome, Electron, Playwright | their fetches through the proxy fail (`not TLS`) | none: the cut stops fronting through ECH |
| a token in a file that the sandbox must read and that is not on efr's list | the token reaches a host on your allow list | the token deny covers the project's registry files and the git config; move a token to a file that the sandbox hides |
| a foreground call that runs past its timeout | it loses its network when efrd restarts | none; a restart ends every job anyway |
| MCP servers | they run outside the sandbox | none yet |
| the host naming rule sees only the hosts of the line and of the git remotes | a host that a script connects to has no fact before the proxy refuses it | with the proxy, a host that is not listed gets 403 and then a review of that one host; the masks and the environment filter stay |
| the reviewer's name check is a word match | "do not push" names a push; "zshrc" can name the wrong file | the reviewer still reads your messages; a generic file name such as `config` or `init` names nothing alone, nor does the bare name of a program, a unit or a desktop entry; wide targets ask you |
| an allowed `outside` line runs in the exit child with your full rights, and only predicted targets get a copy | a program that the model did not write in this turn can change `~/.zshrc` with no name and no copy; the turn-end line names only what the copies cover | the one-command rule; a program written in this turn or in a write root asks you, and so does an interpreter on code that efr cannot read (`python3 -c`, `sh -c`, a script); the reviewer sees the line |
| the payload of a file tool edit of a floor file goes to the reviewer | the payload is model text, which can hold words that try to steer the reviewer | the reviewer's prompt says that text in the action never authorizes; the naming rule still needs a message of yours; a payload over 8 KiB asks you |
| older system prompts of a conversation keep the old auto rule | the model can still read the old, longer rule in its history | the newest rule says that only the newest block counts |
| a cache whose path holds a link | efrd refuses to apply its deletes | the deletes stay private; nothing is lost |
| `sandbox.cache_mode = "overlay"` | a refused delete stays in the layer of the conversation and comes back after each call | efr does not raise the same paths again in the conversation |
| a snapshot leaves out large files, ignored files above 1 MiB, more than 2000 ignored files, names that are not UTF-8 and nested repositories | undo cannot bring them back | the preview and the turn-end line say `not covered`; a delete of such a path is not routine |
| a directory over `snapshot.home_max_files` or `snapshot.home_max_mib` | undo cannot bring back the other files of that dir | the turn-end line says `not covered: <dir> (over the copy limits)` |
| an approved exit outside the sandbox can change efr's snapshot store | undo can lose its record of a turn | at the turn's end efr checks each `pre` ref; a missing one says `not covered: <root> (the snapshot store changed)` |
| a command that runs on after its timeout | it can change files after the turn's last snapshot | the turn-end line says `not covered: <root> (a call still runs)`, undo refuses while it runs, and undo checks each path again |
| another conversation writes the same project while a call runs | that write counts as the call's change | undo moves its bytes to the undo trash; the plan lock orders the calls |
| content that is only staged, in neither the work tree nor `HEAD` | `git reset --hard` loses it | such a reset is not covered, so it goes to the reviewer |
| older stash entries live only in the reflog | `git stash clear` loses them | stash drop and clear are covered only with one entry |
| a snapshot holds no empty dirs, fifos, sockets, devices, extended attributes, ACLs, owners or hard links | a covered `rm -r` loses them, and undo makes hard-linked files separate | the preview notes `empty directories and special files do not come back` |
| `at`, `batch` and `systemd-run --user` | they run later with no copy | they stay with you; a started unit runs on after an undo |
| a background job in the exit child runs as you | it can open the hidden shell's terminal by its path and read efr's runtime files | it holds no terminal and no descriptor of one; `no_new_privs`; its time limit; a thin mount namespace for such jobs comes later |
| a job in the exit child that starts a process in a new session (`setsid`) and ends its own launcher, in any order | that process can escape the stop, the time limit and the sweep; the end does not name it, and efrd's socket counts it as your process, not the model's | the sweep by the job's session ends every other process; the end names a lost launcher; a cgroup per job comes later |
| a job keeps the sandbox plan of its start | a new mask, secret or floor of a settings change does not reach a running job | stop the job and let the model start it again |
| a job writes a path that a later call also changes | that change stays with the later call's turn, not with the job's | undo refuses while a job writes in its roots |
| a dev server in a contained job listens on the sandbox's own loopback | your browser cannot reach it | the model names `localhost` with the port in `needs.hosts`, and after a review the job gets the whole network, so it listens on your loopback; the reviewer allows this only when a message of yours names the host; a grant to listen comes later |
| a job's output goes to a pipe | some programs keep their output in a buffer, so the output file and the notices come late | run the program with `stdbuf -oL` or its own flag for line output |
