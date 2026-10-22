<!-- sources: ARCHITECTURE.md "Shape", "Crate map", "Startup and restart order"; README.md "What it is", "Use", "Settings"; crates/efr-daemon/src/main.rs, screens.rs; crates/efr-sbx/src/main.rs; crates/efr-transport/src; crates/efr-protocol/src/shell_context.rs; shell/zsh/efr.plugin.zsh (header); docs/protocol.md "Summary"; docs/storage.md "Directory layout"; docs/config.md "[shell]" -->

# How efr works

This page shows the parts of efr and how a prompt moves through them. You do not need
it to use efr. It helps you understand what you see, and where to look when something
does not work.

## The parts

```text
zsh + efr.plugin.zsh
  |  runs efr for each , line
  v
efr
  |  Unix socket
  v
efrd
  |-- the model provider
  |-- the database
  `-- a hidden zsh for each
      conversation
        `-- efr-sbx (auto)
```

- **The zsh plugin** (`efr.plugin.zsh`) watches your shell. For each `,` line, it runs
  `efr`. It reads your directory and your recent commands, but it runs no git and
  decides nothing.
- **`efr`** is the command line client. It sends a request to the daemon and shows
  the answer, such as a reply that streams. It is a thin relay: the daemon holds the
  state.
- **`efrd`** is the daemon. It runs as the systemd user service `efrd.service`. It
  owns the conversations, the model loop, the hidden shells and the database.
- **`efr-sbx`** is the launcher of the `auto` sandbox. efrd and the hidden shells run
  it. It is never on your `PATH`.

efrd talks to the model API directly. It does not wrap another coding agent.

## The path of a prompt

1. You type `, why does cargo test fail?` in `~/p/app`.
2. The plugin starts `efr`. It gives efr your directory, the terminal, the last command
   with its exit status and the recent commands of the terminal. It puts them in the
   environment of efr, never in its arguments, because any user on the machine can
   read the arguments of a process.
3. efr connects to efrd over a Unix socket.
4. efrd decides the scope of the prompt: the registered project that holds the
   directory, or the scope outside projects. It sends the prompt to this terminal's
   conversation of that scope.
5. The conversation runs a turn. It sends the prompt to the model. When the model calls
   a tool, such as the shell, efrd checks the call against the permission mode and your
   rules. Then it runs the call, runs it in the sandbox, sends it to the reviewer model,
   asks you or denies it.
6. efrd sends each part of the turn to efr as it happens. efr shows it below your
   prompt.
7. When the turn ends, efr exits and your shell prompt comes back.

The turn runs in efrd, not in efr. When a turn ends in a terminal that does not follow
it, the next prompt there shows one line about it.

## Conversations and scopes

A conversation is the history of your prompts, the answers and the calls of the agent.
efrd keeps it in its database.

A terminal has one conversation for each registered project that you worked in, and
one for all places outside projects. A registered project is a directory that you
listed with `efr project add`. The scope of a turn decides the conversation and what
the agent can write without a question.
[Conversations, resume and search](../use/conversations.md) tells more.

What the agent can change depends on the target path, not on the directory where you
stand. In `cautious`, a write in the registered project of the turn runs with no
question. A write to a file in your home directory, outside the project, asks you. It
asks also when you send the prompt from inside the project.

## The hidden shell

Each conversation has its own hidden zsh. It is a real zsh with a terminal engine
behind it, so the agent sees a rendered screen, not raw escape codes. efrd uses the
engine of Ghostty (libghostty-vt) when the build has it. If not, it uses the vt100
engine. The [`screen`](../config.md#top-level) key of `config.toml` can choose vt100. `efr status` shows which one runs.

The hidden shell stays between calls, so a `cd` of one call holds for the next call.
When a command asks for input, efr shows the question below the reply, and you answer
it there.

efrd closes a hidden shell that waited at its prompt, with no input, no output and no
terminal that follows it, for 60 minutes
([`shell.idle_minutes`](../config.md#shell); 0 keeps it). A hidden shell ends when
efrd stops. [The hidden shell](../use/hidden-shell.md) tells more.

## Permissions

Each turn runs in one of four permission modes:

- `manual`: everything asks you, except what your rules allow.
- `cautious` (the default): reads, read-only commands and writes in the project and in
  `$SCRATCH` run with no question. Other actions ask you.
- `auto`: each command runs in a kernel sandbox. An action that leaves the sandbox goes
  to a reviewer model or to you.
- `full`: commands run as you, with no sandbox. efr asks only before a few actions,
  such as `sudo` or a delete of `~`. You must allow this mode in `config.toml` first.

[Permission modes](../use/modes.md) tells more.

## Where efr keeps its files

efr has four roots:

- **config**, `~/.config/efr` by default: `config.toml` and `projects.toml`.
- **data**, `~/.local/share/efr` by default: the database, the scratch directories,
  the snapshots for undo and the logins.
- **state**, `~/.local/state/efr` by default: the logs, the model catalog and the state
  of the sandbox.
- **runtime**, `$XDG_RUNTIME_DIR/efr` by default: the socket, the notices for each
  terminal and the files of each call.

`EFR_CONFIG_DIR`, `EFR_DATA_DIR`, `EFR_STATE_DIR` and `EFR_RUNTIME_DIR` move one root.
`EFR_HOME` moves all four. `efr paths` shows each root and where it came from.
[Settings](../configure/settings.md) and [Storage](../storage.md) tell more.

## The protocol

efr and efrd speak one protocol over the socket `daemon.sock` in the runtime root
(`$XDG_RUNTIME_DIR/efr/daemon.sock` by default).
The socket has mode 0600, so only you can connect to it. Each frame is a 4-byte length
and one JSON object. Each side first sends its protocol version. When the versions do
not match, efr tells you that efr and efrd come from different builds.
[Protocol](../protocol.md) tells more.

## When efrd starts

efrd does these steps in this order:

1. It takes a lock, so that only one efrd runs for each data root.
2. It loads `config.toml`.
3. It copies the database to a backup, then it updates the database to the new
   version.
4. It cleans up what the last efrd left. It cancels a turn that ran, and a question
   that waited expires. A prompt that waited in the queue does not run, and its
   terminal gets a notice to send it again. Nothing continues on its own.
5. It starts the providers, the tools and the shells. It checks whether the `auto`
   sandbox can run on this machine.
6. It opens the socket.
7. It tells systemd that it is ready.

While efrd restarts, efr finds no daemon. It exits with code 3, and you can send the
prompt again. A restart ends every hidden shell, and the commands that run in it.

## Next steps

- [Permission modes](../use/modes.md) tells what the agent can do in each mode.
- [The sandbox](../use/sandbox-basics.md) tells how the `auto` mode works.
- [Architecture](../internals/architecture.md) is the map of the code.
