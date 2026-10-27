<!-- sources: README.md "Install", "Building", "First use"; packaging/efr-code/PKGBUILD; packaging/efr-code-bin/PKGBUILD; packaging/efr-code/efr-code.install; systemd/efrd.service; justfile "install", "run"; shell/zsh/efr.plugin.zsh -->

# Install

efr has three programs and one zsh plugin:

- `efr`, the command that you and the plugin run.
- `efrd`, the daemon. It runs as a systemd user service.
- `efr-sbx`, the launcher of the `auto` sandbox. efrd and the hidden shells run it.
  It is not on your `PATH`.
- `efr.plugin.zsh`, which sends each `,` line to efr.

You can install efr from the AUR on Arch Linux, or build it from a clone of the
repository.

## Requirements

- Linux with a systemd user session. The AUR packages are for x86_64.
- zsh. The plugin works only in zsh.

These programs are optional. efr works without them, but some parts do not:

| Program | What needs it |
|---|---|
| `bubblewrap` and Linux 7.1 or newer | The kernel sandbox of the [`auto` mode](../use/sandbox-basics.md). Without them, `auto` runs as `cautious`. |
| `fzf` 0.46 or newer | The live search picker of [`,resume`](../use/conversations.md). Without it, `,resume` shows a numbered list. |
| `git` | Project roots and the git facts of a turn. |
| `xdg-dbus-proxy` 0.1.9 or newer | Read-only system queries in the `auto` mode, such as `hostnamectl`. |
| `xdg-utils` | `efr login openai` opens the browser itself, with `EFR_OPEN_BROWSER=1`. |

## Install from the AUR

There are three packages. Install one of them:

- `efr-code` builds efr from the source of the release tag. For the build, it
  downloads Zig 0.17.0 from ziglang.org. It does not use the Zig of the Arch
  repositories.
- `efr-code-bin` installs the release build from GitHub.
- `efr-code-git` builds the newest commit of `main`. It gets the changes before a
  release, and it can break between releases. For the build, it downloads the
  Zig and the ghostty commit that `docs/ghostty-pin.md` of that commit names. To
  update it, build it again, for example with `yay -S efr-code-git`.

```sh
yay -S efr-code                       # or: yay -S efr-code-bin, yay -S efr-code-git
```

The packages install these files:

| File | Path |
|---|---|
| `efr` and `efrd` | `/usr/bin/` |
| `efr-sbx` | `/usr/lib/efr/` |
| The zsh plugin | `/usr/share/zsh/plugins/efr/efr.plugin.zsh` |
| The user unit `efrd.service` | `/usr/lib/systemd/user/` |
| An example `config.toml` | `/usr/share/doc/efr-code/examples/` |

Then do the three steps of [Set up efr](#set-up-efr) as your own user. The package
prints them when you install it.

## Install from a clone

You need `just`, rustup and Zig 0.17.0. `rust-toolchain.toml` pins Rust 1.99, and
rustup installs it for you. Only the ghostty screen backend needs Zig. `just install`
makes a release build, and a release build enables that backend.

```sh
just install
```

`just install` copies these files:

| File | Path |
|---|---|
| `efr` and `efrd` | `~/.local/bin/` |
| `efr-sbx` | `~/.local/lib/efr/` |
| The user unit `efrd.service` | `~/.config/systemd/user/` (or `$XDG_CONFIG_HOME/systemd/user/`) |

`~/.local/bin` must be on your `PATH`. `just install` reloads systemd, but it does not
enable or start the unit. The zsh plugin stays in the clone, at `shell/zsh/efr.plugin.zsh`.

## Set up efr

Do these steps as your own user, not as root.

1. Add the plugin to your `~/.zshrc`. For the AUR packages:

   ```sh
   echo "source /usr/share/zsh/plugins/efr/efr.plugin.zsh" >> ~/.zshrc
   ```

   For a clone, run this in the root of the clone:

   ```sh
   echo "source $PWD/shell/zsh/efr.plugin.zsh" >> ~/.zshrc
   ```

2. Start the daemon now and at each login:

   ```sh
   systemctl --user enable --now efrd
   ```

3. Log in to your ChatGPT plan:

   ```sh
   efr login openai
   ```

   efr prints a URL. Open it in a browser on this machine. efr waits until the login
   in the browser is done. With `EFR_OPEN_BROWSER=1`, efr opens the URL itself.

To use an OpenAI API key or Anthropic's Claude models, see
[Models, providers and login](../use/models-and-login.md).

Open a new terminal, or run `exec zsh`, so that zsh loads the plugin. When efr is not
on your `PATH`, the plugin says so when it loads:

```text
efr.plugin.zsh: efr is not on PATH; the , commands stay inactive until it is installed
```

### Keep the daemon running

To keep the daemon running when you are not logged in, run this command one time:

```sh
loginctl enable-linger
```

### Your .zshrc and the hidden shell

efrd starts one hidden zsh for each conversation. That zsh reads your `.zshrc` too,
with `EFR_HIDDEN_SHELL=1` set. If your `.zshrc` runs `exec tmux` or an instant prompt,
skip that part when the variable is set:

```zsh
if [[ -z $EFR_HIDDEN_SHELL ]]; then
  # your exec tmux or instant prompt lines
fi
```

[The hidden shell](../use/hidden-shell.md#your-zshrc) tells more.

## Check the install

```sh
efr status
efr sandbox check
```

- `efr status` shows the daemon, its screen backend and the providers that you logged
  in to.
- `efr sandbox check` runs every check of the `auto` sandbox. For each check that
  fails, it prints the fix, such as `pacman -S bubblewrap`.

When no daemon runs, efr exits with code 3 and tells you to run
`systemctl --user start efrd`. [Troubleshooting](../help/troubleshooting.md) tells more.

## Upgrade

The running daemon keeps the old version until it restarts. After each upgrade, run:

```sh
systemctl --user restart efrd
```

Install `efr` and `efrd` from the same build. When efrd is older than efr, efr tells you
to restart efrd.

## Try efr without an install

In a clone, `just run` starts a daemon in the foreground. It uses new, empty
directories in a temporary directory, and it removes them when you stop it with Ctrl+C.

1. Run `just run` in one terminal. It prints the `EFR_RUNTIME_DIR` of the daemon.
2. In a second terminal, run `cargo build -p efr-cli`.
3. In that terminal, export the `EFR_RUNTIME_DIR` that `just run` printed, and put
   `target/debug` of the clone first on your `PATH`.

The plugin then shows the notices of that daemon too. That daemon has no login, so log
in to it before your first prompt.

## Next step

Send [your first prompt](first-prompt.md).
