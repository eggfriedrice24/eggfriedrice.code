# efr

efr is a terminal-first AI agent harness. It lives inside your shell and it manages the whole machine, not one project directory.

## Why this exists

Every coding agent I used has the same shape. You open it inside a project directory, and that directory becomes its world. It can reach the rest of the machine, but only through permission prompts or extra configuration, and it treats each step outside as an exception. That shape is correct for a coding tool. It is wrong for the way I use a terminal.

I live in the terminal. I want to type a line to an agent in the same shell where I run `cd`, `git` and `systemctl`. I want that agent to know where I am, to see what I see, and to work on my dotfiles, on `/etc`, or on a project, with the same rules in each case. No harness did this, so this one exists.

## What it is

- **It lives in zsh.** A line that starts with a comma goes to the agent, exactly as you typed it: zsh never reads it as shell syntax, and history keeps it as typed. Comma alone, or Ctrl+Space, switches to sticky agent mode. Everything else is your normal shell.
- **A daemon owns the state.** It runs as a systemd user service. It holds the conversations, the hidden shells and the memory. Close the terminal, open it again, and nothing is lost.
- **The agent runs commands in real shells.** Each conversation has a persistent hidden zsh with a terminal engine behind it, the same engine that Ghostty uses. The agent sees rendered screens, not raw escape codes. When a command asks for a password or a `[Y/n]` answer, efr asks you below the reply. A password is not echoed, so it reaches the agent only if the program that asked prints it.
- **Scope follows you, permissions follow the path.** The shell sends the current directory with each message. The daemon decides the scope for each turn: the machine, a path, or a project. What the agent may change depends on the class of the target path, never on where you happen to stand.
- **The usual features, done once.** MCP servers, skills in the Agent Skills format, and a memory that keeps one fact per file with an index, loaded from the places that Claude Code, OpenCode and Codex already use.
- **Remote from a phone, over your tailnet.** No relay, no third party. The daemon checks the tailnet identity of each connection and a device key that you approve from the shell.

The daemon owns the model loop. It talks to the model API directly. It does not wrap another coding agent.

## Status

