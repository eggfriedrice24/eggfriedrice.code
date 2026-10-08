# efr-cli

## Purpose

`efr`, the command-line relay to the daemon. The zsh plugin
(`shell/zsh/efr.plugin.zsh`) runs it for every `,` line; people run it for status,
history, login and settings. It parses arguments, calls the daemon over the Unix
socket through `efr-client`, and prints. The daemon decides everything; `efr` keeps no
state and never writes the daemon's database or credentials.

| Command | Protocol | Notes |
|---|---|---|
| `efr send [--context-json <json>] [--last-command <text>] [--conversation <id>] [--mode <m>] [--model <id>] [--effort <e>] [--] [prompt]` | `prompt.send`, then `conversation.subscribe` after the prompt's `seq`; from the input row `turn.steer`, `prompt.send`, `turn.interrupt` and `prompt.withdraw` | follows the turn until it ends, and then each prompt queued from its input row |
| `efr send --steer [--context-json <json>] [--conversation <id>] [--] [text]` | `conversations.list` to find the tty's active conversation, `turn.steer` | `--conversation <id>` skips the lookup; a steer takes no turn settings |
| `efr new [--context-json <json>] [--last-command <text>] [--mode <m>] [--model <id>] [--effort <e>] [--] [prompt]` | `prompt.send` with `new_conversation` | the prompt is required (exit 2 without one); the plugin's bare `,new` sends nothing and makes the next `,` line run `efr new` |
| `efr settings [--mode <m>] [--model <id>] [--effort <e>]` | `models.list`; `admin.status` for `--mode auto` | the mode, model and effort that a prompt with these values would use, one `key = value  # source; choices: ...` line each; a value the daemon would refuse exits 2 with the choices; `--mode auto` with a sandbox that is not available prints a warning on stderr with the reason (turns run as `cautious`) |
| `efr models [--names]` | `models.list` | the daemon's models, `*` before the default, with the efforts of each; `--names` prints only the ids, for completion |
| `efr status` | `admin.status` | says on stderr how to log in when no provider is logged in; shows the config file, its last reload error, the keys that wait for a restart, and the `sandbox` line: `ready (Landlock ABI 10, bubblewrap 0.13.0, caches tmp, network none)` or `unavailable: <reason>; auto runs as cautious` with its fix |
| `efr history [conversation] [--limit n] [--cursor c] [--verbose]` | `conversations.list`, `conversation.history` | a conversation is its id or the start of it (4 characters or more); the lines of each turn show together, and the turns show in the order that they started, not in event order; a prompt that never started (taken back, cancelled or still waiting) shows where it was in the queue, after the turns whose prompts were queued before it, with its note (`withdrawn before it ran`); each turn shows its mode, model and effort as a dim line after its prompt, and the fallback note when `auto` ran as `cautious`; the sandbox's notes, exits, quarantine questions and turn-end reports show as dim lines; each compaction of the model's context shows at its place with the line that the turn showed (see "Context" below); `--verbose` adds the record of each exit (`exit_requested`: its line, targets, hosts, programs and counts, never a user message) and how it was judged, and the focus and the summary of each compaction as dim lines; `--verbose` without a conversation shows the newest conversation of this terminal (the first listed whose `tty` is the terminal on stdin), else the newest of all, after a dim line that says which; `--limit` and `--cursor` then page its events |
| `efr diff [--turn <id>] [--conversation <id>] [--stat]` | `conversations.list` to find the conversation, `conversation.diff` (read scope) | what a turn changed in the files of its project and `$SCRATCH`, from the daemon's snapshots: by default the last turn of this terminal's conversation (the newest whose `tty` is the terminal on stdin), else of the newest conversation, with a dim line on stderr that says which; `--turn` asks for that turn and looks nothing up, `--conversation` takes an id or the start of one; on a terminal the diff is painted in the `diff.*` roles with the files' syntax colours, then a dim `… N more lines` for a diff the daemon cut and a dim `3 files changed, +24 −7`; in a pipe stdout holds the daemon's diff alone (tabs and newlines kept, other control characters as stand-ins), for `git apply` or a pager; `--stat` lists each file with its kind, path and counts, then the totals; an ignored file such as `.env` shows in the list, and the daemon's diff has only a line `<path>: ignored file, content not shown` for it; a turn that changed nothing says so on stderr (exit 0); no conversation, an unknown turn or a conversation with no finished turn (`not_found`) is an error (exit 1) that names the turn |
| `efr compact [--conversation <id>] [focus]` | `conversations.list` to find the conversation, `conversation.compact` (operate scope) | makes room in the model's context now: efrd writes a summary of the earlier turns and keeps the newest ones word for word; it never starts a turn. The focus (the words, else `EFR_PROMPT`; blank is none) says what the summary must keep. By default it compacts this terminal's conversation, else the newest one, with a dim line on stderr that says which; `--conversation` takes an id or the start of one. On a terminal a status row (`⠼ compacting context  4s`) shows while efrd works and goes before the result. Then one line says what came of it, as a turn shows a compaction: `context compacted (efr compact): 140k -> 19k tokens, kept 3 turns, summary 3.2k`, muted on a terminal. A refusal of efrd (`conflict` while a turn runs or when nothing lies before the newest turns) is an error (exit 1) with its message. No conversation is an error (exit 1). Ctrl+C stops the wait, not the compaction (exit 130, with a note) |
| `efr sandbox check` | `admin.sandbox_check` | the daemon runs its sandbox probe now; one line per check (`ok`, `warn`, `fail` with its fix, `skip`), the warnings, the launch cost, then `auto: ready` or `auto: unavailable: <reason>`; exit 1 when it is unavailable |
| `efr sandbox explain PATH` | `sandbox.explain` (read scope) | whether a contained command can read and write PATH (relative to the current directory, which also picks the project), and why: `read yes`, `write no: a shell startup file (floor); a write is a persistence exit, user only` |
| `efr login openai` | `admin.login_openai` (stream) | prints the authorize URL, opens it only when `EFR_OPEN_BROWSER` is on, waits for completion |
| `efr config show` | `admin.status` when the daemon runs | every key of `config.toml` with its value and source, then what `efr` uses (the theme, and for `auto` the background that chose it, the code theme, colour, roots), then the file the daemon reads, its reload error and `restart_needed`, with a warning when the daemon reads another file; as TOML |
| `efr config check [path]` | none | the file checked with the daemon's schema, the theme names, the theme file of `render.palette` and its code theme; an error names its line, column and key; exit 0 or 1 |
| `efr config edit` | `admin.config_reload` | creates a missing file from the commented example (never through a link to nothing), runs `$VISUAL`, else `$EDITOR`, else `vi` (through `sh`, so an editor with arguments works) on the file behind a link when `config.toml` is one, so an editor that saves by replacing the file keeps the link, checks the file, offers to edit again on an error when stdin is a terminal, then asks the daemon to reload |
| `efr config set <key> <value>`, `efr config unset <key>` | `admin.config_reload` | one scalar or list key through `efr-config`'s writer: comments and layout stay, a link stays and its target is written, a value the daemon would refuse is never written; then a reload |
| `efr config schema` | none | the JSON schema of `config.toml` |
| `efr config reload` | `admin.config_reload` | applied, or the file's error (exit 1), and the keys that wait for a restart |
| `efr project list` | `projects.list` | one line per registered project: its name (`-` for none) and its root; with none, the registry file and how to add one |
| `efr project add [PATH] [--name NAME]` | `admin.project_add` | registers PATH (relative to the current directory; the daemon resolves links), or without PATH the git work tree that holds the current directory, else the directory, which may not be the home directory or `/`; the daemon writes `projects.toml` with its comments and reloads; a reload that fails is a warning on stderr, because the file is written |
| `efr project remove PATH` | `admin.project_remove` | takes the project with that root out, the same way; a root that no project has exits 1 |
| `efr paths [--json]` | `admin.status` when the daemon runs | each root with its source (`EFR_<ROOT>_DIR`, `EFR_HOME`, XDG, `/run/user`) and whether it exists, `config.toml`, the database, `secrets/` and the socket; then the daemon's roots, with a warning on stderr for each one that differs; a daemon with the sandbox adds the launcher's copy (with its source and whether its SHA-256 matches), bubblewrap and its version, and the sandbox's state and runtime directories |

