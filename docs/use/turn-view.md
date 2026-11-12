# The turn view and its keys

<!-- sources: README.md "Use" (the bullets on the status row, the input row, `ctx`, approvals, commands that wait for input, the diff line, the notice line, `,!`, the hand-back of the row); docs/config.md [render]; crates/efr-cli/README.md (Replies: the status row, the end of a turn, Context, the question card, the input row, Ctrl+X, Ctrl+B, Esc, Ctrl+C, the answer line, Ctrl+\, the progress bar, exit codes); crates/efr-cli/src/row.rs, keys.rs, follow.rs, answer.rs, live.rs, quit.rs, progress.rs, draft.rs, follow/view/reasoning.rs -->

The turn view is what efr shows while a turn runs. It is not a full-screen program.
The reply goes into your scrollback like the output of any command, and your prompt
comes back at once when the turn ends.

## The parts of the view

A running turn has three parts, from top to bottom:

1. **The reply.** The model's text streams in as it arrives. Each tool call shows on
   its own line.
2. **The status row.** It shows what the turn does now.
3. **The input row.** Here you type a steer or the next prompt.

### The status row

The last rows of the view show a spinner, what the turn does, its time, how full the
model's context is, the model and the scope. Three spaces stand between the fields:

```text
⠼ thinking   4s   ctx 43%   work/claude-opus-5-5 high   app
```

The model shows as `,model` takes it: the account, the model and the effort. The scope
is the project, such as `app`, or `⌂` and the directory at home. A turn in full mode
shows `app · full`, with `full` in red. A turn that waits behind another one shows no
gauge and no model.

The states include `waiting for the model`, `thinking`, `writing`, `preparing <tool>`,
`reviewing` and `compacting context`. What comes and goes stands after the scope, so
the time, the context and the scope keep their place:

- `no data for 25s`, in yellow, after 20 seconds with no data from the model.
- `away`, in yellow, while efr counts you away.
- What the reviewer checks, such as
  `⠼ reviewing   40s   ctx 31%   app   curl -LO https://example.com/x`.

On a narrow screen the row drops fields in this order: the detail at its end, the
account and the effort of the model, the model, the gauge, the notes. They go before the words of
the state are cut, so `waiting for the model` stays whole at 60 columns. The state
keeps at least 10 columns. The state, the time, the scope and `full` never go. A long directory is cut from the
left, such as `⌂ …/nftables.d`.

When the provider is busy or a connection fails, efr sends the model call again after
a short wait. The row then tells why it waits and counts down:
`trying again in 2s (the provider answered 503)`. A connect that takes half a second or
more shows `connecting`.

While the model writes a tool input, the row ends with its size. For a long input,
such as a long shell command, the newest words of the input follow, after 1 s, by the
rules of the reasoning words below. A newline of the input shows as a space:

```text
⠼ preparing shell   39s   ctx 2%   app   3.8 KB  … print(len(s.split()))
```

