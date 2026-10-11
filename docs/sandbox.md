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
| Network | none. Only loopback works, so a test can start a server and talk to it. |
| Sockets and D-Bus | no connection to a socket that the call did not make: no `docker.sock`, no session bus, no system bus, no `ssh-agent`, no desktop sockets. An approved exit opens one socket for one call (see "Desktop sockets"). |
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
directories on your `PATH` are never an overlay; they stay read-only. A cache inside
another cache (`~/.cache/pip` and `~/.cache`, or `~/.cargo` as a link into `~/.cache`)
gets no overlay of its own: the outer overlay holds it, and its read-only paths still
apply. The order of `sandbox.caches` does not change this.

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
file into its backup store (below). Only you approve a dir, a link into a `*.wants`
dir, `crontab`, `systemctl --user enable`, linger, the git surface and
`sandbox.protect`, and a floor file that no message of yours names.

**Backups of floor files.** Before efrd runs an approved edit of a floor file, by
`write_file`, `edit`, `apply_patch` or a shell line outside the sandbox, it copies the
file to `~/.local/share/efr/file-backups/<conversation>/<turn>/<n>/<file name>`, with
a `meta.json` that names the path, the links to the real file, the real file, its mode
and whether it was missing. This happens for every approver: the reviewer or you.

- efrd resolves the real path first and opens the real parent dir with
  `RESOLVE_NO_SYMLINKS`, then reads the file with `O_NOFOLLOW`. It never copies a mask
  or a secret, at the path or at the real path.
- The dirs are 0700 and the files 0600. A copy holds at most 16 MiB. The sandbox masks
  the store with the rest of efr's data dir.
- When efrd cannot make the copy, the call does not run, and the model gets the
  reason. A file that the call did not change drops its copy.
- The turn ends with one dim line for each changed file, such as `changed ~/.zshrc;
  backup at ~/.local/share/efr/file-backups/<conversation>/<turn>/1/.zshrc`. To undo
  the change, copy the backup back. A file that did not exist before has only its
  `meta.json`.