`efr project` goes through the daemon, because `efr-scope` owns `projects.toml` and the
CLI may not depend on it; without a daemon it exits 3 like the other daemon commands.

A command that finds an error in a config file prints it and exits 1. Without a
daemon, the commands that change the file say that it reads the file when it starts.

The zsh plugin runs a bare `efr send`, `efr send --steer`, `efr new` or `efr compact`
and hands the shell context, the last command line and the prompt over in the
environment: `EFR_CONTEXT`, `EFR_LAST_COMMAND` and `EFR_PROMPT`. For `efr compact`,
`EFR_PROMPT` holds the focus, and the plugin hands over no last command and no turn
settings. Any local user can read a command
line in `/proc/<pid>/cmdline`; `/proc/<pid>/environ` is readable only by the user's
own processes. `--context-json`, `--last-command` and the prompt words do the same by
hand, and each wins over its variable. The variables reach no child process (`efr`
starts only `xdg-open`, through `efr_stdx::process::command`, which removes them) and
no log: `LastCommand` and `efr_stdx::env::Env` show them in `Debug` by length only.
The plugin also sets `EFR_DRAFT_FILE`, where `efr send` and `efr new` put the text that
is still in the input row when they end (`draft.rs`, below).

Turn settings: `efr send` and `efr new` ask for a permission mode, a model and a
reasoning effort with `--mode`, `--model` and `--effort`, else with `EFR_MODE`,
`EFR_MODEL` and `EFR_EFFORT`, in which the plugin hands over the terminal's choice. A
flag wins over its variable, and an empty variable counts as unset. Each value left
out is not sent, and the daemon's config decides it when the turn starts
(`prompt.send` `params.settings`). The CLI checks only the mode's name, before it
connects (exit 2 with the choices); the daemon checks the model and the effort against
its model list. `efr send --steer` takes no settings, because a running turn keeps its
own, and it ignores the variables. `efr settings` shows what a prompt would get: the
model list and the default model come from the daemon (`models.list`), the default
mode and effort from `config.toml` as `efr` reads it, so they match the daemon's when
both read the same config root. Each line names its source: a flag, a variable, the
file's path, `default`, `the daemon's default`, the model's default effort, or none
sent (the backend chooses). A model whose efforts `models.list` leaves empty takes any
effort.

The last command travels as `prompt.send` `params.last_command`, never inside the
context: the daemon gives it to the turn's preamble and records it in no event. The
context is decoded as a `ShellContext`, so a `last_command` member that a caller puts
there is dropped. Without a context (typed by hand), the context is the working
directory and the terminal on stdin, and the connection's origin is `cli` instead of
`shell`.

Replies:

- When the daemon says that a setting of the turn came from the prompt
  (`PromptSendResult.settings`, `overridden`), the reply's first line is a dim note
  with those values, such as `mode auto, model gpt-5.4`. It comes from the
  `prompt.send` result, so it shows before the first event. Settings that the config
  gave are left out, and a daemon that sends no settings gets no note.
- When stdout is a terminal (and `TERM` is not `dumb`), each assistant message streams
  through an `efr_render::Renderer`. Committed output is written once; the live zone is
  redrawn in place: carriage return, cursor up over the old live zone's rows (counted
  again at the current width when the terminal was resized), erase to the end of the
  screen, then the new committed output and live zone, all inside synchronized output
  (`CSI ? 2026 h` and `l`). The live zone is clipped to one row less than the screen.
- Events change the view; they write nothing themselves. The follow loop writes frames:
  a change shows at once when the last frame is 16 ms old, else when the 16 ms are up,
  so the screen gets at most 60 frames a second. A question, an answer, a key, the end
  of the turn and an error show at once. A resize of the window (SIGWINCH) draws the
  live zone again at the new width. After a stop (Ctrl+Z, then `fg`, SIGCONT), the
  shell's lines stand below the old live zone: the next frame starts a new live zone
  below them, moves nothing above the cursor, and hides the cursor again. The times come from the injected clock.
- The subscription asks for drafts (`drafts: true`): the text, the reasoning and the
  tool input of the running turn before the daemon records them, about every 16 ms.
  Draft text merges with the persisted updates of its message by byte offset: a
  persisted update that repeats text that drafts showed changes nothing, a draft that
  starts past the text held (an earlier one was dropped) waits for the next persisted
  update, and `assistant_message_completed` stays the truth. A draft of another turn,
  or one older than an event of the turn that ends what drafts show (the end of a
  message, the start of a tool call, the end of the turn; the daemon's rule), changes
  nothing. A steer or an interrupt request moves nothing: the conversation records
  them, not the turn, so the drafts after them still name the turn's own last event. An older
  daemon sends no drafts, and the text then comes with the persisted updates.
- All text that arrived goes into the renderer at the next frame, so it shows within
  one frame of its arrival; nothing holds it back to pace it. What streams is what
  `efr_render::render` makes of the whole text.
- While a turn runs on a terminal, the last row of the live zone is the status row:
  a braille spinner (one frame each 100 ms) in the `accent` role, the state in the
  `muted` role, and from 1 s on the time since `turn_started` (`12s`, `1m 05s`,
  `1h 02m 05s`). The states: `waiting for the running turn` (a queued prompt),
  `waiting for the model`, `thinking` or `thinking: <title>` (a reasoning draft, with
  the newest bold title of the reasoning), `writing`, `preparing <tool>, 3.2 KB` (a
  tool input draft, the newest call of the answer), `running <tool>` (a call of the
  running turn that a queued prompt waits behind), `waiting for an answer` (an approval
  that another client must answer), `compacting context` (a `compacting` draft, until
  the turn's `conversation_compacted`, the next `context` draft or the end of the
  turn), and, after 20 s without an event or a draft while it waits for the model or
  writes, `waiting for the model, no data for 25s`. After the time comes the gauge of
  the model's context (see "Context" below), such as `⠼ writing  12s  ctx 43%`. A band of three characters in the
  `text` role moves over the state one character per tick, then rests for a second. The
  band starts with a reset (SGR 0), so it shows on a dim `muted` and on a `muted` with
  a colour. `render.motion = false` shows a still `•` and no band. A tick (every 100 ms
  on the injected clock) that changes only the status row writes only that row:
  carriage return, cursor up to the row, erase the line, the row (and back down to the
  cursor in the input row when there is one). While the user is asked
  something here (an approval, the quarantine question, an answer line) the row goes
  and its time stops; the 20 s of a stall count again from the answer. Without the
  input row, the cursor is hidden while the row shows; with it, the cursor waits in the
  input row. The cursor comes back for a
  question and on every way out: the end of the turn, Ctrl+C, SIGTERM and SIGHUP, an
  error, a panic (the hook in `output.rs`) and the default action of SIGQUIT. The zsh plugin's precmd
  shows it again after any line that ran `efr`, for a `kill -9`.
- A completed turn ends with one muted line after a blank line, such as `done in 42s,
  18.2k tokens in, 1.1k out`: the time from the `at` of `turn_started` to the `at` of
  `turn_completed`, and `turn_completed.usage`. When the event carries `context`, the
  gauge takes the place of the input tokens: `done in 42s, ctx 43% (89k/206k), 1.1k
  out` (see "Context" below). An interrupted turn ends with
  `interrupted after 12s`, also after Ctrl+C, which counts the time on efr's clock
  without the time of questions, and with the gauge when `turn_interrupted` carries
  `context` (`interrupted after 12s, ctx 43% (89k/206k)`); a failed one has no such
  line. `render.turn_summary = false` leaves the line out. Piped output keeps its notes
  as they were.
- Context. A turn shows how full the model's context is with a gauge, `ctx N%`
  (`format/context.rs`). N is the tokens as a percent of the limit, rounded down: the
  limit is the point where efrd compacts (`[compaction] auto_at` percent of the
  model's window), or the hard cap (95%) when `[compaction] auto` is off, so 100% means
  that a compaction runs now. The gauge is in the `success` role below 50%, in the
  `warning` role from 50% and in the `error` role from 90%, each in the colour of the
  role only (`RenderOptions::tint`), so the warning level is not bold. Under `NO_COLOR`
  only the `error` level stands out, in bold. The status row has it always once a
  `context` draft came (the turn sends one before and after each model call), and so
  does the line of a running call, which takes the row's place; the newest count wins,
  and a compaction's `tokens_after` counts too. On a screen too narrow for it, the
  gauge goes first, so that the state keeps 10 columns. The end-of-turn line has it
  from the end event. Each `conversation_compacted` of the turn leaves one muted line
  in the scrollback (on stderr in a pipe), wrapped at the width with the rows after
  the first indented 2 columns, so its way out is never cut off:
  - `context compacted (auto): 231k -> 24k tokens, kept 3 turns, summary 3.2k`, with
    `pruned 12 outputs` in place of the summary when pruning alone made room, and
    `(efr compact)` for a manual one;
  - `context full: the request was 281k of 272k tokens; compacted and retried` for an
    `overflow` one;
  - `context full: compaction did not free enough room (still 240k); run ,compact or
    efr new` when `tokens_after` is at or above the limit (a miss of the breaker),
    and `...; run efr new` after a manual one.

  The summary size is the `output_tokens` of the summary request, else the summary's
  bytes / 4. The counts read `999`, `3.2k`, `24k`, `207k`, `1.2M`. A turn that fails
  for its context (`internal` with `data.cause = "context_overflow"`) shows the
  daemon's message, which names `,compact`.
- A turn whose `turn_completed` carries `changes` (its first snapshot against its
  last) ends with one more muted line right before that one, such as `3 files
  changed, +24 −7`, also with `render.turn_summary = false` and in a pipe (on
  stderr). A turn that changed no file has no such line.
- The progress bar of the terminal's tab (OSC 9;4): an indeterminate bar (`9;4;3`)
  while the turn runs, sent again on every tick, a paused one (`9;4;4`) while the user
  is asked something, `9;4;0` on every way out, and `9;4;2;100` when the turn failed.
  `render.progress = "auto"` sends it only to Ghostty 1.2 or later
  (`TERM_PROGRAM=ghostty` and `TERM_PROGRAM_VERSION`), kitty 0.47 or later
  (`TERM_PROGRAM=kitty`) and Windows Terminal (`WT_SESSION`), never through a
  terminal multiplexer (below); other terminals read OSC 9 as a notification. `on` sends it whenever stdout
  is a terminal, `off` never. No query decides any of this: a query needs a reply on
  stdin, which would take the keys typed ahead for the shell.
- A turn is a list of blocks with one blank line between them: prose, a tool call, a
  question, a note. There is no blank line inside a block, between notes that follow
  each other, between a question and the line of its answer, and between `✓ allowed`
  and the call that it allowed.
- A tool call is named by what it does: `$ cargo test` for a shell call, `read
  src/main.rs` and `write src/main.rs` for the file tools, `settings ...` for the
  settings tool, `apply_patch <files>` for a patch, `<tool>: <detail>` for any other.
  The block of a call starts with `·` in the `accent` role and what the call does in
  the `code` role:

  ```text
  · $ cargo metadata --no-deps | jq -r '.packages[].name'
    ✓ 1.4s

  · $ make
    │ cc -c a.c
    │ make: *** No rule to make target 'all'.  Stop.
    ✗ exit 2
  ```

  Each line of a command of several lines shows on its own row, under the first, so
  two commands never look like one with more arguments. A line wider than the screen
  goes on in the next row, and nothing of what runs is cut. A row is cut after the
  last space that fits when the row is then at least half full: a muted `\` ends it,
  and the rows after it are indented 4 more columns, so the indent stands for that
  space. Otherwise (no space fits, or the cut would leave a short word such as `cp`
  alone in the row) the row is cut inside a word: a muted `↩` ends it, and the word
  goes on in the next row at the same column, with no indent, so the rows show no
  space that the command does not have.
- An `apply_patch` call never shows the text of its patch. Its line names each file of
  the patch in the patch's order, with the lines that the patch adds and removes:
  `apply_patch src/a.rs +3 −1, new notes.md +2, delete old.rs, move src/expr.rs →
  src/expression.rs +1 −1`. The input is the patch text (a JSON string in the
  freeform form, `freeform` on `tool_call_started`) or the member `input` of the
  function form. Only the file lines (`*** Add File:`, `*** Delete File:`,
  `*** Update File:`, `*** Move to:`) and the `+` and `-` lines count; the CLI does not
  check the patch, the daemon does. The line goes on in the next row only between two
  files, at the column of the first file and with no mark, because it is no command.
  `efr history` shows the same line, and the live row of the running call cuts it with
  `…`.
- On a terminal, a call of the followed turn shows in the live zone while it runs: the
  spinner (accent), the call (code) cut to the width with `…` at the cut, and from 1 s
  on how long it has run, then the gauge of the context, such as `⠹ $ cargo test -p
  app  12s  ctx 43%`; up to three more lines
  of a command of several (or two and `(5 more lines)`); then the last three lines of
  its output with text in them (from `tool_call_output_updated`), each after `  │ `,
  muted and cut to the width. The status row hides meanwhile, because the call's row
  carries the spinner. A call whose approval waits shows nothing until the answer, and
  its time counts from the answer.
- When the call ends, its block is written once in place of the live rows, with its
  result on a row of its own: `✓` in the `success` role, or `✗ exit 101` (or `✗
  failed`) in the `error` role, also for a contained call (the end of the turn says
  where the sandbox can write), and `✗ refused: efr's config (floor)` when efr refused
  it before it ran (`tool_call_completed` with a `refusal`). The time follows for a
  call that ran 1 s or more: `✓ 6.2s`, `✗ exit 2 · 1.5s`. A failed call keeps the last three lines of its
  output above its result (from the last `tool_call_output_updated`, else from the
  output that the model got, without efr's own notes in brackets at its end); a call
  that went well keeps none. A call that the user denied, or whose approval expired,
  never ran: its rows follow the line of the answer at once, with no result, so the
  scrollback keeps what did not run. Notes and the end of a turn are muted lines.
- What a call changed comes with its `tool_call_completed`, from the daemon's own
  snapshots, so it works in every project, git or not, and in `$SCRATCH`. A file
  write's `diff` shows in its block above the result, after the bar: the first
  `render.diff_lines` lines (20 by default) from the first hunk on, in the `diff.*`
  roles with the file's syntax colours, then a muted `… N more lines` that also counts
  the lines that the daemon cut (its last line `... N more lines`). The file headers
  (`diff --git`, `---`, `+++`) stay out, because the call names the file. A line wider
  than the screen goes on in the next row at the same column after a muted `↩`, so
  nothing of it is cut and the rows read back as the line (`efr_render::diff_rows`).
  The diff of an `apply_patch` call holds the diffs of its files one after another,
  in the order of the patch, each with its `---` and `+++` lines. The CLI cuts it at
  each file (`format::patch::file_diffs`; the lines of a hunk are counted from its
  header, so a removed line `-- note` stays in its hunk) and shows one diff block per
  file, each after a row that names the file at the column of the result: the path
  in the `code` role, after a muted `new`, `deleted` or `moved` (`moved a.rs →
  b.rs`) when the headers say so (`/dev/null` on one side, or two paths). The
  `render.diff_lines` rule and its `… N more lines` row apply to each file on its own.
  A diff of one file shows as a write's diff, with no such row, because the call's
  line names the file.
  A call that shows no diff and has `changes` (a shell call, or a write with
  `render.diff_lines = 0`) gets one muted row under its result, such as `changed
  src/a.rs +3 −1 · deleted old.rs · new notes.md (+2 more)`: the first three files,
  grouped by kind (`changed`, `new`, `deleted`, `renamed a → b`), `(binary)` for a
  binary file, the counts of a changed or renamed file with `+N` in the `success` role
  and `−N` in the `error` role, and how many more files changed. A row wider than the
  screen goes on in the next row, indented 2 more columns. Every path passes through
  `format::one_line`. The row and the turn's line say what changed in the roots while
  the call or the turn ran, not who changed it: the snapshots cannot tell, so a file
  that another conversation or the user's editor saved at the same time shows too.
- When stdout is not a terminal, the blocks go to stderr in plain text, in the order
  that a terminal keeps them: the question of a call and the line of its answer, then
  the first rows of the call, then its result (with the last lines of a failure's
  output) when it ends. The first rows of a call wait for its question until another
  event of the turn comes, such as its output, so the command shows once. Nothing
  wraps there, because no screen sets a width.
- A question is a card in the live zone, below the rest:

  ```text
  ? allow outside the sandbox
  │ cd ~/p/app && find target/debug/build \
  │     -path '*ghostty*' -type f | awk '{print $1}'
  │ runs with your full rights: files, secrets, network
  │ programs: cd, find, awk
  │ y allow · n deny
  ```

  The title (`? ` and what the user decides) is in the `warning` role, the bar `│ ` is
  muted, what runs is in the `code` role, one row for each line of a command, wrapped
  as in a call's block. Secondary facts are muted: why the call asks (`why: ...`), the
  parts of a long line that ask (`asks for: hostnamectl, systemctl --failed`), the
  programs, efr's own facts after `efr:` and the model's reason as `the model says:
  "..."`. The last row gives the keys, muted with the keys in bold, or `waiting for
  another client to answer`. A text row wider than the screen goes on in the next
  row, indented 2 more columns. The title of an approval says what a "yes" allows:
  `allow this command`, `allow this write`, `allow this read`, or for an exit `allow
  outside the sandbox`, `allow ~/notes writable for this call` and the like. An
  approval of the turn that the followed one waits behind starts `the running turn
  asks:`. A diff preview shows after the bar, rendered as a diff at the width less the
  bar.
- The approval of an `apply_patch` call has the title `allow this patch`. Its preview
  shows the diff of every file of the patch, none left out but what the daemon cut,
  each after a row that names the file: the path, `new <path>` with `new` muted, and
  `delete <path>` or `move <from> → <to>` whole in the `warning` role, because a "yes"
  allows a delete or a move. The daemon marks them with a line `delete <path>` or
  `move <from> -> <to>` before the diff of that file; the CLI reads the headers as
  well. The rows of a diff are cut as in a call's block, with a muted `↩`, and the
  file headers stay out.
- A shell call's card shows each line of its command when the summary quotes exactly
  the command of the call's `tool_call_started`; what the summary says besides follows
  after `why:`. Any other summary shows as one plain row. Only a last line of plain
  names counts as the line of the parts that ask; anything else stays on the first
  row. `efr history` joins both with `; `, as the daemon's notices do.
- When the question is answered, its card gives its place to one line: `✓ allowed` in
  the `success` role, `✗ denied` in the `error` role, `✓ allowed from the phone` for
  an answer from another client, `✗ the approval expired`. The command is written
  once, in the block of the call that follows, also when the call did not run. A card taller than the screen (the live
  zone must stay smaller than the screen, or rows that scroll off the top could never
  be erased) is written to the scrollback whole instead, with its keys left in the
  live zone; the line of the answer then follows it. When stdout is not a terminal,
  the card goes to stderr whole, then the line of the answer, then the rows of the
  call.
- The `auto` sandbox (`docs/sandbox.md`). A turn in which a call ran in the sandbox
  (`tool_call_started` with a contained `launch`, not refused) ends with one muted
  line, `sandbox: writes in the project, $SCRATCH, private /tmp; no network`, before
  the end-of-turn line; a failed contained call ends `✗ exit 2 · 1.5s`, as any other. A
  call's `sandbox` summary adds muted rows under its result: `network: blocked
  <host>:<port> (<reason>)` and the background jobs that stopped. A sandbox that could
  not start is the call's result: `✗ the sandbox could not start: <reason>; efr checks
  it again`. A turn whose `auto` fell back to `cautious`
  (`EffectiveSettings.fallback`) starts with `auto is not available here; this turn
  runs as cautious: <reason>`.
- An approval with `exit` (an action that leaves the sandbox) shows the whole line of
  the call from its `exit_requested` record instead of the summary, each line of a
  command of several on its own row. A line that runs outside the sandbox also gets
  the risk row `runs with your full rights: files, secrets, network` in the `warning`
  role and `programs:` with every program word as a plain name. A program in a write
  root or changed this turn also shows the path it resolves to and the mark
  `(untrusted: written in the sandbox; in a write root, changed this turn)` in the
  `warning` role; a word that resolves to nothing gets `(not found)`. `why:` names what
  leaves (`write ~/.zshrc`, `network (example.com)`, `sudo (you may need to type your
  password)`). Every part passes through `format::one_line`, and each line of a command
  through `format::command_line`, so a control or format character shows as a
  stand-in.
- The quarantine question (`surface_question_requested`) is not an approval: its card
  (`? keep the git settings that the last command changed`) names the git settings
  that the last call changed and the launcher moved to quarantine, and its keys are
  `y keep · n leave in quarantine`. Its answer line is `✓ kept` or `✗ left in
  quarantine`. The answer goes with `sandbox.surface_respond` and the question's own
  `QuestionId`; nobody answering leaves the change in quarantine. At the end of an
  `auto` turn, the files that run code later outside the sandbox
  (`turn_surface_report`) show as three dim lines.
- Under `NO_COLOR` the blocks keep their structure and their marks, and the CLI's lines
  keep bold and dim, as the rest of the reply does, without a colour.
- When stdout is not a terminal, the raw markdown is written, and notes, calls and
  questions go to stderr, so stdout holds the reply alone.
- `RenderOptions` come from the window size (`TIOCGWINSZ` through rustix), `NO_COLOR`
  (no colour), `COLORTERM=truecolor` or `24bit` (24-bit colour, otherwise 16), `TERM`
  and whether stdout is a terminal, the way the terminal counts widths, and the theme
  and the colours from `config.toml`:

  ```toml
  [render]
  theme = "catppuccin-mocha"   # any name efr_render::Theme::from_name accepts, or auto
  theme_dark = "catppuccin-mocha"   # what auto takes on a dark background
  theme_light = "catppuccin-latte"  # what auto takes on a light background
  palette = "~/.config/efr/theme.toml"   # a theme file: [colors] and code_theme
  motion = true                # the spinner and the band of the status row
  turn_summary = true          # the line at the end of each turn
  progress = "auto"            # the progress bar of the tab: auto, on or off
  diff_lines = 20              # the lines of a file write's diff; 0 shows none
  turn_input = true            # the input row below a running turn

  [render.colors]
  accent = "#f2c14e"           # "#rrggbb", an ANSI slot 0 to 15, or a name
  ```

  Every colour of a reply and of the CLI's own lines goes through a role of
  `efr-render` (`text`, `muted`, `accent`, `heading`, `link`, `code`, `success`,
  `warning`, `error`, `quote`, `diff.add`, `diff.remove`, `diff.hunk`; the list of
  `efr_config::COLOR_ROLES` is the same, and a test keeps them equal). The CLI's tones
  map onto roles: notes are `muted`, what needs the user's care is `warning`, a failed
  exit code is `error`, and bold stays bold (SGR 1). A role takes its colour from
  `[render.colors]`, else from the theme file that `render.palette` names (`[colors]`
  with the same keys), else from the terminal's 16 colours (the accent is slot 3,
  yellow). A `#rrggbb` colour takes the nearest of the 16 colours without truecolor,
  and no colour under `NO_COLOR`. The theme file's `code_theme`, the path of a
  `.tmTheme` file (absolute, `~/...` or relative to the theme file), wins over `theme`
  for code blocks and diffs. `theme = "auto"` takes `theme_dark` or `theme_light` as
  `EFR_TERMINAL_BG` says `dark` or `light`; the zsh plugin asks the terminal for its
  background once when it loads and sets the variable. Without it, `auto` takes the
  dark theme. `efr` itself sends no query. In Ghostty outside a terminal multiplexer
  (`TERM_PROGRAM` is `ghostty`) widths count by grapheme cluster, as Ghostty counts
  them with mode 2027; everywhere else by code point. A multiplexer counts widths
  itself and keeps the outer `TERM_PROGRAM`: efr sees one when `TMUX` (tmux), `STY`
  (GNU screen) or `ZELLIJ` (zellij) is set, or `TERM` starts with `screen` or `tmux`. The live zone's rows, the cut of
  a line to the width and the status row use that count.

  `efr-config` reads and checks the whole file with the schema the daemon uses, unknown
  keys refused; the CLI uses `[render]`, and for `efr settings` `permissions.mode`,
  `model.name` and `model.effort`, and warns, without failing, when the file is not
  valid, names a theme `efr-render` does not have, or names a theme file or a code
  theme that cannot be read or used. The layers below apply then. `efr config check`
  and `efr config edit` check the theme names, the theme file and the code theme too.

The input row. When stdin and stdout are terminals and `render.turn_input` is on (the
default), `efr send` and `efr new` read keys for the whole turn into an input row, the
last part of the live zone, below the status row. Inline only: no alternate screen.

- The key thread starts before the prompt goes out and keeps the typeahead
  (`Keys::keep`), so the keys typed between Enter in zsh and the first frame land in
  the row. It turns off the terminal's map of carriage return to newline, so Enter and
  Ctrl+J are two keys, and an escape byte that no other byte follows within one read
  timeout (0.1 s) is Esc. Bracketed paste is on while the row exists (`CSI ? 2004 h`)
  and off on every way out, a panic and SIGQUIT included (`TurnView::restore`).
- The row is `› ` and the text, or the muted hint `enter steer · tab queue · esc
  interrupt` while it is empty. The hint starts one column after the cursor, so the
  cursor stays on a blank cell and does not cover a letter of the hint; efr never
  changes the cursor shape. Typed text starts at the cursor. A long text goes on in
  the next row, at a grapheme cluster, by the width that the terminal counts; at most
  five rows show, the ones around the cursor. The cursor waits in the row where the
  next character goes (`live::Cursor`); the next frame starts there and moves up only
  over the rows above it.
- `row.rs` edits the line: printable text and UTF-8, Backspace and Delete (one
  grapheme cluster), Left and Right, Home and End, Ctrl+A, Ctrl+E, Ctrl+U (to the start
  of the line), Ctrl+K (to its end), Ctrl+W (the word before the cursor), Alt+B and
  Alt+F, Ctrl+J and Alt+Enter (a newline), and a bracketed paste, whose newlines never
  send. A tab shows as a blank and another control character as its stand-in.
- Enter sends `turn.steer` for the followed turn with `if_late` set to queue it as
  `prompt.send` would, with the context, the last command and the settings that the
  plugin handed over. An empty Enter does nothing. Tab sends `prompt.send` to the
  conversation, queued behind the running turn. Alt+Up sends `prompt.withdraw` for the
  newest prompt that this view queued and puts its text into the row, after the text
  there on a line of its own. A prompt that already started (`conflict`) stays in the
  list with a note, so the view follows it until it ends; one that the daemon does not
  know (`not_found`) leaves the list. A send that fails puts the text back and says
  why.
- Esc sends `turn.interrupt` with the unread steers of this view (`resend_steers`),
  this terminal's context, last command and settings for them (`resend_as`, as Enter
  sends them), and the prompts that it queued (`withdraw`). The prompts that the
  daemon took back come into the row after its text; the steers that it sent again
  become a prompt that runs next, with the note `interrupted to send your message`.
  The note waits for the end of the interrupted turn, so it comes after the call that
  Esc stopped and after the line of the end. When the followed prompt still waits
  behind another turn, Esc takes it back with `prompt.withdraw` instead, with the
  prompts after it: the newest first and the followed one last, so none of them can
  start in between. When the followed one started meanwhile (`conflict`), Esc
  interrupts it as above. A turn that Esc stopped, and after which nothing runs, ends
  the command as Ctrl+C does (exit 130, no message).
