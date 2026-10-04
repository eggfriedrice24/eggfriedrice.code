# efr

efr is a terminal-first AI agent harness. It lives inside your shell and it manages the whole machine, not one project directory.

## Why this exists

Every coding agent I used has the same shape. You open it inside a project directory, and that directory becomes its world. It can reach the rest of the machine, but only through permission prompts or extra configuration, and it treats each step outside as an exception. That shape is correct for a coding tool. It is wrong for the way I use a terminal.

I live in the terminal. I want to type a line to an agent in the same shell where I run `cd`, `git` and `systemctl`. I want that agent to know where I am, to see what I see, and to work on my dotfiles, on `/etc`, or on a project, with the same rules in each case. No harness did this, so this one exists.

## What it is

- **It lives in zsh.** A line that starts with a comma goes to the agent. Comma alone, or Ctrl+Space, switches the prompt into agent mode. Everything else is your normal shell.
- **A daemon owns the state.** It runs as a systemd user service. It holds the conversations, the hidden shells and the memory. Close the terminal, open it again, and nothing is lost.
- **The agent runs commands in real shells.** Each conversation has a persistent hidden zsh with a terminal engine behind it, the same engine that Ghostty uses. The agent sees rendered screens, not raw escape codes. Interactive prompts work, and you can attach to the shell to type a password yourself.
- **Scope follows you, permissions follow the path.** The shell sends the current directory with each message. The daemon decides the scope for each turn: the machine, a path, or a project. What the agent may change depends on the class of the target path, never on where you happen to stand.
- **The usual features, done once.** MCP servers, skills in the Agent Skills format, and a memory that keeps one fact per file with an index, loaded from the places that Claude Code, OpenCode and Codex already use.
- **Remote from a phone, over your tailnet.** No relay, no third party. The daemon checks the tailnet identity of each connection and a device key that you approve from the shell.

The daemon owns the model loop. It talks to the model API directly. It does not wrap another coding agent.

## Status

Pre-alpha. The first milestone runs: the daemon with its event log, one model provider (the OpenAI subscription), the shell tool, the permission rules, the zsh plugin and the `efr` CLI. MCP, skills, memory, attaching to a hidden shell and the phone come in later milestones.

## Install

```sh
just install                          # efrd and efr to ~/.local/bin, the user unit to ~/.config/systemd/user
systemctl --user enable --now efrd    # start the daemon now and at every login
echo "source $PWD/shell/zsh/efr.plugin.zsh" >> ~/.zshrc
efr login openai                      # log in to your ChatGPT plan in a browser
```

`just install` makes a release build with the ghostty screen backend, so it needs Zig 0.16.0. `~/.local/bin` must be on your `PATH`. `efr login openai` prints a URL to open on this machine and waits until the browser is done; with `EFR_OPEN_BROWSER=1` it opens the URL itself. `loginctl enable-linger` keeps the daemon running when you are not logged in. `efr status` shows the daemon, its screen backend and which providers are logged in.

The daemon starts one hidden zsh for each conversation, and that zsh reads your `.zshrc` with `EFR_HIDDEN_SHELL=1` set. If your `.zshrc` runs `exec tmux` or an instant prompt, skip it when that variable is set.

## Use

- `, <prompt>` sends a prompt with the current directory, the terminal and the last command, and the reply streams below it. A prompt sent while a turn runs waits for it.
- `,` alone, or Ctrl+Space, switches sticky agent mode on: the prompt starts with `efr> ` and every line goes to the agent. `!<command>` runs one shell command, a line that starts with `,` runs that command, and `,` alone switches sticky mode off.
- `,new [prompt]` starts a new conversation in this terminal. Without a prompt, the next `,` line starts it.
- `,! <text>` steers the running turn instead of waiting for it.
- When the agent asks for approval, `y` allows and `n` denies. Ctrl+C interrupts the turn.
- When a turn ends or an approval waits in a terminal that does not follow it, the next prompt there shows one line about it.

The plugin hands the context, the last command and the prompt to `efr` in its environment, never in its arguments, because any user on the machine can read a command line. The same commands work by hand: `efr send <prompt>`, `efr new <prompt>`, `efr send --steer <text>`, `efr status`, `efr history [conversation]`, `efr login openai` and `efr config show`. `efr --help` lists the flags.

## First use

- Without a login, the first prompt fails as `unauthorized`, and `efr` says to run `efr login openai`.
- Without a daemon, `efr` exits with 3 and says to run `systemctl --user start efrd`.
- To try efr without installing it, `just run` starts a daemon in the foreground with throwaway directories. In the shell that you test from, export the `EFR_RUNTIME_DIR` that it prints and put `target/debug` on `PATH` after `cargo build -p efr-cli`; the plugin then shows that daemon's notices too.

## Repository map

| Path | Content |
|---|---|
| `ARCHITECTURE.md` | The crate map, the dependency rule and the threading model |
| `CONVENTIONS.md` | How code in this repository is written |
| `docs/` | Protocol, storage, permissions, the ghostty pin and the decision records |
| `crates/` | The Rust workspace, one crate for each bounded context |
| `shell/zsh/` | The zsh plugin |
| `systemd/` | The user unit for the daemon |

## Building

Rust 1.99 is pinned in `rust-toolchain.toml`; rustup installs it. `cargo build` and `cargo test` work without Zig. Only the ghostty screen backend needs Zig 0.16.0, and only release builds enable it, `just install` among them. `just` lists the development recipes. `just test-shell` runs the tests that drive a real zsh.

## Licence

MIT.
