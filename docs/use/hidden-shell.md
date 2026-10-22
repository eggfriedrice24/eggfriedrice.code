# The hidden shell

<!-- sources: README.md "Install" (the .zshrc note on EFR_HIDDEN_SHELL), "API keys" (the environment of efrd), "Use" (the bullet on commands that wait for input, Ctrl+\, sudo_cache), "A busy hidden shell"; docs/permissions.md "A command left running"; docs/config.md [shell]; crates/efr-shell/README.md "Purpose", "Environment hygiene"; crates/efr-conversation/src/turn.rs (move_shell); crates/efr-daemon/src/shells.rs; crates/efr-cli/src/cli.rs (shell); crates/efr-cli/README.md (the efr shell row, the answer line, the busy hidden shell); shell/zsh/efr.plugin.zsh (,shell); docs/adr/0017-busy-shell-and-call-reach.md -->

The agent runs its shell commands in a hidden shell. This is a real zsh that efrd
starts for each conversation. It reads your startup files, so your aliases, functions
and `PATH` work for the agent too. You do not see the hidden shell's screen. You see
each command and its output in the turn view.

## How the hidden shell starts

efrd starts the hidden shell of a conversation at the first shell call that needs
it. The shell is an interactive login zsh. It reads your
`.zshenv`, `.zprofile`, `.zshrc` and `.zlogin` as a terminal of yours does. When no
zsh is on the `PATH`, efrd uses `/bin/sh`.

The hidden shell keeps its state between calls: the directory, the variables, the
functions and the aliases. When you `cd` between two prompts, your next prompt moves
the hidden shell to your new directory too, and the shell keeps its variables. The
agent then works where you are.

### Your .zshrc

The hidden shell has `EFR_HIDDEN_SHELL=1` in its environment. If your `.zshrc` runs
`exec tmux` or an instant prompt, skip that part when the variable is set:

```zsh
if [[ -z $EFR_HIDDEN_SHELL ]]; then
  # exec tmux, instant prompt and the like
fi
```

The efr plugin itself does nothing in the hidden shell.

### What efr changes in its environment

- Nobody reads a pager in the hidden shell. So `PAGER`, `GIT_PAGER`, `MANPAGER` and
  the other pager variables are `cat`. `git log` and `systemctl status` do not wait
  for you to quit `less`.
- Nobody can use an editor there either. `EDITOR`, `VISUAL`, `GIT_EDITOR` and the
  like name a small script that fails at once:

  ```text
  efr: there is no editor in the hidden shell; pass the text another way, such as git commit -m, a file, or ask the user to edit it
  ```

  So `git commit` without `-m` or `git rebase -i` fails at once and does not wait.
- `SCRATCH` names the scratch directory of the conversation, in every command of the
  agent: in the sandbox, outside it and in a job. A `SCRATCH` that your startup files
  set does not reach the agent's commands. Your own shell does not change.
- efrd gives its own environment to the hidden shell. Do not put an API key in the
  environment of efrd: in `manual`, `cautious` and `full`, a command of the agent can
  read it with `env`. Only the `auto` sandbox removes names such as `*API_KEY*`. Use
  `efr login`. [Models, providers and login](models-and-login.md) tells how.

### When the hidden shell closes

efrd closes a hidden shell after `shell.idle_minutes` (60) with no output and no
input, while it waits at its prompt and nobody watches it. The next call starts a new
shell. The hidden shell also ends when efrd stops, and a command that runs in it ends
too.

## A command that asks for input

While a command of the agent runs, its last output lines show below its line. When
the command waits for an answer, type it and press Enter.

- **A password.** efr does not show what you type. The agent sees a password only if
  the program that asked for it prints it. The prompt text comes from the command, so
  check which command asks before you answer.
- **A plain question**, such as `[Y/n]`, shows as you type it. The agent sees your
  answer if the program shows it.