- Ctrl+C with text in the row clears the text. On an empty row it interrupts the turn
  and ends the command as before. It sends the unread steers of this view as
  `withdraw_steers` and the prompts that it queued as `withdraw`, which would run with
  nobody to follow them. Only the texts that the result names go back to the shell: a
  steer that it does not name was read by a model call, or stays part of the turn.
  When the followed prompt still waits behind another turn, Ctrl+C takes it back as
  Esc does, with the prompts after it, and their texts go back to the shell. When no
  answer comes in 3 s, or the connection ends before the last answer, nothing says
  what the daemon took back: the followed prompt that did not start, the unread
  steers and the queued prompts of this view all go back to the shell, with a note
  that efrd may still run them. A late `steering_withdrawn` would otherwise keep a
  steer from every model call and from the shell.
- Above the status row, each unread steer of this view shows as `↳ steer: <first
  line>` and each queued prompt as `↳ queued: <first line>`, muted; a steer that came
  too late is a queued prompt with `(too late to steer, so it waits in the queue)`.
  When `steering_delivered` names a steer, it goes to the scrollback as the user's
  message (`> ` and each line in bold, as `efr history` shows a prompt), and this
  view's own `turn_steered` gets no note. A queued prompt goes to the scrollback the
  same way when its turn starts.