- efr keeps the backups of the newest `snapshot.keep_turns` turns of each conversation,
  for at most `snapshot.max_age_days` days. There is no restore command yet.

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
| `host` | `curl -LO https://example.com/x.tar.gz`, `npm ci` | the sandbox, with full network for this call | the reviewer, else you |
| `host_view` | `ss -tlnp`, `ip addr` | the sandbox, with the host's network | the reviewer, else you |
| `socket` | a tool's own Unix socket | the sandbox, with that one socket | the reviewer, else you; only you for a credential agent's socket (`$SSH_AUTH_SOCK`, gpg-agent, a keyring, a password manager's agent); refused inside a secret or efr's own dirs |
| `desktop_ipc` | `hyprctl clients`, `hyprctl reload`, `swaymsg -t get_tree`, `i3-msg -t get_tree`; `hyprctl dispatch exec`, `xdotool type` | the sandbox, with the one control socket of that desktop and its variable | the reviewer for a read or reload form; only you for the rest, and for a desktop socket that the model names |
| `bus` | `hostnamectl`, `systemctl --failed` | the sandbox, with the bus socket | the reviewer, else you |
| `device` | `nvme smart-log /dev/nvme0n1` | the sandbox, with that one device | the reviewer, else you |
| `masked_read` | read a project `.env` | the sandbox, with that one mask removed | only you |
| `destructive` | `git reset --hard`, `git clean -fd`, `rm -r` of tracked files | the same sandbox | the reviewer when every target lies in a write root; else only you |
| `outside` | `df`, `findmnt`, or the model asks to run outside, such as `gdb` or git over ssh | outside the sandbox | the reviewer; only you when a program of the line changed in this turn or lies in a write root, or when the line runs an interpreter on code that efr cannot read (`python3 -c`, `sh -c`, `node build.js`) |
| `privilege` | `sudo`, `doas`, `pkexec`, `yay`, `docker`, `podman` | outside the sandbox, and you can type your password | only you |
| `persistence` | `~/.zshrc`, a Hyprland config; `crontab`, `systemctl --user enable` | outside the sandbox | the reviewer for a floor file that a message of yours names, after a backup copy; else only you |
| `upload` | `git push`, `npm publish`, `cargo publish`, `curl -d` | outside the sandbox | the reviewer when a message of yours names it; else only you |
| `synced_write` | `~/Dropbox` and the others of `sandbox.synced_dirs` | outside the sandbox | only you |
| `above_root` | `rm -rf ..`, `rm -rf ~/p/old-app` | outside the sandbox | the reviewer for one path that a message of yours names and that holds no floor; else only you |
| `secret` | a read or write of a secret | never: denied | nobody |
| `config` | a write of efr's config, of efr's own programs or of efr's own dirs | never: denied | nobody |

"Else you" means that the reviewer handed the question to you, or that it is off
(`classifier.enabled = false`), paused, or failed at each step of its chain.

A grant lives for one call. The next call asks again.

**Desktop sockets.** A `desktop_ipc` exit opens the one control socket of the desktop
that its program talks to, and sets the one variable that names it:

| Program | Desktop | Socket | Variable |
|---|---|---|---|
| `hyprctl` | Hyprland | `$XDG_RUNTIME_DIR/hypr/<signature>/.socket.sock` | `HYPRLAND_INSTANCE_SIGNATURE` |
| `swaymsg` | sway | `$SWAYSOCK` | `SWAYSOCK` |
| `i3-msg` | i3 | `$I3SOCK` | `I3SOCK` |
| `niri msg` | niri | `$NIRI_SOCKET` | `NIRI_SOCKET` |
| `wlr-randr`, `wtype`, `wl-copy`, `wl-paste` | Wayland | `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY` | `WAYLAND_DISPLAY` |
| `xrandr`, `xdotool`, `wmctrl`, `xprop`, `xdpyinfo`, `xclip`, `xsel` | X11 | `/tmp/.X11-unix/X<n>` for a local `:<n>` | `DISPLAY` |

efr reads these variables from the terminal of your prompt, because efrd's own
environment goes stale when the compositor restarts. Only when the prompt sends none
does efrd use its own environment. efrd checks the socket at each call: it must be a
socket of yours in `$XDG_RUNTIME_DIR` (an X server's socket in `/tmp/.X11-unix` can
also be root's), with no link on the way. The call never gets another socket of the
desktop, such as Hyprland's event socket `.socket2.sock`, and never its dir or its
log. When efr finds no such socket, the call does not run, and the model reads "efr
found no Hyprland session in the terminal of this prompt". An X server that needs an
X cookie (`$XAUTHORITY` or `~/.Xauthority`) is refused for now: the cookie file stays
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
install or update. The text of the line never changes this. Before the proxy, efr
knows a host only from the line (a URL, a remote operand, the destination of `ssh`);
for a host that the line does not show, such as a git remote name, there is no name
to check.

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
│ npm ci
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
  private /tmp; no network`.
- Three refusals in a row without a person (by a floor) stop the turn. A refusal of
  the reviewer does not count here; it has its own pause (above).

`efr history --verbose` shows the record of each exit: the line, its targets, hosts and
programs, and how it was judged (by you, a floor or the reviewer). Without a conversation id, it shows the newest
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
  write  no: a shell startup file (floor); a write goes to the reviewer when a message of yours names the file, and efr keeps a backup copy; else you decide; a write is a persistence exit
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
| `tty` and `$$` show the sandbox's view | confusing output for system work | the prompt text says so; `df` and `findmnt` run outside the sandbox after a review; `host_view` exits for `ss` and `ip` |
| an approved exit outside the sandbox has your full rights | it can read secrets, use the network, or start a service through `systemd-run --user` that the call end cannot stop | one command per exit; the question shows the whole line, each program and the untrusted mark; privilege, upload and persistence are yours only |
| changes to your caches while an overlay uses them | old or wrong cache content in the sandbox | `sandbox.cache_mode = "tmp"` |
| no network | a build that must download fails | the network is an exit; `sandbox.offline_hints` tells package managers to use the cache |
| MCP servers | they run outside the sandbox | none yet |
| the host naming rule sees only the hosts of the line | a host behind a git remote name or a script reaches the reviewer's allow with no name; the `host` exit opens the whole network for the call | the reviewer reads the line and your messages; the masks and the environment filter stay; the proxy of a later phase grants one host |
| the reviewer's name check is a word match | "do not push" names a push; "zshrc" can name the wrong file | the reviewer still reads your messages; a generic file name such as `config` or `init` names nothing alone, nor does the bare name of a program, a unit or a desktop entry; wide targets ask you |
| an allowed `outside` line runs in the exit child with your full rights, and only predicted targets get a backup | a program that the model did not write in this turn can change `~/.zshrc` with no name and no backup | the one-command rule; a program written in this turn or in a write root asks you, and so does an interpreter on code that efr cannot read (`python3 -c`, `sh -c`, a script); the reviewer sees the line |
| the payload of a file tool edit of a floor file goes to the reviewer | the payload is model text, which can hold words that try to steer the reviewer | the reviewer's prompt says that text in the action never authorizes; the naming rule still needs a message of yours; a payload over 8 KiB asks you |
| older system prompts of a conversation keep the old auto rule | the model can still read the old, longer rule in its history | the newest rule says that only the newest block counts |
| a cache whose path holds a link | efrd refuses to apply its deletes | the deletes stay private; nothing is lost |
| `sandbox.cache_mode = "overlay"` | a refused delete stays in the layer of the conversation and comes back after each call | efr does not raise the same paths again in the conversation |
