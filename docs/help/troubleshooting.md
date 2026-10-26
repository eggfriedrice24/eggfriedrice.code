<!-- sources: README.md "Install" (.zshrc note), "Use" (hidden shell), "First use", "Settings"; docs/sandbox.md "When the sandbox cannot run", "Commands", "Known limits"; docs/permissions.md "Full mode", "The config hold"; crates/efr-cli/src/error.rs (exit codes, hints); crates/efr-cli/src/run.rs (message format); crates/efr-transport/src/client/error.rs; crates/efr-cli/src/commands/status.rs, paths.rs, config.rs, sandbox.rs; crates/efr-cli/src/format.rs (status, reloaded, held_text); crates/efr-protocol/src/settings.rs (FullRefusal); crates/efr-daemon/src/reload.rs (config notices); docs/use/phone.md; crates/efr-daemon/src/remote/tailnet.rs (Down), admission.rs (Refusal), listener.rs; crates/efr-daemon/src/error.rs (RemoteOff, NotListening); crates/efr-cli/src/error.rs (the device errors); app/src/ui (the texts of the app) -->

# Troubleshooting

This page tells what to do when efr does not work as you expect. Start with the
commands of the first section. They show most problems.

## Look first

```sh
efr status            # the daemon, the providers, the config and the sandbox
efr paths             # where efr and the daemon keep their files
efr config show       # every setting and where it comes from
efr sandbox check     # every check of the auto sandbox, with a fix for each failure
journalctl --user -u efrd   # the log of the daemon
```

An error of `efr` starts with `efr: `. When efr knows the next step, a second line
names it:

```text
efr: no daemon is listening on /run/user/1000/efr/daemon.sock
efr: start the daemon with: systemctl --user start efrd, or `just run` in the efr checkout for a foreground one
```

## Exit codes

Scripts and the zsh plugin use the exit code of `efr` to tell the cases apart.

| Code | Meaning |
|---|---|
| 0 | The command did what you asked. |
| 1 | The daemon failed the request, the turn failed, or the connection broke. Also: a config file has an error, or `efr sandbox check` found the sandbox not available. |
| 2 | The command line or its input is wrong, such as an unknown mode, model or effort. |
| 3 | No daemon listens on the socket, or the socket path is too long. |
| 130 | You pressed Ctrl+C, or Esc in the input row. |
| 128 + N | Signal N, such as SIGTERM or SIGHUP, ended a turn that efr followed. |

## The daemon

**Exit code 3: no daemon.** Start the daemon:

```sh
systemctl --user start efrd
```

`systemctl --user enable --now efrd` also starts it at each login.
`loginctl enable-linger` keeps it running when you are not logged in.

**The daemon does not answer.** efr says `the daemon is not answering; its log:
journalctl --user -u efrd`. Read the log.

**efr and efrd come from different builds.** efr says `efr and efrd come from
different builds; install both from one build`, or `efrd is older than this efr;
restart it: systemctl --user restart efrd`. After each upgrade, restart the daemon:

```sh
systemctl --user restart efrd
```

**The socket path is too long.** A socket path can be at most 107 bytes. This can
occur when you set `EFR_HOME`, because the socket is then below it. Set
`EFR_RUNTIME_DIR`, or `EFR_HOME`, to a shorter directory.

**`XDG_RUNTIME_DIR` is not set.** A systemd login sets it. Without it, efr uses
`/run/user/<uid>` when that is your own directory with mode 0700. If not, set
`EFR_HOME` or `EFR_RUNTIME_DIR`.

**The shell and the daemon use different files.** This occurs, for example, when only
your shell sets `EFR_HOME`. Then the daemon reads another config and another database.
`efr paths` warns: `the daemon uses ... for the config root, this shell uses ...`. Give
the daemon the same variable:

```sh
systemctl --user edit efrd
```

## Login and models

**The prompt fails as `unauthorized`.** The provider of `[model]` has no login, or it
refused the key. efr names the login command, such as `efr login openai`,
`efr login openai-api` or `efr login anthropic`. `efr status` shows a line such as
`efr: anthropic-api is not logged in; log in with: efr login anthropic`. A key can
expire. Make a new key and log in again.

**The provider refuses the model.** efr names the fix: `choose another model: ,model
<model> for this terminal, or name = "<model>" under [model] in config.toml, which the
daemon reloads without a restart`. You can also use
`efr config set model.name <model>`.