- The view follows each prompt that it queued after the turn before it: the turn's end
  line, then the status row says `waiting for the running turn` until the next one
  starts. The command ends when the last one ends (`turn_completed`, `turn_failed`,
  `turn_interrupted`, `turn_cancelled` or `prompt_withdrawn`), with the exit code of
  that one; an earlier failure is a note. Steers that no model call read when their
  turn ended, and a queued prompt that a restart cancelled, come back into the row.
- A question, an answer line and the keys that an allowed call keeps take the keys
  first: the row hides, keeps its text, and comes back after. The keys typed before
  the question appeared go into the row: those that the reader queued, and those still
  in the terminal (`KeyReader::mark`). The mark counts them at once: the keys that
  the key thread took and the bytes that wait in the terminal (`FIONREAD`). So a key
  that the thread reads a moment after the question showed never goes into the row. A paste that the question cut goes on into the row until its end, for
  at most 1 s; then the row ends it with the text that came. A key among them that
  would send stays text there. When the keys come back, the row drops an escape
  sequence that the question cut. Before the keys go back to the row after an answer line, a call that
  asked for a password or the keys it kept, the reader throws away what is still
  unread (`KeyReader::flush`), so the rest of a password never lands in the row.
- While the last line of a running call's output looks like a password prompt
  (`efr_protocol::looks_secret`) and the daemon reported no wait yet, the keys do not
  go to the row: they wait in a line that is never shown or sent, and Enter there
  does nothing (`Ask::Retain`, as for a call allowed here). The row hides meanwhile.
  The keys typed before the prompt showed stay the row's (the mark). A visible wait
  of the call takes that line as its answer line (not shown when it looks secret); a
  hidden wait, a new last line and the end of the call throw it away, zeroed. The
  daemon reports a wait only after a quiet time (about 1 s for a hidden one), and the
  output shows the prompt earlier, so without this a password typed at once would
  land in the row, and Enter would send it as a steer. Only an open last line holds
  the keys: a line that ends with a line break, as in the output of `grep password`,
  is not a prompt, and the daemon too tests only the line that the cursor is on.
