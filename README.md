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

Pre-alpha. The repository holds the design and the workspace skeleton. Nothing runs yet. The first milestone is: the daemon with its event log, one model provider, the shell tool, the permission rules and the zsh plugin.

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

Rust 1.99 is pinned in `rust-toolchain.toml`; rustup installs it. `cargo build` and `cargo test` work without Zig. Only the ghostty screen backend needs Zig 0.16.0, and only release builds enable it. `just` lists the development recipes.

## Licence

MIT.