**The ChatGPT backend refuses the client.** The error shows the backend's message.
`originator` under `[openai]` changes the name that efr sends (`efr` by default). This
key needs `systemctl --user restart efrd`.

**A Claude key that is not scoped to one workspace.** The API refuses each request
with a 400 that names `anthropic-workspace-id`. Make a key that is scoped to one
workspace, or set `[anthropic] workspace_id` and restart efrd.

**A login does not change the provider.** Set `model.provider`, then restart the
daemon. [When a prompt fails](../use/models-and-login.md#when-a-prompt-fails) tells
more.

## The config file

**An error in `config.toml`.** A file with an error changes nothing: the daemon keeps
its old settings. You see the error in three places:

- `efr status` shows a `config error` line.
- The next prompt in each terminal with a recent conversation shows one line, such as
  `efr: config.toml has an error; the old settings stay: <error> (<key>, line <n>)`.
- `efr config check` names the line, the column and the key, and exits with 1.

Fix the file with `efr config edit`. It checks the file when you save.

**A change has no effect.** Some keys apply only after a restart, such as `screen` and
`model.provider`. `efr status` shows them in the `restart needed` line. Run
`systemctl --user restart efrd`. A running turn also keeps its settings until it ends.

**A value does not come from the file.** `efr config show` names the source of each
value. `EFR_LOG`, `EFR_SCREEN` and the flags of `efrd` win over the file.

**`config held`.** A change of `config.toml` that loosens permissions came while a
full mode turn ran. efrd keeps the old settings. `efr status` shows `config held` and
the keys that loosen. If you did not make the change, take it out of the file. Then run
`efr config reload`. [The config hold](../permissions.md#the-config-hold) tells more.

## The auto sandbox

**`auto` runs as `cautious`.** `auto` needs every part of the sandbox. When one part
is missing, efr tells you:

- `,mode auto` warns at once, such as `efr: auto needs the sandbox; turns run as
  cautious: Landlock ABI 6 found; auto needs 9 (Linux 7.1). efr sandbox check shows
  more.`
- Each turn that asked for `auto` starts with a dim line: `auto is not available here;
  this turn runs as cautious: <reason>`.
- `efr status` shows the `sandbox` line, with a `sandbox fix` line below it.

Run `efr sandbox check`. Each failed check has a `fix:` line. The most frequent causes:

| Reason | Fix |
|---|---|
| `bubblewrap is not installed` | Arch: `pacman -S bubblewrap` |
| `Landlock ABI N found; auto needs 9 (Linux 7.1)` | a newer kernel |
| `AppArmor blocks user namespaces for bwrap ...` | an AppArmor profile for bwrap with `userns,`, or set the sysctl to 0 |
| `auto cannot use your home directory as a project` | register a narrower project, such as `efr project add ~/dotfiles` |
| `efr's state dir /tmp/... lies below /tmp or /var/tmp, ...` | keep efr's state dir and `XDG_RUNTIME_DIR` outside `/tmp` and `/var/tmp` |

[When the sandbox cannot run](../sandbox.md#when-the-sandbox-cannot-run) has the full
table and the results on some systems. The daemon runs its probe again after a
failure, so a fix needs no restart.

**A command cannot write where it must.** `efr sandbox explain PATH` tells whether a
command in the sandbox can read and write a path, and why. Register the project with
`efr project add`.

**`network none (proxy self-test failed)`.** The network proxy failed its self-test,
so commands in the sandbox have no network. Run `efr sandbox check`. The log of the
daemon has the error. [No network](../use/network.md#no-network) tells more.

**`bus filter off`.** The line names the reason. Without the filter, system queries
such as `hostnamectl` go to the reviewer, and session bus commands ask you. The fix
depends on the reason:

- `dbus.proxy = false`: set `dbus.proxy = true`.
- `xdg-dbus-proxy is not installed` or `xdg-dbus-proxy is too old`: install
  `xdg-dbus-proxy` 0.1.9 or newer.
- `its self-test failed`: run `efr sandbox check`, and read the log of the daemon.

[The bus filter](../use/network.md#the-bus-filter) tells more.

## Full mode

**`full mode is off`.** Set `permissions.allow_full = true` in `config.toml` yourself,
with `efr config edit` or `efr config set permissions.allow_full true`. The agent
cannot set it.

**`full mode cannot run`.** efrd refuses full mode in these cases:

- `efrd runs as root`,
- `snapshot.enabled is false, so efr undo cannot revert the turn`,
- `undo.enabled is false, so efr undo cannot revert the turn`,
- `shell.program is no zsh, so efr cannot clear sudo's cached password before a call`.

`efr status` shows the reason in the `full mode` line. [Permission
modes](../use/modes.md) tells more.

## The hidden shell

**Your `.zshrc` runs `exec tmux` or an instant prompt.** Each conversation has a
hidden zsh. It reads your `.zshrc` with `EFR_HIDDEN_SHELL=1` set. If your `.zshrc` runs
`exec tmux` or an instant prompt, skip that part when the variable is set.
[Install](../start/install.md#your-zshrc-and-the-hidden-shell) shows how.

**The agent's shell calls fail at once.** A command of the agent still runs in the
hidden shell, and it waits for input. Type `,shell`. It shows the command and the row
that it waits at. Enter answers the wait, Ctrl+C stops the command, and Esc leaves.
Never type a password at your own prompt: zsh runs it as a command and keeps it in
your history. [A busy hidden shell](../use/hidden-shell.md#a-busy-hidden-shell) tells
more.

**A command prints nothing and waits.** When a command prints nothing for 10 seconds
and efr sees no prompt, a line says `no output for 10 s; press Ctrl+\ to type an input
for the command`. Press Ctrl+\ and type the input. efr does not show what you type.
[A command that asks for input](../use/hidden-shell.md#a-command-that-asks-for-input)
tells more.

## The phone

[The phone app](../use/phone.md) tells how to set up the listener and pair a phone.
The log of efrd shows most problems:

```sh
journalctl --user -u efrd | grep -E 'remote|Tailscale|tailscaled'
```

**`efr device add` says `the tailnet listener is off`.** Turn on the listener and
restart efrd:

```sh
efr config set remote.enabled true
systemctl --user restart efrd
```

**`efr device add` says `the tailnet listener does not run now`.** efrd waits for
Tailscale. Run `tailscale status` and make sure that Tailscale runs and is logged in.
The log line `Tailscale is not usable` gives the reason:

| Reason | What to do |
|---|---|
| `tailscaled does not answer` | Start tailscaled. If it uses another socket path, set `remote.tailscaled_socket` and restart efrd. |
| `Tailscale does not run` | Run `tailscale up`. |
| `tailscaled runs in userspace networking mode` | Run tailscaled with a TUN device. efrd cannot listen on a Tailscale address in this mode. |
| `tailscaled is older than 1.100` | Update Tailscale. |
| `this node has no Tailscale IPv4 address` | Let your tailnet give this machine an IPv4 address. |
| `tailscaled names no node or tailnet id` | Log in again with `tailscale up`. |

**The log says `the remote listener could not bind`.** Another program uses the port.
Set another `remote.port` and restart efrd. efrd tries the bind again every few
seconds, so the listener starts when the port is free.

**`efr device add` says `efr device add needs a terminal`.** Run it in a terminal,
not in a pipe or a script. efr must ask you about the phone.

**The terminal cannot show the code.** Make the window as wide as efr says. On a
light background, use `efr device add --invert`.

**The pairing ends with `the pairing expired, or the phone left before the
answer`.** The code is good for 5 minutes, and the phone must wait for your answer.
Run `efr device add` again and scan the new code.

**The app shows "Tailscale is not connected".** Turn on Tailscale on the phone. iOS
runs one VPN at a time, so another VPN app stops Tailscale.

**The phone does not connect, and the log says `a remote peer was refused`.** The
reason in the log line tells why:

- `a node of another user`: the phone uses another Tailscale user. Log in on the
  phone as your own user.
- `a tagged node` or `a node shared from another tailnet`: efrd never admits these.
- `this node is tagged, so no peer is its user's`: this computer is a tagged device.
  Remove the tag of this computer in the Tailscale admin console.
- `this node itself` or `an address of this node`: the connection came from this
  computer. efrd never admits it.

**The app shows "Pair this phone again".** Tailscale on the phone is a new node, for
example after a new install of Tailscale. Pair the phone again with `efr device add`,
and revoke the old device with `efr device revoke`.

**The app says that your computer does not know this phone, or that it removed this
phone.** The device was revoked, or efrd has another database. Pair the phone again.

## Known limits

Some problems are limits of the design, not errors. The sandbox page lists them with
what efr does about each one: [Known limits](../sandbox.md#known-limits).