- When the connection to efrd ends while the row exists (efrd restarts, the socket
  breaks), the view says so in a note, connects again as the command did (60 tries,
  0.25 s apart), and subscribes after the last event that it showed. The
  `turn_cancelled` events of a restarted efrd then put the texts of the cancelled
  prompts and of the unread steers back into the row, as for any cancelled turn. A
  steer or a prompt from the row whose answer was lost goes again with the same
  command id, so efrd answers from its receipt when it took the first one, and the
  text never goes twice. Ctrl+C, SIGTERM and SIGHUP stop the wait for efrd at once,
  and the text of that steer or prompt goes back to the shell. A restart ends the
  stream and then the connection; the end and the loss count against limits of their
  own (3 each, until the next event), so a restart counts once. A daemon of another
  protocol stops the tries at once. When efrd does not come back, or ends the
  subscription for good, the followed prompt that did not start, the unread steers
  and the queued prompts of this view go back to the shell with a note, because
  nothing says whether they will run. Without the row, a connection that ends is the
  command's error, as before.
- After a stop (Ctrl+Z, then `fg`), the reader sets its mode again and the next frame
  turns bracketed paste on again.
- When `efr` ends, the text that is still in the row, with the keys that the reader
  did not hand over yet, goes back to the shell (`draft.rs`). The reader also hands over
  the keys that it reads until it stops (`KeyReader::stop_keeping`), so a key typed as
  the turn ends is never lost; the keys typed after that stay for the shell. The text is
  written to
  `EFR_DRAFT_FILE` as UTF-8, mode 0600, without a final newline, in a directory that it
  creates with mode 0700 when it is missing. The plugin's precmd puts it on the command
  line as `, <text>`. Without the variable, or when the write fails, one muted note
  `not sent: <text>` shows it. This happens on every way out: the end, Ctrl+C, Esc,
  SIGTERM and SIGHUP, an error, and a failure before the turn is followed.

