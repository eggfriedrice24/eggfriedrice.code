<!-- sources: README.md "Why this exists", "What it is", "Status", "Install", "Use", "The full mode", "Settings"; docs/adr/0006-auto-sandbox.md; docs/adr/0015-full-mode.md; docs/providers.md "The reviewer"; docs/permissions.md "Permission modes", "Full mode", "The settings tool", "Config protection"; docs/storage.md "Directory layout"; shell/ (zsh only) -->

# FAQ

## About efr

### What is efr?

efr is an AI agent that lives in your shell. A line that starts with a comma goes to
the agent. Every other line is your normal shell. A daemon, efrd, holds the
conversations and runs the commands of the agent in hidden shells.
[How efr works](../start/how-efr-works.md) tells more.

### Why does efr not start in a project directory?

Most coding agents start in one project directory, and that directory is their world.
That is correct for a coding tool. efr is for work in the whole terminal: your
dotfiles, `/etc` or a project, with the same rules in each case. So the scope follows
you, and the permissions follow the path that the agent acts on.

### Is efr ready for daily use?

No. efr is pre-alpha. The first milestone runs: the daemon with its event log, the
model providers, the shell tool, the permission rules, the zsh plugin and the `efr`
CLI. MCP, skills, memory, attaching to a hidden shell and the phone come in later
milestones.

### Does efr wrap another coding agent?

No. efrd owns the model loop and talks to the model API directly.

### Which models can I use?

- the ChatGPT plan, with `efr login openai`,
- the OpenAI API, with an API key and `efr login openai-api`,
- Anthropic's Claude models, with an API key and `efr login anthropic`.

[Models, providers and login](../use/models-and-login.md) tells more.

### Which systems does efr run on?

The packages are for Arch Linux, from the AUR. You can also build efr from a clone.
The daemon runs as a systemd user service. The plugin is for zsh only. The `auto`
sandbox needs Linux 7.1 or newer with Landlock, unprivileged user namespaces, and
bubblewrap 0.10.0 or newer that is not setuid. Without them, `auto` runs as `cautious`.
`efr sandbox check` shows what is missing. [Install](../start/install.md) tells more.

### Does efr open a full-screen view?

No. The reply goes into your scrollback, like the output of any command. Your prompt
comes back when the turn ends.

## Privacy and safety

### What does the agent see of my terminal?

Each prompt sends the current directory, the terminal, the last command with its exit
status, and the recent commands of the terminal (8 by default). Set
`context.recent_commands` to change the number, or to 0 to send none. A command line
that starts with a space never goes to the model.

### Can the agent see my passwords?

When a command asks for a password, efr asks you below the reply, and it does not show
what you type. The agent sees the password only if the program that asked prints it.
The prompt text comes from the command, so check which command asks before you answer.
In `auto`, efr never types an answer that may be a password into a command in the
sandbox.

### Which permission mode should I use?

- `cautious` is the default. It reads outside secrets, writes in `$SCRATCH` and in
  the turn's registered project, and runs read-only commands with no question. Other
  actions ask.
- `auto` runs each command in a kernel sandbox. Use it where the sandbox can run.
- `manual` asks for nearly everything.
- `full` runs every command as you, with no sandbox, after you set
  `permissions.allow_full = true`. efr still reads each line: it denies secrets and its
  own config, and asks before `sudo`, a process that outlives the turn and a delete of
  `~`, `/` or a project.

[Permission modes](../use/modes.md) compares them.

### Why does `auto` not ask before a build or a test?

A build or a test runs whatever the project's code, its build scripts and its git
hooks do. A list of allowed command lines cannot judge that. So in `auto` the kernel
holds each command and every process that it starts. A command whose effects stay in
the sandbox runs with no question. An action that leaves the sandbox is an exit, and a
reviewer model or you decide it. [The sandbox](../use/sandbox-basics.md) tells more.

### Is full mode safe?

Full mode is a protection against mistakes. It is not a protection against a hostile
model or a prompt injection. efr reads each command line before it runs, but it does
not read a script or a program that the line starts. Such a program can do what you
can do. Where `auto` can run, `auto` is the safer choice. You must turn full mode on
yourself with `permissions.allow_full = true`.
[Full mode](../permissions.md#full-mode) tells more.

### Does the reviewer cost money?

Each review is one more request to your provider. With an API key, you pay for it per
token. With a subscription, it counts against the limits of your plan. A review is
small: it sends only the facts of one action and your messages. Within a turn, the same
action gets the same verdict without a second request. `classifier.enabled = false`
sends no review and asks you about every exit.
[The reviewer](../providers.md#the-reviewer) tells more.

### Can the agent change its own permissions?

Not without you. No tool of the agent can write efr's config directory. The agent
changes settings only with its settings tool, and each change asks you, in every mode.
The tool never turns on full mode. [Settings](../configure/settings.md) tells more.

In full mode, a script that a command starts can write `config.toml`. When such a
change loosens permissions, efrd does not load it. It holds the change until you run
`efr config reload` ([The config hold](../permissions.md#the-config-hold)).

## Your data

### Where does efr keep my conversations?

In the database `efr.sqlite` in the data root, `~/.local/share/efr` by default.
`efr paths` shows where each root is. [Storage](../storage.md) lists every file.

### Does the search of `,resume` store my secrets?

The search index holds your prompts, the answers and the commands that the agent ran.
It never holds the output of a command or the contents of a file. efrd removes the
secrets that it recognises before text goes into the index, but it cannot recognise
every secret. The full text of each conversation stays in `efr.sqlite`, with no
redaction. [What the search finds](../use/conversations.md#what-the-search-finds)
tells more.

### How do I delete a conversation?

`efr conversation delete <id>` deletes it, or Ctrl+D on its row in the `,resume`
picker. efrd deletes its events, its search text, its recordings, its `$SCRATCH`, its
snapshots and its undo trash. The backups that efrd takes before a migration of the
database keep their copy. [Delete a conversation](../use/conversations.md#delete-a-conversation)
tells more.

### Can I keep my settings in a dotfiles repository?

Yes. `config.toml` holds no secrets. Make `~/.config/efr/config.toml` a symbolic link
to the file in your repository. efr keeps the link when it writes the file.
[Settings](../configure/settings.md) tells more.