Pre-alpha. The first milestone runs: the daemon with its event log, the model providers (the OpenAI subscription, an OpenAI API key and Anthropic's Claude models with an API key), the shell tool, the permission rules, the zsh plugin and the `efr` CLI. efrd can serve a phone over Tailscale, and the iPhone app in `app/` is a preview that you build yourself. MCP, skills, memory and attaching to a hidden shell come in later milestones.

## Docs

The guide to install and use efr is in [`docs/`](docs/). mdBook builds the same pages into the docs site at <https://eggfriedrice24.github.io/eggfriedrice.code/>. The site starts with the first release that has it.

- [Install](docs/start/install.md), [Your first prompt](docs/start/first-prompt.md) and [How efr works](docs/start/how-efr-works.md)
- [Permission modes](docs/use/modes.md), [The sandbox](docs/use/sandbox-basics.md) and [The network proxy](docs/use/network.md)
- [Conversations, resume and search](docs/use/conversations.md), [Background jobs](docs/use/jobs.md), [The hidden shell](docs/use/hidden-shell.md), [Undo and diff](docs/use/undo-and-diff.md) and [The turn view and its keys](docs/use/turn-view.md)
- [Models, providers and login](docs/use/models-and-login.md)
- [Settings](docs/configure/settings.md) and the [config.toml reference](docs/config.md)
- [Troubleshooting](docs/help/troubleshooting.md) and the [FAQ](docs/help/faq.md)

## Install

On Arch Linux, install a package from the AUR. `efr-code` builds from source, and `efr-code-bin` installs the release build. Then do the three steps as your own user:

```sh
yay -S efr-code                       # or: yay -S efr-code-bin
echo "source /usr/share/zsh/plugins/efr/efr.plugin.zsh" >> ~/.zshrc
systemctl --user enable --now efrd    # start the daemon now and at every login
efr login openai                      # log in to your ChatGPT plan in a browser
```

From a clone of this repository (it needs `just`, rustup and Zig 0.17.0):

```sh
just install                          # efrd and efr to ~/.local/bin, efr-sbx to ~/.local/lib/efr, the user unit
systemctl --user enable --now efrd    # start the daemon now and at every login
echo "source $PWD/shell/zsh/efr.plugin.zsh" >> ~/.zshrc
efr login openai                      # log in to your ChatGPT plan in a browser
```

Install `bubblewrap` for the `auto` mode, and `fzf` 0.46 or newer for the live search of `,resume`. After an upgrade, run `systemctl --user restart efrd`. `efr status` shows the daemon and the providers that you logged in to.

To use an OpenAI API key, or Anthropic's Claude models with an API key:

```sh
efr login openai-api                  # type the key at a prompt that does not show it
printenv ANTHROPIC_API_KEY | efr login anthropic
```

[Install](docs/start/install.md) and [Models, providers and login](docs/use/models-and-login.md) tell more.

## Use

```text
~/p/app $ , why does cargo test fail?
```

- `, <prompt>` sends a prompt with the current directory, the terminal, the last command and the recent commands of the terminal. The reply goes into your scrollback, and your prompt comes back when the turn ends.
- `,` alone, or Ctrl+Space, switches sticky agent mode on: every line goes to the agent until you type `,` alone again.
- While a turn runs, Enter in the input row steers it, Tab queues a prompt, and Esc interrupts it.
- `,new` starts a new conversation, `,resume` finds an older one, and `efr conversation delete` deletes one.
- `,mode`, `,model` and `,effort` choose the permission mode, the model and the effort for one terminal.
- `efr diff` shows what the last turn changed, and `efr undo` takes it back.

[Your first prompt](docs/start/first-prompt.md) tells more.

## Permission modes

| Mode | What runs with no question |
|---|---|
| `manual` | Only what your own rules allow. |
| `cautious` | The default: reads outside secrets, writes in `$SCRATCH` and in the turn's registered project, and read-only commands. |
| `auto` | Every shell command, in a kernel sandbox (bubblewrap, Landlock and seccomp). A reviewer model decides most actions that leave the sandbox. |
| `full` | Every command, as you, with no sandbox. You must allow it in `config.toml` first. efr still asks before `sudo` and a few other actions. |

Register a project with `efr project add`, so that a turn there can write it. `efr sandbox check` tells whether `auto` can run on this machine. [Permission modes](docs/use/modes.md) tells more. [`docs/permissions.md`](docs/permissions.md) and [`docs/sandbox.md`](docs/sandbox.md) have every rule.

## Settings

All settings are in one optional file, `config.toml` in the config root (`~/.config/efr/config.toml` by default). `efr config edit` opens it, and `efr config set` and `efr config unset` change one key. The daemon reloads the file when it changes. [Settings](docs/configure/settings.md) tells more, and [`docs/config.md`](docs/config.md) lists every key with its default.

## When something fails

- Without a daemon, `efr` exits with 3 and says to run `systemctl --user start efrd`.
- Without a login, the first prompt fails as `unauthorized`, and `efr` names the login command, such as `efr login openai`.

[Troubleshooting](docs/help/troubleshooting.md) tells more.

## Repository map

| Path | Content |
|---|---|
| `ARCHITECTURE.md` | The crate map, the dependency rule and the threading model |
| `CONVENTIONS.md` | How code in this repository is written |
| `docs/` | The pages of the docs site: the guide to install and use efr, the settings, help, and the internals: protocol, storage, permissions, the auto sandbox, the providers and their logins, the config reference and schema (generated by `cargo xtask config-docs`) and the ghostty pin; `docs/adr/` holds the decision records, which only GitHub shows for now |
| `book/` | The mdBook config and the theme of the docs site; `just docs` checks the links and builds it into `target/book` |
| `crates/` | The Rust workspace, one crate for each bounded context |
| `shell/zsh/` | The zsh plugin |
| `systemd/` | The user unit for the daemon |
| `packaging/` | The AUR packages `efr-code` and `efr-code-bin`; `docs/releasing.md` tells how a release reaches them |

## Building

Rust 1.99 is pinned in `rust-toolchain.toml`; rustup installs it. `cargo build` and `cargo test` work without Zig. Only the ghostty screen backend needs Zig 0.17.0, and only release builds enable it, `just install` among them. `just` lists the development recipes. `just test-shell` runs the tests that drive a real zsh, and `just test-shell-ubuntu` runs them as CI's shell job does, in an Ubuntu 24.04 container set up like GitHub's runner (it needs Docker). `just lint` also runs CI's `cargo hack` check of every feature combination without Zig.

## Licence

MIT.