With `render.turn_input = false`, or without a terminal on stdin or stdout, no key is
read for the row and everything below works as it did before the row.

Approvals show inline. When stdin is a terminal, `y` allows and `n` denies with one key:
a named thread puts the terminal into non-canonical mode without echo, discards keys
typed before the question, and restores the terminal before the command goes on or
exits. Without a terminal on stdin, the question waits for another client (the phone).

Answers to a running command work the same way. `conversation.subscribe` sends
`answers_input: true` exactly when stdin is a terminal, so the daemon knows that a
person here can type. When `tool_call_input_changed` says that the running call waits
for input, the key thread starts (and discards typeahead), and the live zone shows the
command's prompt (its last output line) and how to answer:

- `hidden` (echo off: `sudo`, `ssh`, `passwd`): nothing typed is shown, and the note
  says that the agent sees it only if the program prints it. Echo being off is all
  that is checked: the prompt text comes from the command, so a program that fakes a
  `sudo` prompt would read the answer and could print it.
- `visible` (a `[Y/n]` question): the CLI echoes what is typed, and says that the agent
  sees it if the program shows it. On a plain terminal the echo puts it in the output;
  behind a relay such as `sudo`'s own pty, the program on the inner terminal decides
  whether it is shown, and a password prompt of a program there counts as `visible`
  too.
- `visible` with `looks_secret` (a password prompt behind a relay, such as `ssh` or a
  `sudo -u` that runs `passwd`): the CLI shows nothing of what is typed and says "this
  looks like a password prompt behind another program: your typing is not shown here,
  and the agent sees it only if that program shows it". The answer still goes as a
  visible one, and keys typed while the call asks nothing more are thrown away until it
  completes, as for a hidden answer.

