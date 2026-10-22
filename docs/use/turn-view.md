# The turn view and its keys

<!-- sources: README.md "Use" (the bullets on the status row, the input row, `ctx`, approvals, commands that wait for input, the diff line, the notice line, `,!`, the hand-back of the row); docs/config.md [render]; crates/efr-cli/README.md (Replies: the status row, the end of a turn, Context, the question card, the input row, Ctrl+X, Ctrl+B, Esc, Ctrl+C, the answer line, Ctrl+\, the progress bar, exit codes); crates/efr-cli/src/row.rs, keys.rs, follow.rs, answer.rs, live.rs, quit.rs, progress.rs, draft.rs -->

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
model's context is, and the scope:

```text
⠼ thinking  4s  ctx 43%  app
```

The states include `waiting for the model`, `thinking`, `writing`, `preparing <tool>`,
`reviewing: <command>` and `compacting context`. After 20 seconds with no data, the row
says so, such as `waiting for the model, no data for 25s`. The row also shows `away`
while efr counts you away, and `full` in red for a turn in full mode.

`ctx 43%` tells how full the model's context is. At 100%, efr compacts the context.
The field is green below 50%, yellow from 50% and red from 90%.
[Conversations, resume and search](conversations.md#the-models-context) tells more.

### Tool calls

A command of the agent shows on one line with the spinner and its time, and its last
three output lines below:

```text
⠹ $ cargo test -p app  12s
```

When the command ends, one line stays, such as `$ cargo test -p app  exit 101  6.2s`.
A failed command also keeps its last three output lines. A file tool shows its diff,
and a command that changed files shows one dim line that names them.
[Undo and diff](undo-and-diff.md) tells more.

### Questions

When the agent asks for approval, a card shows below the reply:

```text
? allow outside the sandbox
│ cd ~/p/app && find target/debug/build \
│     -path '*ghostty*' -type f | awk '{print $1}'
│ runs with your full rights: files, secrets, network
│ programs: cd, find, awk
│ y allow · n deny
```

`y` allows, and `n` denies. At some questions, `a` (always) and then `y` saves a rule,
so the same action runs with no question next time. When you answer, the card gives
its place to one line, such as `✓ allowed` or `✗ denied`. A question takes the keys
first: the input row hides and keeps its text, and comes back after the question.
[Permission modes](modes.md) tells what asks you in each mode.

## The input row

While a turn runs, the input row below the status row shows `› `. When it is empty,
it shows a hint:

```text
› enter steer · tab queue · esc interrupt
```

| Key | What it does |
|---|---|
| Enter | Steer the turn. The model reads the text at its next call, and the text shows in the reply as your message. |
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
`↳ steer: <text>`, and each queued prompt as `↳ queued: <text>`.

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
input row. Type the answer and press Enter.

- efr does not show a password as you type it.
- When a command prints nothing for 10 seconds and efr sees no prompt, a dim line
  offers `Ctrl+\`. Press it to type an input for the command.
- While the last output line looks like a password prompt, the input row hides. What
  you type then never goes to the model.

[The hidden shell](hidden-shell.md#a-command-that-asks-for-input) has the details.

## The end of a turn

A turn that changed files ends with a line such as `3 files changed, +24 -7`. Then one
dim line sums up the turn:

```text
done in 42s, ctx 43% (89k/206k), 1.1k out, cache 91%
```

`cache 91%` is the part of the turn's input that came from the provider's prompt
cache. An interrupted turn ends with `interrupted after 12s`. When the model stopped
for a reason, such as the output limit, one line before the end says why.

When a turn ends, a job ends, or a question waits in a terminal that does not follow
the turn, your next prompt there shows one line about it:

```text
efr: job j3 ended (exit 0): <title>
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
| `progress` | `"auto"` | The progress bar of the tab: `auto`, `on` or `off`. |

`[render.colors]`, `palette` and `theme` set the colours.
[Settings](../configure/settings.md) and the
[config.toml reference](../config.md#render) tell how. `NO_COLOR` turns colours off.