`ctx 43%` tells how full the model's context is. At 100%, efr compacts the context.
The field is green below 50%, yellow from 50% and red from 90%.
[Conversations, resume and search](conversations.md#the-models-context) tells more.

### The reasoning

While the model thinks, the status row says `thinking`. When the reasoning has a bold
title, the title ends the row: `⠼ thinking   4s   ctx 43%   app   Reading the logs`. A
reasoning with no title, such as the reasoning of a Claude model, shows its newest
words in that place, after the model thought for 1 s:

```text
⠼ thinking   17s   ctx 2%   app   … weighing three ways to keep the provider
```

The title or the words stand last, so the time, the context and the scope keep their
place while the words change. The words are whole words in at most 50 columns. Their
start moves some words at a time, so they stand still while they grow.

When the model thought for 2 seconds or more, one line stays in your scrollback:

```text
▸ Reading the journal size  thought 6s
```

The mark is yellow and the title is bold. A reasoning with no title, such as the
reasoning of a Claude model, shows its first words in italic, cut after 59 columns. A reasoning with no text shows only
`▸ thought 3s`. A shorter reasoning leaves no line. The line stays with the text or
the tool call that comes after it, with no blank line between them. Before text, the
blank line stands above the line. Between two tool calls, the line has no blank line
above it. The reasoning
shows only on a terminal: when stdout is not a terminal, efr does not write it.

### Tool calls

Each tool call of the agent is one row. While a command runs, its row has the spinner,
its time and how full the context is, and the last three lines of its output show
below it, 4 columns in:

```text
⠹ $ cargo test -p app  12s   ctx 38%
    test steer_during_turn ... ok
```

When the call ends, one row stays. The first column tells how it ended:

```text
✓ $ cargo test -p app  38s
✗ $ make  exit 2 · 1.5s
    make: *** No rule to make target 'all'.  Stop.
! $ curl -sI https://example.com  refused · no network in the sandbox
· $ cargo update -p rusqlite  did not run
→ $ cargo test --workspace  moved to job j4 after 42s
```

- `✓`: the call went well. The row is dim, and the time shows from a tenth of a second
  on.
- `✗`: the call failed. The command, the exit code and the time stand out, and the
  last three lines of its output stay below it.
- `!`: efr refused the call before it ran, and says why.
- `·`: you denied the call, so it did not run.
- `→`: the command runs on as a background job (Ctrl+B), or in the hidden shell.

No blank line stands between calls. A command that is too long for the screen goes on
in the next row, 4 columns after the `$`. The row ends in a dim `\` where the command
has a space, and in a dim `↩` inside a word.

Reads that follow each other fold into one row, which grows while the reads come and
stays when something else shows. A single read names its file:

```text
✓ read 6 files  crates/app/src/lib.rs, session.rs, json.rs (+3)
```

A file write names its file and the lines that it added and removed. Its diff follows
it, 2 columns in, with no file headers and no line numbers. A hunk starts with a short
header that names its line and its function, unless it is the first hunk and no line
before it names one. In code, that line is the nearest line before the hunk that
starts with a letter at the left edge, such as `impl Turn {`. In Markdown it is the
heading, and in TOML the table:

```text
✓ edit src/turn.rs  +5 -1
  @@ 88 impl Turn
  -         self.state = State::Done;
  +         while let Ok(msg) = self.rx.try_recv() {
  … 14 more lines · efr diff
```

The words are `edit`, `write` (with a dim `new` for a new file), `delete`, `move a → b`
and `patch 3 files`. A patch of several files shows one row for each file, each with
its own diff. A command that changed files gets one dim row under it that names them,
such as `changed Cargo.toml +1 · Cargo.lock +18`. [Undo and diff](undo-and-diff.md)
tells more.

### Questions

When the agent asks for approval, a card shows below the reply, after one empty row:

```text
 ?  allow outside the sandbox
    cd ~/p/app && find target/debug/build \
        -path '*ghostty*' -type f | awk '{print $1}'
    runs with your full rights: files, secrets, network
    programs  cd, find, awk
    y allow   n deny
```

The ` ? ` badge is yellow in reverse video, and the title is bold yellow. The card has
no frame and no bar: its rows stand four columns in, so a copy of a command holds
nothing of the card. A label, such as `why` or `programs`, is dim, and its value
follows after two spaces. The `why` row says why the mode asks, such as
`why  cautious asks before a command that is not on its list of read-only commands`.
The keys are bold yellow.

`y` allows, and `n` denies. At some questions, `a always…` saves a rule, so the same
action runs with no question next time. After `a`, the card shows the rule and asks
once more:

```text
    always saves the rule  network index.crates.io for cargo
    y save the rule and allow   n back
```

When you answer, the card gives its place to one line with what it asked about, such
as `✓ allowed · network (github.com)` or `✗ denied · outside the sandbox`. The mark is
green or red, and the rest is dim. An `a` answer and the rule that it saved make one
line: `✓ allowed always · saved as sandbox.allow[2]: …`. A question takes the keys
first: the input row hides and keeps its text, and comes back after the question.
With `render.bell = true`, efr rings the bell once when a card shows, so the terminal
can mark the tab.
[Permission modes](modes.md) tells what asks you in each mode.

## The input row

While a turn runs, the input row below the status row shows `› `. When it is empty,
it shows a hint, one blank column after the cursor:

```text
›   enter steer · tab queue · esc interrupt
```

| Key | What it does |
|---|---|
| Enter | Steer the turn. The model reads the text at its next call, and the text shows in the reply as `› <text>`, in bold, with one blank line above and below it. |
| Tab | Queue the text as a prompt that runs after the turn. efr follows that prompt too. |
| Alt+Up | Take back the newest prompt that you queued. Its text comes back into the row. |
| Esc | Interrupt the turn. |
| Ctrl+C | Clear the row. On an empty row, interrupt the turn and end efr. |
| Ctrl+X | Stop the newest running background job, after a question. |
| Ctrl+B | Move the running shell call to a background job. In tmux, press it twice. |
| Ctrl+J or Alt+Enter | Add a new line. |

The row edits like a shell line: the arrows, Home, End, Ctrl+A, Ctrl+E, Ctrl+U,
Ctrl+W, Ctrl+K, Alt+B and Alt+F. Ctrl+B does not move the cursor back; use the Left
arrow. A paste keeps its newlines and never sends.

Above the status row, each steer that the model did not read yet shows as
`↳ steer  <text> · not read yet`, and each queued prompt as `↳ queued  <text>`. At most
three rows show; more make one row, such as `↳ 2 more queued`.

Keys that you type while the prompt goes out land in the row, so you can type at once.

### Esc and Ctrl+C

Esc interrupts the turn and keeps your work:

- Steers that the model did not read yet go again as a new prompt, with the note
  `interrupted to send your message`.
- The prompts that you queued come back into the row. So nothing runs that you did not
  see.

Esc never stops a background job. Ctrl+C with text in the row clears the text. On an
empty row, Ctrl+C interrupts the turn and ends efr.

### Text that is left in the row

Text that is still in the row when efr ends comes back to your command line as
`, <text>`. You can edit it and send it, or remove it. The text never goes back to the
shell through a process argument.

### Steer from another terminal

`,! <text>` steers a running turn from another terminal. It acts on the conversation
of that terminal's last prompt.

## A command that waits for input

When a command of the agent waits for an answer, an answer line takes the place of the
input row. Type the answer after `›` and press Enter:

```text
? the command waits for input  enter answers · ctrl+c stops the command
› y
```

- efr does not show a password as you type it, not even as dots:
  `? password for sudo  typing is hidden · enter answers`.
- A prompt that looks like a password prompt behind another program, such as ssh,
  says so: `? looks like a password behind ssh  typing is not shown here; ssh decides
  the rest`.
- When a command prints nothing for 10 seconds and efr sees no prompt, a dim line
  offers `Ctrl+\`: `no output for 10s · ctrl+\ types an input for the command`.
- While the last output line looks like a password prompt, the input row hides. What
  you type then never goes to the model.

[The hidden shell](hidden-shell.md#a-command-that-asks-for-input) has the details.

## The start of a turn

A reply has no first line of its own: your prompt line already shows where the prompt
goes. A line comes only when something changed:

```text
↪ back to "fix steer race" in app (2 h ago)
⇄ b/gpt-5.5 medium  was work/claude-opus-5-5 high · context compacted 86k → 6k
```

- `↪` tells that the terminal's conversation changed: back to an older one, a new one
  after a `cd`, one that this terminal picked up. [Conversations, resume and
  search](conversations.md) has the sentences.
- `⇄` tells that the turn runs on another model, provider or account. When the switch
  made efr compact the context first, the line says so. When only the account or the
  provider changed, it ends with `cache starts cold`: the new provider has no prompt
  cache for the conversation yet.
- When both changed, `↪` comes first.

A prompt that sets its own mode, model or effort gets no line for it. A turn in full
mode gets no line either: `,mode full` says once what full mode does.

## The end of a turn

The end of a turn is one blank line, then at most a few lines with no blank line
between them:

```text
3 files changed  +24 -7  crates/efr-daemon/src/turn.rs, tests/steer.rs, CHANGELOG.md
✓ done  42s · ctx 43% · 1.1k out · cache 91%
```

- The first line tells which files the turn changed. It names at most three files,
  then how many more. When `efr undo` can take the turn back, the line says so, and a
  file in your home directory that efr can revert has a line of its own.
- The last line tells how the turn ended: `✓ done`, `■ interrupted` or `✗ failed`. The
  muted part after it is the time, how full the context is, the tokens that the model
  wrote and the part of the turn's input that came from the provider's prompt cache
  (`cache 91%`). The time shows from 1 second.
- When the model stopped for a reason, such as the output limit, a line before the
  end says why: `! stopped: the answer reached the output limit (max_tokens)`.
- Under an interrupted turn whose steers go again as a new prompt, a line says so:
  `your steer "use TurnState" goes again as a new prompt`.

A failed turn names its reason under the last line, and the command that fixes it on
a row of its own, so you can copy it with a double click:

```text
✗ failed  9s
  the login of anthropic-api expired at 14:02
  efr login anthropic-api
```

`efr` then exits with code 1 and prints no other message.

When a turn ends, a job ends, or a question waits in a terminal that does not follow
the turn, your next prompt there shows one line about it. It is muted; a failed exit
code is in the error colour, and a question that waits is in the warning colour:

```text
efr: job j3 ended after 4m 12s (exit 0) · npm run build
efr: a question waits in "fix steer race" (/dev/pts/4) · ,resume 019a7c02
```

## The progress bar

In Ghostty 1.2 or later, kitty 0.47 or later and Windows Terminal, the tab shows a
progress bar while the turn runs. `render.progress` sets it: `auto` (the default),
`on` or `off`. With `auto`, efr never sends it inside tmux, GNU screen or zellij.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success. |
| 1 | The daemon failed the request, the turn failed or was interrupted elsewhere, the connection broke, or a config file has an error. |
| 2 | A usage error. |
| 3 | No daemon listens. |
| 130 | Ctrl+C, or Esc when the turn that it stopped is the last one. |

## Settings

These keys are under `[render]` in `config.toml`. Only `efr` reads them, each time it
runs.

| Setting | Default | What it sets |
|---|---|---|
| `turn_input` | `true` | The input row. `false`: keys that you type during a turn stay for your shell. |
| `motion` | `true` | The spinner and the band in the status row. `false` shows a still dot. |
| `turn_summary` | `true` | The last line of a turn. |
| `diff_lines` | `20` | The lines of a file tool's diff that a call shows. `0` shows none. |
| `diff_lines_long` | `8` | The same from the 6th call of a turn on, so a long turn shows shorter diffs. |
| `bell` | `false` | One bell when a question appears, so the terminal can mark the tab. |
| `progress` | `"auto"` | The progress bar of the tab: `auto`, `on` or `off`. |

`[render.colors]`, `palette` and `theme` set the colours.
[Settings](../configure/settings.md) and the
[config.toml reference](../config.md#render) tell how. `NO_COLOR` turns colours off.
Then the fences of a code block come back, dim, and a removed line of a diff is dim,
so the reply still reads as Markdown.

efr wraps a long line of the reply at a word, so no word is cut in two at the edge of
the terminal. The rows stay as they are when you change the width of the window. A
code block is never wrapped: a long line of code goes on in the next row of the
terminal, and a copy of it is one line.