Printable text is added, Backspace removes one character, Ctrl+U clears the line, arrow
keys and other escape sequences are ignored, and Enter sends the line with
`input.respond` (an empty line too: it takes a question's default). After a visible or
manual answer is sent, a dim note says `answer sent`. A password (a hidden answer, or
a visible one behind a relay that looks secret) gets no note: the line under its
prompt said how it goes, and the command's own output shows what came of it. When the
daemon answers `conflict` or `not_found`, a note says that the command no longer waits
and nothing was sent. A wait of `none` or the
call's completion stops the key thread and drops any unsent text. A call that asked for
a hidden answer is the exception: until it completes, the key thread keeps running and
throws keys away, so a password typed again while `sudo` checks a wrong one neither
shows nor waits for the shell. A key thread that read an answer line discards unread
input before it restores echo, also when the turn ends or Ctrl+C ends the command. The
line is an `answer::AnswerLine`: at most `efr_protocol::InputRespond::MAX_TEXT_BYTES`
bytes, allocated once at that size, never in `Debug`, and zeroed when it is cleared,
sent or dropped; a hidden one never reaches the view, a log or a note. The copies of a
typed answer that are zeroed: the line (with the bytes that Backspace removed, at the
next clear, send or drop), the `SecretText` it becomes, and the encoded request frame
once it is written (`efr-client`). The copies that are not: each key's byte in the key
thread's one-byte read buffer and in the key queue, the pieces of the frame that the
JSON encoder's growing buffer leaves in freed memory, and the frame's copy in the
connection's write buffer, until later frames overwrite it or the connection ends. When
stdout is not a terminal, the prompt and how to answer go to stderr once, and a visible
answer is echoed there on a line of its own after `> ` as it is typed (Backspace and
Ctrl+U erase what they remove); a hidden one never is. A prompt that waits behind
another turn counts as a person who can answer, so until its own turn starts it shows
the running turn's last output line and asks for the input that turn's command waits
for, also one asked before the prompt (found on the newest page of the log, as approvals
are). Without a terminal on stdin, one dim note says that the command waits for input
that `efr` cannot ask for here. Full-screen programs need an attach, which comes later.

Keys typed ahead are kept for a call that may wait for input, but only for one that the
user allowed here: when `approval_requested` carries `interactive` (the call may wait
for input at the terminal, such as `sudo`) and the `y` comes from this terminal, the key
thread that read it goes on, and every key typed while the call asks nothing goes into
a pending `AnswerLine`, without echo, never shown and never sent, with Enter dropped.
When the call reports a `visible` wait, the pending text (with the keys still queued)
starts the answer line: shown for a plain question, not shown when it `looks_secret`,
where a dim note says how many characters the line starts with and that Ctrl+U clears
them, since a stray key in front of a password would fail it unseen.
The user still presses Enter once the question is on the screen, so a `y` typed ahead
of a `[Y/n]` waits in the line instead of being sent blind. A `hidden` wait drops the
pending text, zeroed, and starts an empty line; so does a manual line. A wait that ends
keeps the keys for the call again, unless it asked for a password, which keeps them
being thrown away as above. A manual line that closes, sent or not, may have held a
password too, so from then on the call's keys are thrown away instead of kept, and
`Ctrl+\` still opens the next manual line. The call's completion, the turn's end or another approval
drops the pending text and stops the key thread, which discards unread input first.
Keys typed outside such a call, or during a call allowed elsewhere or that waits for
nothing, go to the input row, or without it stay typeahead for the user's shell as
before.

A command can also wait for input without a prompt that the daemon can see, such as a
program that reads a line after printing a newline. `efr` must not send keys to a
command just because it is silent: text typed then goes to the input row, or without
it stays typeahead for the user's shell. So
while a shell call of the followed turn runs, reports no wait, no key is read for it
and keys can be read here, ten seconds without output (`follow::SILENCE`, timed on the
injected clock from the call's last output, wait or answer) bring one dim line: "no
output for 10 s; press `Ctrl+\` to type an input for the command". Only while that line
is shown does `efr` take SIGQUIT (`quit.rs`): `Ctrl+\` opens an answer line, which goes
with `input.respond` `manual: true`; after one answer the keys stop and the silence
starts again. The line is never shown as it is typed: no prompt was reported, so
nothing tells whether the command asks for a password (behind `ssh`, a remote `sudo`
prompt reports no wait), and the program's own echo still shows in its output tail.
Only a call whose `tool_call_started` carries `manual_input` offers the line; the
daemon leaves it off for a call whose command types into a shell that reads command
lines, where a manual answer would run as a command line. `Ctrl+\` again closes the
line unsent and brings the offer back. Output, a wait or an approval takes the offer
away. `efr` also takes SIGQUIT while it reads keys (an approval, an answer line), where
the default action would end it with the terminal left without echo; a press there
does nothing. Any other SIGQUIT keeps its default meaning: tokio's handler
stays installed once the key was first offered, so the listener does the default
action itself (`signal_hook::low_level::emulate_default_handler`), and `efr` ends by
SIGQUIT as it would without a handler. The terminal throws away typeahead when it
turns `Ctrl+\` into the signal, as it does for Ctrl+C. A call that the user approved
because it may wait for input keeps running past the model's timeout while this
terminal follows (`shell.interactive_timeout_minutes`); an answer after the call
completed is refused as `not_found`, which the CLI shows as the note that the command
no longer waits.

Ctrl+C sends `turn.interrupt` for the followed turn and then ends the command (exit
130); the daemon stops the model and any running command. With the input row, it
first clears the row's text, and it takes back the prompts that the row queued and a
followed prompt that still waits (see above). Without the row, a followed prompt that
still waits behind another turn is not interrupted, and the CLI says so. During a login, Ctrl+C
closes the connection. What arrived stays on the screen.

SIGTERM (`kill`, `timeout`) and SIGHUP (the terminal closes) while a turn is followed
end the command with a last frame, which shows the cursor again and clears the
progress bar. Then the signal takes its default action, so the shell sees the signal.
The turn goes on in its conversation.

Exit codes: 0 success; 1 the daemon failed the request, the turn failed or was
interrupted elsewhere, the connection broke, or a config file has an error; 2 a usage
error; 3 no daemon listens, also because the socket path is longer than a socket
address holds; 130 Ctrl+C, or Esc in the input row when the turn that it stopped is
the last one; the signal itself (128 and its number in the shell) for
SIGTERM and SIGHUP during a turn.

A failure that a first run meets gets a second line with the command that fixes it:
no daemon (`systemctl --user start efrd`, or `just run` for one in the foreground), a
turn that fails as `unauthorized` because the provider has no usable credentials
(`efr login openai`), a turn that fails as `invalid` because the provider does not
serve the model (`name` under `[model]` in the daemon's `config.toml`, then a restart),
a daemon that does not answer (`journalctl --user -u efrd`), a socket path too long
for a socket address (a shorter `EFR_RUNTIME_DIR` or `EFR_HOME`), a missing
`XDG_RUNTIME_DIR` or `HOME` (`EFR_HOME` replaces both), and `efr` and `efrd` from
different builds.
`efr status` adds the login line on stderr when no provider is logged in.

Logs go to stderr, filtered by `EFR_LOG` (default `warn`, because stderr shares the
terminal with the reply).

## Tier

Tier 4: a binary.

## Allowed dependencies

`efr-client` (the protocol client), `efr-config` (`config.toml`), `efr-render`
(markdown to ANSI), `efr-protocol` (the wire types) and `efr-stdx` (paths, `Clock`, `Rng`, `EFR_*` variables, named
threads, `process::command`). `xtask/src/deps.rs` holds the allowlist. Not
`efr-transport`, not even in tests: the fake daemon of the tests speaks
`efr_protocol::framing` directly. Tests also use `efr-test-support` (a dev-dependency)
for `Wait`, which waits for a key thread, a view or the daemon's notice with a real
time limit, and for `TestClock`, which moves the frames and the ticks only when a test
moves it.

Third-party crates: `clap`, `tokio`, `futures`, `serde`, `serde_json`, `toml` (strings
in the output of `efr config show`), `jiff`, `rustix` (window size, termios, ttyname), `unicode-width`
(column alignment of listings and the echo of an answer), `unicode-segmentation` (a cut
to the width never splits a grapheme cluster), `tracing`, `tracing-subscriber`, `thiserror`, `zeroize` (the answer
line), `signal-hook` (the default action of SIGQUIT, SIGTERM and SIGHUP once `efr`'s own
handler is installed, without unsafe code).

`NO_COLOR`, `TERM`, `COLORTERM`, `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`, `TMUX`, `STY`,
`ZELLIJ` and `WT_SESSION` are read in `terminal.rs` with `std::env::var_os`,
and `VISUAL`, `EDITOR` and `HOME` in `context.rs`: they are terminal and POSIX
conventions that `efr_stdx::env::Var` does not name. `HOME` shortens the paths of the
sandbox's lines to `~/...` and expands a leading `~` in the paths of the theme file.
`EFR_TERMINAL_BG` is read through `efr_stdx::env`.

## Invariant

- No business logic: the daemon decides; `efr` turns arguments into protocol calls and
  replies into text. It never writes the daemon's database or credentials, and the
  login runs in the daemon.
- `output.rs` is the only module that writes to stdout or stderr.
- efr is not a TUI: no alternate screen, no full-screen view, no footer that stays,
  and committed output is written once and never redrawn. Only the live zone changes,
  and the status row goes before the prompt comes back. No terminal query is sent, and
  no key is read for the status row or a frame.
- The cursor is never left hidden on a way out that efr controls, and `output.rs`
  keeps the bytes that undo the view's changes for a panic and SIGQUIT.
- The terminal is never left in non-canonical mode: every path out of a turn stops the
  key thread, which restores the settings before it reports done.
- Text from the daemon or the model cannot drive the terminal: markdown goes through
  `efr-render`, and everything else the CLI prints passes through `format::one_line`,
  `format::lines` or `format::command_line`, which turn control characters into
  visible stand-ins.
- A hidden answer, one whose prompt looks secret, and a manual one are never written
  to stdout or stderr, never logged and never handed to the view; they leave the
  process only inside `input.respond`.
- No key goes to a command while it is merely silent: only `Ctrl+\`, while the view
  offers it, opens an answer line. Without the input row, no key is read then at all;
  a SIGQUIT while no key is read and nothing is offered ends `efr` as it would without
  a handler.
- The text of the input row leaves `efr` only as a steer, a prompt, or the hand-back
  to the shell's file in the runtime directory, never in an argument. Keys read for an
  answer line that may be a password never reach the row.
- The last command line never reaches the shell context, and so never an event.
- What the plugin hands over in `EFR_CONTEXT`, `EFR_LAST_COMMAND` and `EFR_PROMPT`
  reaches no child process and no log.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-cli
```

Unit tests cover argument parsing (with `efr --help` snapshots), output formatting
(insta snapshots of status, listings, transcripts, config and rendered turns), the
live-zone redraw arithmetic for wrapped lines (including a proptest that the CLI's row
count agrees with the renderer's), the key thread on a real pseudo-terminal, and the
editing of an answer line. The frame tests drive the view and the follow loop with
`efr-test-support`'s `TestClock`: snapshots of the frames and the status row at its
ticks, a burst of 50 events inside one frame time that gives one frame, a question
and the end that never wait, a tick that writes only the status row, a resize that
SIGWINCH stands for, a new live zone after SIGCONT, the cursor and the progress bar on
every way out, the allowlist
of the progress bar, drafts that merge with persisted updates without a line twice
(and without a log line), a dropped draft that heals, and a proptest that pushing text in any
pieces keeps what `render` makes of the whole text. The call tests cover the running rows with
the spinner, time, the lines of a command and three lines of output at 40 and 80
columns, the committed block of a call (with no time under 1 s, the exit code, the
refusal and the failure's last lines), a long line that goes on in the next row and
is never cut, a call whose approval waits, and a blank line between calls. The layout
tests (`follow/view/tests/layout.rs`) play each case on a simulated screen 40 and 80
columns wide, with colour and with `NO_COLOR`, and through a pipe: a call that went
well, a failed call with its tail, a refused call, a command of several lines, a long
command, the file tools, a question for a grant in the sandbox, a question for a run
with full rights with an untrusted program, the answer lines, a card taller than the
screen, a whole turn with prose, two calls and a question, a file write with its
diff (a line wider than 40 columns, the `… N more lines` row, a diff that the daemon
cut, and `render.diff_lines = 0`), a shell call's row of changed files (kinds, a
rename, a binary file, the files left out), the line of a turn that changed files,
a whole turn that writes a file and runs a command, an `apply_patch` call of five
files (its line, one diff block per file with the limit on each, a new, a deleted and
a moved file), the question about that patch with every line of every file and the
delete and the move in the `warning` role, and a patch of one file.
`format/patch/tests.rs` checks the files and counts that the line of a patch reads
(both forms of the input, markers with spaces, a path with an escape), the rows of
the line, the split of a diff of several files (a removed line that looks like a
header, a cut at the end, the marks of a preview, a move without a hunk) and the
heading of each file. A view test shows `preparing apply_patch, 3.2 KB` from a draft,
then the patch's line, and a history test shows the line of a patch in both forms and
never its text. `format/changes/tests.rs`
checks the rows, the turn line, the split of a diff and the list of `efr diff
--stat`; `commands/diff/tests.rs` runs `efr diff` against a fake daemon (the
conversation it picks, `--turn`, a terminal, a pipe, a turn that changed nothing, no
snapshot, no conversation) and snapshots its diff at 40 and 80 columns with colour
and with `NO_COLOR`. The context tests: `format/context/tests.rs` checks the percent
(rounded down, over 100, no limit), the level of each percent, the colour of each
level without bold and under `NO_COLOR`, the short counts and the line of each kind of
compaction (auto, manual, prune-only, overflow, a miss after auto and after manual);
`follow/view/tests/context.rs` snapshots the gauge in the status row at each level in
16 colours and under `NO_COLOR`, `compacting context` until the event and until the
next count, the gauge in a running call's line and where it goes on a narrow screen,
the end-of-turn line at each level, an interrupted turn with and without a count, a
pipe with the lines on stderr and no gauge, and a compaction of another turn; the
layout case `a_turn_that_compacts_its_context` plays a whole turn with three
compactions at 40 and 80 columns, with colour, under `NO_COLOR` and in a pipe.
`commands/compact/tests.rs` runs `efr compact` against a fake daemon (the
conversation it picks, the focus from the words and from `EFR_PROMPT`, a refusal, no
conversation, the status row on a terminal, Ctrl+C), and a history test shows the
compaction lines and, with `--verbose`, the focus and the summary with its control
characters as stand-ins. The card tests check
that every row fits the width and that the rows give the command back whole. The
colour tests run the status row, a call and a question in 16 colours, in truecolor and
under `NO_COLOR` with a palette of the user's, and check that `COLOR_ROLES` equals the
roles of `efr-render`; the settings tests lay `[render.colors]` over a theme file, read
a code theme by a path relative to the theme file, warn about a theme file or a code
theme that cannot be used, and pick the theme of `auto` by `EFR_TERMINAL_BG`. The
width tests count the rows of ZWJ emoji, flags and variation selectors by code point
and by grapheme cluster, and redraw a live zone of such text both ways. The end-to-end tests run whole commands against a fake
daemon on a socket in a temporary directory, with a fixed screen, scripted keys and a
Ctrl+C the test triggers; the input tests check the `input.respond` params and that no
byte written to the fake terminal holds a hidden answer. The silence tests use a clock
whose sleeps end when the test opens a gate and a `Ctrl+\` the test presses, and check
that no key reader starts before the key and that the key is waited for only while the
line offers it. `quit.rs` is tested with SIGQUITs that the test sends to its own process,
with a stand-in for the default action, which would end it.
The input row: `row/tests.rs` checks each key of the line editor, grapheme clusters,
pastes, the limit and the layout (with a proptest that the cursor stays on a
character and every row fits); `follow/view/input/tests.rs` the row, the hint, the
five rows and the lines of what waits; `live/tests.rs` the cursor in the tail and a
tick above it on a simulated screen (`testing::Grid`); `follow/view/tests/input.rs`
snapshots the live zone with the row, a steer that a model call read and a queued
prompt that the view follows, and checks that the note of a resend comes after the
call that Esc stopped. `follow/tests/row.rs` runs the follow loop with the row
against a fake daemon: the params of Enter, Tab, Esc, Alt+Up and Ctrl+C, a late steer,
the prompts that the view follows, a question and a password that take the keys, the
cursor and bracketed paste, and the text that goes to `EFR_DRAFT_FILE`.
`draft/tests.rs` checks the file's mode and content and the note without the plugin;
`commands/send/tests.rs` the keys typed while the prompt goes out, a refused prompt
and `render.turn_input = false`; the key thread tests on a pseudo-terminal check the
typeahead that the row keeps, Enter and Ctrl+J, Esc, the flush, the keys that a stop
for the row keeps, and the mode after a stop. `tests/it/smoke.rs` runs the built `efr send` on a pseudo-terminal against a
`TestDaemon` and checks that keys typed before it started go back to the shell's file
and that the terminal's mode comes back.
The integration tests are one test binary, `tests/it/main.rs`, with one module per
area (nextest names a test `efr-cli::it <module>::<test>`). `tests/it/binary.rs` runs the built `efr` against the same kind of fake daemon for exit
codes and the environment. `tests/it/plugin.rs` sources `shell/zsh/efr.plugin.zsh` in
`zsh -f` with a fake `efr` that records its command line from `/proc` and the
variables it was handed, so a test can prove that no typed text reaches a command
line; one test runs the built `efr` behind the plugin against a `TestDaemon`. The
fake prints canned output for `efr settings` and `efr models`, so the tests cover
`,mode`, `,model` and `,effort` (a value is kept only when `efr settings` accepts it,
`default` clears it, a bare one prints its line), the handover of `EFR_MODE`,
`EFR_MODEL` and `EFR_EFFORT` to `,` and `,new` and not to `,!` or `,compact`,
`,compact` with its focus in `EFR_PROMPT` and no last command (a bare one too, and
one typed at a terminal with quotes, `$`, `*` and `!!`, which arrive as typed, and
the typo `,compcat`, which stays on the line with a hint), completion after
`compinit`, and the runtime root of the notices (`EFR_RUNTIME_DIR`, `EFR_HOME`,
`XDG_RUNTIME_DIR`, then a private `/run/user/<uid>`, which a test points at a
temporary tree). The hand-back of the input row: `,` and `,new` name the shell's draft
file under the runtime root, precmd puts its text on the command line once as `, `
and the text (a blank text puts nothing), and on a pseudo-terminal the text that the
fake hands back waits on the next command line and runs as one prompt of two lines
with Enter. The question about the background runs against a driver that plays the
terminal on a pseudo-terminal: a light and a dark OSC 11 reply (ended by ST or BEL), a
terminal that answers only DA1, no reply at all (dark after one second), a reply that
comes after 300 ms (it sets the background and never reaches the line editor), keys
typed while the replies are on their way and between them (they stay for the shell,
without the replies), and no question without a terminal,
with keys typed ahead or with `EFR_TERMINAL_BG` set. The widgets (the lone `,` and Ctrl+Space toggles, sticky mode, the
tag of the terminal's settings before the robot, the bare setting words of sticky
mode, a `,word` that names nothing as a prompt) are tested by typing into
an interactive `zsh -f -i` on a pseudo-terminal through zsh's own `zsh/zpty` module,
so ZLE reads every key as it does for a person. Its `e2e_` tests need zsh and skip
with a message unless `EFR_TEST_ZSH=1`:

```sh
EFR_TEST_ZSH=1 cargo nextest run -p efr-cli -E 'test(/^plugin::/)'
```

`tests/it/smoke.rs` runs the built `efr` against a real daemon, `efr-test-daemon`'s
`TestDaemon`, in the test's process: the `efr --help` snapshot, `efr status`, and an
`efr send` round trip whose model is a local Responses server.

Nothing touches the network, the real home, config or runtime directory, and nothing
under test sleeps on real time: only `Wait` sleeps between its polls, and its limit
decides only when a failing test stops waiting.
