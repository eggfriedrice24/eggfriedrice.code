<!-- sources: README.md "Use", "First use"; shell/zsh/efr.plugin.zsh (header, _efr_toggle_sticky, bindkey ^@, the ,word rules, efr_prompt_info, EFR_SCOPE_STYLE); crates/efr-cli/src/commands/send.rs, new.rs; crates/efr-cli/src/format/route.rs; crates/efr-cli/src/error.rs; docs/permissions.md "Permission modes" -->

# Your first prompt

This page tells how to send a prompt, what you see while the agent works, and how to
answer it. It starts after [Install](install.md): the daemon runs, you logged in, and
zsh loaded the plugin.

## Send a prompt

Type a comma, a space and your prompt:

```text
~/p/app $ , why does cargo test fail?
```

The prompt goes to the agent with the context of your terminal:

- the current directory and the terminal,
- the last command and its exit status,
- the recent commands of this terminal (8 by default).

`context.recent_commands` in [`config.toml`](../config.md#context) sets how many recent commands the model
sees, and 0 sends none. A command line that starts with a space never goes to the
model.

zsh never reads a prompt as shell syntax. Quotes, `*` and `$` stay as you typed them,
and history keeps the line as you typed it. A prompt of several lines stays one prompt.
A paste of several lines that all start with `,` sends one prompt for each line. The
first runs, and the others wait behind it in their order. A paste with any other line
stays one prompt.

## What you see

The reply goes into your scrollback, like the output of any command. When the turn
ends, your shell prompt comes back at once.

The first line names the scope of the prompt. In a registered project it is the name of
the project, such as `app`. Outside projects it is `○ no project · ~/notes`. When the
conversation changes, the first line tells you that in place of the scope (see
[Conversations](#conversations)).

While the turn runs, the last row shows a spinner, what the turn does, its time and how
full the model's context is:

```text
⠴ writing  12s  ctx 43%
```

A command of the agent shows on one line with its time, and its last three output lines
below it. When the command ends, one line stays:

```text
$ cargo test -p app  exit 101  6.2s
```

The turn ends with one dim line:

```text
done in 42s, ctx 43% (89k/206k), 1.1k out, cache 91%
```

`cache 91%` is the part of the input of the turn that came from the provider's prompt
cache.

## Answer a question

The default permission mode is `cautious`. In this mode, the agent can read files
outside your secrets and run read-only commands. It writes freely in `$SCRATCH`, its
own scratch directory, and in a registered project. Other actions ask you first.

When the agent asks for approval, press `y` to allow or `n` to deny.

When a command of the agent asks for a password or a `[Y/n]` answer, type it and press
Enter. A password does not show. The agent sees it only if the program that asked
prints it. Read which command asks before you answer. [The hidden
shell](../use/hidden-shell.md) tells more.

## Register a project

A registered project is a directory where the agent can write freely in `cautious`, and
where the `auto` sandbox can write. efr never registers a directory by itself. In a
repository, run:

```sh
efr project add
```

This registers the git work tree that holds the current directory, or the directory
itself. `efr project add PATH` registers PATH,
`efr project list` shows the projects and `efr project remove PATH` removes one.

A prompt in a git repository that is not registered shows a hint, at most one time each
day for each repository. `conversation.repo_hint = false` turns it off:

```text
○ ~/p/tool is a git repo but not a project: writes there ask you. efr project add ~/p/tool registers it.
```

[Permission modes](../use/modes.md) tells what each mode allows.

## Work with a running turn

While a turn runs, your keys go to an input row below it (`› `):

| Key | What it does |
|---|---|
| Enter | Steers the turn. The model reads the text at its next call. |
| Tab | Queues the text as a prompt that runs after the turn. |
| Esc | Interrupts the turn. |
| Ctrl+C | Clears the row. On an empty row, it interrupts the turn. |

When you send a prompt while a turn runs, the prompt waits for that turn. [The turn
view and its keys](../use/turn-view.md) lists all the keys.

## Sticky agent mode

Type `,` alone, or press Ctrl+Space, to switch sticky agent mode on. A robot then
stands before the text that you type, and every line goes to the agent. In a
registered project, the project name stands after the robot. Outside projects, `○`
stands there:

```text
🤖 app › fix the failing test
🤖 ○ › check the disk space
```

In sticky mode:

- `!<command>` runs one shell command.
- A line that starts with `,` runs that `,` command.
- `,` alone switches sticky mode off.

When you press Enter, the robot becomes the first word of the line. History keeps
`🤖 <prompt>`, and the line goes to the agent again when you recall it.

### The scope in your own prompt

efr never changes your prompt. To show the scope of the next prompt in your own
prompt, use the function `efr_prompt_info`. It prints `app` or `○`:

```zsh
setopt prompt_subst
RPROMPT='$(efr_prompt_info)'
```

The variable `$efr_scope` holds the same text, for a prompt with no subshell:
`RPROMPT='${efr_scope}'`. Both write each `%` of a project's name as `%%`, and each `!`
as `!!` when you set `prompt_bang`, so the prompt shows the name as you typed it. `EFR_SCOPE_STYLE` sets the style of the scope in sticky
mode (`fg=8` by default). Before efrd runs for the first time, there is no scope.

## Commands of the plugin

| Command | What it does |
|---|---|
| `,new [prompt]` | Starts a new conversation for the scope where you are. |
| [`,resume`](../use/conversations.md#continue-an-older-conversation) | Finds a conversation and continues it in this terminal. |
| `,mode`, [`,model`, `,effort`](../use/models-and-login.md#choose-a-model) | Shows or sets the permission mode, the model or the effort of this terminal. |
| [`,compact [focus]`](../use/conversations.md#the-models-context) | Makes room in the model's context now. |
| [`,context`](../use/conversations.md#the-models-context) | Shows what fills the model's context. |
| [`,away`](../use/conversations.md#when-you-are-away) | Tells efr that you are away from this terminal's conversation. |
| `,shell` | Answers or stops a command that still runs in the hidden shell. |
| `,! <text>` | Steers the running turn from another terminal. |

A prompt of one command word alone runs that command. So `, resume` runs `,resume`.
`, resume the build` stays a prompt. `mode`, `model` and `effort` can also have one value
that efr accepts: `, mode auto` runs `,mode auto`.

A `,word` that is not a plugin command, and that zsh cannot run, is a prompt that starts
with the word. `,run make` sends `run make`. Your own `,word` command, function or alias
still runs. A typo of a plugin command, such as `,moed auto`, stays on the line with a
hint. It does not go to the model.

## Conversations

A terminal keeps one conversation for each registered project that you worked in, and
one conversation for all places outside registered projects. A prompt goes to the
conversation of the scope where you send it. The conversation changes when you send a
prompt, not when you `cd`. The first line of the reply tells you when it changes:

```text
↪ back to "fix the tests" in app (2 h ago)
```

[Conversations, resume and search](../use/conversations.md) tells more.

## Send a prompt from a script

The plugin runs `efr` for you. You can also run it yourself:

```sh
efr send "why does cargo test fail?"
efr new "start a new conversation with this prompt"
```

Both take `--mode`, `--model` and `--effort`. `efr send --steer <text>` steers the
running turn. `efr --help` lists all the commands.

## When the first prompt fails

- **`unauthorized`.** The provider of `[model]` has no login. efr names the login
  command, such as `efr login openai`.
- **Exit code 3.** No daemon runs. Start it with `systemctl --user start efrd`.
- **The provider refuses the model.** efr says so. Choose another model with
  `,model <model>` for this terminal, or with `efr config set model.name <model>`.

[Troubleshooting](../help/troubleshooting.md) tells more.
