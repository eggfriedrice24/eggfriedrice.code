<!-- sources: README.md "What it is", "Status"; CHANGELOG.md "[Unreleased]" -->

# efr

efr is a terminal-first AI agent harness. It lives inside your shell. It manages the
whole machine, not only one project directory.

```text
~/p/app $ , why does cargo test fail?
```

A line that starts with a comma goes to the agent. Every other line is your normal
shell. The reply goes into your scrollback, like the output of any command. efr is not
a full-screen program.

## What it is

- **It lives in zsh.** A line that starts with `,` goes to the agent, exactly as you
  typed it. zsh never reads it as shell syntax, and history keeps it as you typed it.
  `,` alone, or Ctrl+Space, switches sticky agent mode on.
- **A daemon owns the state.** efrd runs as a systemd user service. It holds the
  conversations and the hidden shells. When you close a terminal, you lose nothing.
- **The agent runs commands in real shells.** Each conversation has its own hidden
  zsh. The agent sees rendered screens, not raw escape codes. When a command asks for
  a password or a `[Y/n]` answer, efr asks you below the reply.
  [The hidden shell](use/hidden-shell.md) tells more.
- **Scope follows you, permissions follow the path.** The shell sends the current
  directory with each prompt. efrd decides the scope of each turn: the machine, a
  path or a project. What the agent can change depends on the target path, not on the
  directory where you stand.
- **A sandbox for the agent.** In the `auto` mode, each command of the agent runs in a
  kernel sandbox. It can build and test your project with no question. A reviewer
  model decides most actions that leave the sandbox.
  [The sandbox](use/sandbox-basics.md) tells more.
- **No other agent below it.** efrd owns the model loop and talks to the model API
  directly. It does not wrap another coding agent.

## Status

efr is pre-alpha. The first milestone runs. It has the daemon with its event log, the
shell tool, the permission rules, the zsh plugin and the `efr` CLI. It has three model
providers: the ChatGPT plan, an OpenAI API key, and Anthropic's Claude models with an
API key. MCP servers, skills, memory, attaching to a hidden shell and the phone come in
later milestones.

efr runs on Linux with zsh and a systemd user session.

## Where to start

- [Install](start/install.md) efr, start the daemon and log in.
- [Your first prompt](start/first-prompt.md) tells what to type and what you see.
- [How efr works](start/how-efr-works.md) shows the parts and how a prompt moves
  through them.
- [Permission modes](use/modes.md) tells what the agent can do without a question.
- [Settings](configure/settings.md) tells how to change efr, and the
  [config.toml reference](config.md) lists every key.
- [Troubleshooting](help/troubleshooting.md) helps when something does not work.

The [changelog](changelog.md) lists the changes of each release.

## Source

The source code is on [GitHub](https://github.com/eggfriedrice24/eggfriedrice.code),
under the MIT licence.