- **No prompt.** If a command prints nothing for 10 seconds and efr sees no prompt, a
  dim line offers `Ctrl+\`. Press it to type an input for the command. efr does not
  show this input as you type it. Before you press `Ctrl+\`, what you type goes to the
  input row, never to the command.

While the last output line of the command looks like a password prompt, the input row
hides. What you type then is not shown and never goes to the model.

In `auto`, efr never types an answer that may be a password into a command in the
sandbox: such a prompt stops the command.

### sudo and ssh

A command outside the sandbox that may ask for a password, such as `sudo`, `ssh`, or
`rsync`, `scp` or `git` over ssh, can run past the agent's timeout while you follow
the turn. The limit is `shell.interactive_timeout_minutes` (60). The same is true for
any command that waits for input when its timeout passes.

As in any terminal, sudo remembers your password for a few minutes in the hidden
shell. To make it forget the password after each call, set this in `config.toml`:

```toml
[shell]
sudo_cache = "per_call"
```

Each sudo call asks for your approval with either value.

## A busy hidden shell

A shell call answers the agent at its timeout. When its command still runs then, the
command goes on in the hidden shell, such as a slow build, or an `rsync` that asked
for a password after its call answered. The hidden shell takes no other call until
that command ends. efr tells you so:

- The call shows `still runs in the hidden shell` in place of its check mark.
- While you follow a turn of the conversation and the command waits for input, an
  answer line asks for it. It names the command, who approved the call (`you approved
  it`, `your rule allowed it`) and the row of the screen that asks, such as
  `u@h's password:`. Read that row before you answer: it names the program and the
  host that ask. Enter sends the answer. Ctrl+C stops the command, not the turn.
- At the end of the turn, one line names the command:

  ```text
  the hidden shell still runs: rsync -a ... (it waits for a password)
  ```

  When you can answer there, efr stays open with `enter answers · ctrl+c stops it ·
  esc leaves it`. Otherwise the line adds `· ,shell answers or stops it`, and efr
  ends.
- When no terminal showed it, your next prompt shows a notice, such as:

  ```text
  efr: <title>: the hidden shell still runs rsync -a ... and waits for a password (asked by "u@h's password:"; you approved it). Type the password only after ,shell; never at this prompt.
  ```

**Never type the password at your own prompt.** zsh runs it as a command and keeps it
in your history. In sticky mode, it goes to the agent.

### ,shell

`,shell` answers or stops the command. It works in sticky mode too. `efr shell
[conversation]` does the same by hand. It shows the command, how long it runs, who
approved it and the row where it waits, or `the hidden shell is free`. It stays inline
until the command ends.

| Key | What it does |
|---|---|
| Enter | Answer the wait. A password is not shown. |
| Ctrl+C | Stop the command. |
| Ctrl+C again | Ask `close the hidden shell? [y/n]`, for a command that ignores Ctrl+C. |
| Esc | Leave. The command stays as it is. |

### What waits and what stops

- A wait for a password stays while you can answer it. When nobody can answer, it
  stays only for a line that may ask for one (`sudo`, `ssh`, and `rsync`, `scp` or
  `git` over ssh) and that you or your own rule approved. efr stops every other such
  wait at once and tells you:

  ```text
  efr stopped rsync -a ... in the hidden shell: it asked for a password while nobody followed
  ```

- A command that waits for input stops `shell.interactive_timeout_minutes` (60) after
  its start. A command that waits for nothing, such as a slow build, runs until it
  ends.
- While the command waits for input, the agent's next shell call fails at once. The
  agent reads which command holds the shell, and that you answer or stop it with
  `,shell`. It never asks you to type at your prompt.

[A command left running](../permissions.md#a-command-left-running) has the full rules.
To make a long command free the hidden shell, move it to a job with Ctrl+B: see
[Background jobs](jobs.md#move-a-running-command-to-the-background).

## Settings

| Setting | Default | What it sets |
|---|---|---|
| `shell.program` | unset | The shell, as an absolute path. Unset: `zsh` on the `PATH`. |
| `shell.login` | `true` | Start the hidden shell as a login shell. |
| `shell.idle_minutes` | `60` | Minutes with no output or input before efrd closes an idle shell. `0` keeps idle shells. |
| `shell.sudo_cache` | `"keep"` | `per_call` makes the hidden shell forget sudo's password after each call. |
| `shell.interactive_timeout_minutes` | `60` | How long a command that may ask for input can run while you follow the turn. |

[config.toml reference](../config.md#shell) lists every key.
[ADR 0017](https://github.com/eggfriedrice24/eggfriedrice.code/blob/main/docs/adr/0017-busy-shell-and-call-reach.md) records the decision for a busy
hidden shell.

You cannot attach to the hidden shell yet, so a full-screen program does not work
there.
