# Conversations, resume and search

<!-- sources: README.md "Use" (scopes, sticky scope, repo hint, tty_idle_hours, pick-up, `,new`, `,resume`, the fzf picker, `,away`, `ctx`, `,compact`, `,context`, the commands by hand); docs/config.md [conversation], [compaction], [presence], [resume]; crates/efr-cli/src/cli.rs (history, resume, search, compact, context, away); crates/efr-cli/README.md (the rows of efr history, compact, context, resume, search, away; Context); crates/efr-cli/src/commands/resume.rs, search.rs; crates/efr-cli/src/picker.rs; crates/efr-cli/src/format/route.rs, resume.rs, search.rs and their snapshots; shell/zsh/efr.plugin.zsh (,resume); docs/adr/0019-conversation-search.md -->

A conversation is the record of your prompts, the model's answers and the tool calls
in between. efr keeps your conversations, so you can continue one later, find it by
its text, or start a new one.

## One conversation for each scope

A terminal keeps one conversation for each registered project that you worked in. It
also keeps one conversation for everything outside registered projects: `~`, `/etc`,
or a git repository that is not registered. A directory below a project's root is
that project. [Permission modes](modes.md#registered-projects) tells how to register
a project.

A prompt goes to the conversation of the scope where you send it. The conversation
changes when you send a prompt, not when you `cd`. For example:

1. A prompt in `~/p/app` continues this terminal's `app` conversation.
2. `cd ~` and a prompt go back to the conversation outside projects.
3. `cd ~/p/app` and a prompt continue the `app` conversation again.

A prompt about two projects stays in the conversation of the project where you are.

The first line of each reply names the scope, such as `app` or
`○ no project · ~/notes`. A dim line names each change of conversation:

```text
↪ back to "fix the tests" in app (2 h ago)
↪ new conversation for app (from ~)
```

In sticky mode, the scope of the next prompt stands after the robot: `🤖 app › ` in a
project, and `🤖 ○ › ` outside projects. The status row of a running turn ends with
the scope too.

### A repository that is not a project

A prompt in a git repository that is not a registered project shows one dim line, at
most once a day for each repository:

```text
○ ~/p/tool is a git repo but not a project: writes there ask you. efr project add ~/p/tool registers it.
```

efr never registers a repository by itself. The prompt goes to the conversation
outside projects. `conversation.repo_hint = false` turns the line off.

### When a conversation ends

A conversation of a scope ends after `conversation.tty_idle_hours` (12 by default)
with no activity. Then the next prompt in that scope starts a new conversation. Each
scope of a terminal has its own clock. A background job that still writes keeps its
conversation active. `0` continues a conversation forever.

### A new terminal

A new terminal in a registered project picks up the project's newest conversation
when these conditions are true:

- No open terminal holds the conversation.
- The conversation was active within `conversation.tty_idle_hours`.

The hidden shell of the conversation starts in its last directory, and a dim line
tells you:

```text
↪ picked up "fix the proxy test" in app (25 min ago); ,new starts fresh
```

A conversation is never open in two terminals. A terminal holds a conversation while
its shell runs, or while it follows the conversation. The scope outside projects
never picks up a conversation. A conversation that no terminal held, such as one of
an `efr send` in a script, is never picked up.

`conversation.follow_project = false` keeps one conversation per terminal, in all
scopes.

## Start a new conversation

`,new` starts a new conversation for the scope where you are:

```sh
,new                      # the next , line starts it
,new fix the flaky test   # start it with this prompt
```

The other scopes of the terminal keep their conversations. `efr new <prompt>` does
the same by hand.

## Continue an older conversation

`,resume` shows your recent conversations. The conversations of the current project
come first. Each row has a title, the project, the age, the count of turns, and a mark
when a terminal holds it.

### Without fzf

Without fzf, `,resume` prints a numbered list. On a terminal, efr cuts the titles so
that each row fits:

```text
 1  fix the proxy test     app              25 min ago   6 turns   this terminal
 2  why is the build slow  app              2 h ago      3 turns   open in /dev/pts/4
 3  (no title)             removed project  3 d ago       1 turn
,resume N continues one here
```

`,resume 2` makes conversation 2 this terminal's conversation for its project again.
You can also give a conversation id, or its first 4 characters or more.

The next `,` prompt in that project continues the conversation, with its context. Its
hidden shell starts in its last directory, and its background jobs report here. A
project conversation stays in its project. When you resume it from another
directory, the dim line says where it continues:

```text
↪ resumed "fix the tests" (app, 2 h ago); it continues when you prompt in ~/p/app
```

When another open terminal holds the conversation, efr asks first:

```text
"why is the build slow" is open in /dev/pts/4. Move it here? [y/N]
```

`y` moves it. The next prompt in the other terminal says where it went.
`,resume --take 2` moves it with no question.

### The search picker

With fzf 0.46 or newer on your `PATH`, `,resume` opens a live search picker. The
picker opens below the command line, in 40% of the terminal's height. It never takes
the full screen, and it clears its rows when it closes.

- Before you type, the picker shows the recent conversations, the ones of the current
  project first.
- Type words to search the titles and the text of your conversations. The best match
  comes first.
- The preview shows the matching lines of the selected conversation. It is on the
  right in a wide terminal (about 100 columns or more), and below in a narrow one.

| Key | What it does |
|---|---|
| Enter | Continue the conversation here, as `,resume N` does. |
| Ctrl+O | Print the conversation into your scrollback, as `efr history` does. It moves nothing. |
| Esc | Close the picker. Nothing changes. |

`,resume <words>` opens the picker with the words as its query. Without fzf, it prints
the best matches. `[resume] picker = "list"` in `config.toml` turns the picker off, so
`,resume` always prints the numbered list.

### What the search finds

efrd keeps a search index of your conversations. The index holds:

- your prompts and steers,
- the model's answers,
- the command lines that the agent ran.

It never holds the output of a command, the contents of a file, a diff, the command of
a background job, or a compaction summary.

The rules of a query:

- Every word must match in one prompt, answer or command.
- A `"quoted phrase"` matches as one.
- The last word matches as the start of a word, from 3 characters.
- A query can have at most 256 characters and 16 words and phrases.

Before text goes into the index, efrd removes the secrets that it recognises and puts
`[***]` in their place. It cannot recognise every secret. For example, it misses
prose such as "my password is hunter2". The raw text stays in the conversation's log,
as before.

After an upgrade, efrd builds the index of your old conversations in the background.
Until it ends, a search finds fewer conversations, and the list shows how far the
index is, such as `indexing: 40%`.

The model cannot search your conversations: efrd refuses a search from the model's
side. [ADR 0019](https://github.com/eggfriedrice24/eggfriedrice.code/blob/main/docs/adr/0019-conversation-search.md) records the decision.

### Search by hand

`efr search <words>` prints the same matches as a list, best first. Each row has the
start of the conversation's id, which `efr resume` takes:

```text
rsync the photos to the laptop  app         25 min ago  6 turns   this terminal        019a9b11
    please rsync ~/photos to the laptop and check its disk
efr resume <id> continues one here
```

`--limit N` sets the number of rows (10 by default, 200 at most). `--json` prints the
daemon's answer for scripts.

Other users of the machine can read the arguments of a process. So the words of
`efr search` show in the process list. `,resume <words>` keeps them out of it: the
plugin gives the words to efr, and efr gives them to fzf, in the environment.

## See a conversation

`efr history` lists your recent conversations. `efr history <id>` shows one, with the
mode, model and effort of each turn. The id can be the start of an id, from 4
characters.

- `--verbose` adds the records of each exit from the sandbox, the last lines of each
  job's output, and the tokens of each turn. Without an id, it shows the newest
  conversation of this terminal.
- `--limit N` and `--cursor` page through a long conversation.

`efr history` only shows a conversation. It never moves it to this terminal.

These commands act on the conversation of this terminal's last prompt, also after a
`cd`: `efr history --verbose`, `efr undo`, `efr diff`, `efr jobs`, `efr compact`,
`efr context`, `efr away` and `,!`.

## The model's context

The status row of a running turn shows how full the model's context is, such as
`ctx 43%`. At 100%, efr compacts the context: it writes a summary of the earlier
turns and keeps the newest turns as they are. Then the turn goes on, and one dim
line stays:

```text
context compacted (auto): 231k -> 24k tokens, kept 3 turns, summary 3.2k
```

`[compaction] auto_at` sets the point, as a percent of the model's window (76 by
default). With `[compaction] auto = false`, efr compacts only when you tell it to.

`,compact` compacts the context of this terminal's conversation now, between turns.
It starts no turn. Give a focus to tell the summary what it must keep:

```sh
,compact the failing test
```

`,context` shows what fills the context of this terminal's conversation:

- the model's last count, with its `ctx N%`,
- efr's estimate of the next request by part: the system prompt, the tools, the
  summary, the prompts, the answers, the tool calls and their outputs,
- the five largest items,
- the compactions and the use of the prompt cache.

The sizes are estimates. `,context` shows no text of a message. `,context <id>` shows
another conversation, and `efr context --model <id>` counts for another model.

## When you are away

`,away` tells efr that you are away from this terminal's conversation. A question then
waits only 5 minutes, and its call does not run. The model goes on without it and
names it in its answer. The status row shows `away`.

efr also counts you away after 10 minutes with no input in the conversation. Input is
a prompt, a steer, an answer, or keys for the hidden shell. Your next input ends the
away state.

| Setting | Default | What it sets |
|---|---|---|
| `presence.away_after_minutes` | `10` | The minutes with no input before efr counts you away. |
| `presence.away_wait_secs` | `300` | How long a question waits while you are away. |

## Settings

| Setting | Default | What it sets |
|---|---|---|
| `conversation.tty_idle_hours` | `12` | Hours with no activity before a scope starts a new conversation. |
| `conversation.follow_project` | `true` | One conversation for each scope. `false`: one per terminal. |
| `conversation.repo_hint` | `true` | The line about a git repository that is not a project. |
| `compaction.auto` | `true` | Compact on its own at `auto_at`. |
| `compaction.auto_at` | `76` | The percent of the model's window where efr compacts. |
| `resume.picker` | `"auto"` | `auto` opens the fzf picker when it can. `list` always prints the list. |

[config.toml reference](../config.md#conversation) lists every key.
