# efr-cli

## Purpose

`efr`, the command-line relay to the daemon. The zsh plugin
(`shell/zsh/efr.plugin.zsh`) runs it for every `,` line; people run it for status,
history, login and settings. It parses arguments, calls the daemon over the Unix
socket through `efr-client`, and prints. The daemon decides everything; `efr` keeps no
state and never writes the daemon's database or credentials.

| Command | Protocol | Notes |
|---|---|---|
| `efr send [--context-json <json>] [--last-command <text>] [--conversation <id>] [--mode <m>] [--model <id>] [--effort <e>] [--] [prompt]` | `prompt.send`, then `conversation.subscribe` after the prompt's `seq` | follows the turn until it ends |
| `efr send --steer [--context-json <json>] [--conversation <id>] [--] [text]` | `conversations.list` to find the tty's active conversation, `turn.steer` | `--conversation <id>` skips the lookup; a steer takes no turn settings |
| `efr new [--context-json <json>] [--last-command <text>] [--mode <m>] [--model <id>] [--effort <e>] [--] [prompt]` | `prompt.send` with `new_conversation` | the prompt is required (exit 2 without one); the plugin's bare `,new` sends nothing and makes the next `,` line run `efr new` |
| `efr settings [--mode <m>] [--model <id>] [--effort <e>]` | `models.list`; `admin.status` for `--mode auto` | the mode, model and effort that a prompt with these values would use, one `key = value  # source; choices: ...` line each; a value the daemon would refuse exits 2 with the choices; `--mode auto` with a sandbox that is not available prints a warning on stderr with the reason (turns run as `cautious`) |
| `efr models [--names]` | `models.list` | the daemon's models, `*` before the default, with the efforts of each; `--names` prints only the ids, for completion |
| `efr status` | `admin.status` | says on stderr how to log in when no provider is logged in; shows the config file, its last reload error, the keys that wait for a restart, and the `sandbox` line: `ready (Landlock ABI 10, bubblewrap 0.13.0, caches tmp, network none)` or `unavailable: <reason>; auto runs as cautious` with its fix |
| `efr history [conversation] [--limit n] [--cursor c] [--verbose]` | `conversations.list`, `conversation.history` | a conversation is its id or the start of it (4 characters or more); each turn shows its mode, model and effort as a dim line after its prompt, and the fallback note when `auto` ran as `cautious`; the sandbox's notes, exits, quarantine questions and turn-end reports show as dim lines; `--verbose` adds the record of each exit (`exit_requested`: its line, targets, hosts, programs and counts, never a user message) and how it was judged; `--verbose` without a conversation shows the newest conversation of this terminal (the first listed whose `tty` is the terminal on stdin), else the newest of all, after a dim line that says which; `--limit` and `--cursor` then page its events |
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

The zsh plugin runs a bare `efr send`, `efr send --steer` or `efr new` and hands the
shell context, the last command line and the prompt over in the environment:
`EFR_CONTEXT`, `EFR_LAST_COMMAND` and `EFR_PROMPT`. Any local user can read a command
line in `/proc/<pid>/cmdline`; `/proc/<pid>/environ` is readable only by the user's
own processes. `--context-json`, `--last-command` and the prompt words do the same by
hand, and each wins over its variable. The variables reach no child process (`efr`
starts only `xdg-open`, through `efr_stdx::process::command`, which removes them) and
no log: `LastCommand` and `efr_stdx::env::Env` show them in `Debug` by length only.

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
  live zone again at the new width. The times come from the injected clock.
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
  that another client must answer), and, after
  20 s without an event or a draft while it waits for the model or writes,
  `waiting for the model, no data for 25s`. A band of three characters in the
  `text` role moves over the state one character per tick, then rests for a second. The
  band starts with a reset (SGR 0), so it shows on a dim `muted` and on a `muted` with
  a colour. `render.motion = false` shows a still `•` and no band. A tick (every 100 ms
  on the injected clock) that changes only the status row writes only that row:
  carriage return, cursor up one row, erase the line, the row. While the user is asked
  something here (an approval, the quarantine question, an answer line) the row goes
  and its time stops. The cursor is hidden while the row shows and comes back for a
  question and on every way out: the end of the turn, Ctrl+C, an error, a panic (the
  hook in `output.rs`) and the default action of SIGQUIT. The zsh plugin's precmd
  shows it again after any line that ran `efr`, for a `kill -9`.
- A completed turn ends with one muted line after a blank line, such as `done in 42s,
  18.2k tokens in, 1.1k out`: the time from the `at` of `turn_started` to the `at` of
  `turn_completed`, and `turn_completed.usage`. An interrupted turn ends with
  `interrupted after 12s`; a failed one has no such line. `render.turn_summary = false`
  leaves the line out. Piped output keeps its notes as they were.
- The progress bar of the terminal's tab (OSC 9;4): an indeterminate bar (`9;4;3`)
  while the turn runs, sent again on every tick, a paused one (`9;4;4`) while the user
  is asked something, `9;4;0` on every way out, and `9;4;2;100` when the turn failed.
  `render.progress = "auto"` sends it only to Ghostty 1.2 or later
  (`TERM_PROGRAM=ghostty` and `TERM_PROGRAM_VERSION`), kitty 0.47 or later
  (`TERM_PROGRAM=kitty`) and Windows Terminal (`WT_SESSION`), never inside tmux
  (`TMUX`); other terminals read OSC 9 as a notification. `on` sends it whenever stdout
  is a terminal, `off` never. No query decides any of this: a query needs a reply on
  stdin, which would take the keys typed ahead for the shell.
- A tool call is named by what it does: `$ cargo test` for a shell call, `read
  src/main.rs` and `write src/main.rs` for the file tools, `settings ...` for the
  settings tool, `<tool>: <detail>` for any other. A command of several lines never
  shows its lines joined: the line is its first line and how many follow, such as
  `$ cd src (and 3 more lines)`, and a cut to the width keeps that count.
- On a terminal, a call of the followed turn shows in the live zone while it runs: the
  spinner (accent), the call (muted) and from 1 s on how long it has run, such as
  `⠹ $ cargo test -p app  12s`, then the last three lines of its output with text in
  them (from `tool_call_output_updated`), each after `  │ `, muted and cut to the
  width. The status row hides meanwhile, because the call's line carries the spinner.
  A call whose approval waits shows nothing until the answer, and its time counts
  from the answer. When the call ends, one muted line is written once in place of the
  live lines: `$ cargo test -p app  6.2s`, with `exit 101` (or `failed`) in the
  `error` role when it failed, `(sandbox)` after the code of a failed contained call,
  and `refused: efr's config (floor)` in the `warning` role when efr refused it before
  it ran (`tool_call_completed` with a `refusal`). The time shows for a call of 1 s or
  more. A failed call keeps the last three lines of its output below its line (from
  the last `tool_call_output_updated`, else from the output that the model got, without
  efr's own notes in brackets at its end); a call that went well keeps none. A call
  that the user denied, or whose approval expired, writes no line of its own. Lines of
  consecutive calls have no blank line between them. Answers and the end of a turn are
  muted notes, one line each. When stdout is not a terminal, a call is one note on
  stderr when it starts, such as `$ make`, and one more when it fails, such as `shell
  exited with 2`.
- An approval question shows the daemon's summary on one line and, when the daemon
  named the simple commands of a long line that ask, a second line
  `asks for: hostnamectl, systemctl --failed`. A shell call whose command has several
  lines shows `shell: run 2 lines:` and then each line on its own, numbered, so two
  commands never look like one with more arguments; the whole command shows, and a
  terminal wraps a long line. This needs a summary that quotes exactly the command of
  the call's `tool_call_started` (what the summary says after it follows after
  `also:`); any other summary shows as before. The heading is in the `warning` role,
  the rest is plain. Only a last line of plain names counts
  as that line; anything else stays on the first line. `efr history` joins both with
  `; `, as the daemon's notices do.
- The `auto` sandbox (`docs/sandbox.md`). The first call of a turn that runs in the
  sandbox (`tool_call_started` with a contained `launch`) gets one dim line,
  `sandbox: writes in the project, $SCRATCH, private /tmp; no network`, and a failed
  contained call ends `$ make  exit 2 (sandbox)` (`shell exited with 2 (sandbox)` when
  stdout is not a terminal). A call's `sandbox` summary adds
  `network: blocked <host>:<port> (<reason>)` and the background jobs that stopped. A
  turn whose `auto` fell back to `cautious` (`EffectiveSettings.fallback`) starts with
  `auto is not available here; this turn runs as cautious: <reason>`.
- An approval with `exit` (an action that leaves the sandbox) shows the whole line of
  the call from its `exit_requested` record instead of the summary (each line of a
  command of several on its own, numbered), then
  what leaves and how the call runs after a "yes" (`leaves the sandbox: network; runs
  in the sandbox with full network for this call`, or `runs outside the sandbox: sudo
  (you may need to type your password)`). A line that runs outside the sandbox also
  gets `the whole line runs with your full rights (files, secrets, network)` and
  `programs:` with every program word and the path it resolves to, or `(builtin)` for
  a word that the shell runs itself; a program in a
  write root or changed this turn gets `(untrusted: written in the sandbox)`. Only the
  facts that matter most are in the `warning` role: the heading (`approval needed:`),
  the full-rights line and the untrusted mark. The other fact lines are plain text.
  efr's own facts follow muted after `efr:`, then the model's reason as `the model
  says: "..."`, and the key line is muted with the keys in bold (`allow? y = yes, n =
  no`). Every part passes through `format::one_line`, and each line of a command
  through `format::command_line`.
- The quarantine question (`surface_question_requested`) is not an approval: it names
  the git settings that the last call changed and the launcher moved to quarantine,
  and asks `keep it? y = yes, n = no` with one key: the heading in the `warning` role,
  the changes plain, the key line muted with bold keys. The answer goes with
  `sandbox.surface_respond` and the question's own `QuestionId`; nobody answering
  leaves the change in quarantine. At the end of an `auto` turn, the files that run
  code later outside the sandbox (`turn_surface_report`) show as three dim lines.
- When stdout is not a terminal, the raw markdown is written, and notes and approval
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
  dark theme. `efr` itself sends no query. In Ghostty outside tmux (`TERM_PROGRAM` is
  `ghostty` and `TMUX` is unset) widths count by grapheme cluster, as Ghostty counts
  them with mode 2027; everywhere else by code point. The live zone's rows, the cut of
  a line to the width and the status row use that count.

  `efr-config` reads and checks the whole file with the schema the daemon uses, unknown
  keys refused; the CLI uses `[render]`, and for `efr settings` `permissions.mode`,
  `model.name` and `model.effort`, and warns, without failing, when the file is not
  valid, names a theme `efr-render` does not have, or names a theme file or a code
  theme that cannot be read or used. The layers below apply then. `efr config check`
  and `efr config edit` check the theme names, the theme file and the code theme too.

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
`input.respond` (an empty line too: it takes a question's default). After a send, a dim
note says `answer sent`; when the daemon answers `conflict` or `not_found`, the note
says that the command no longer waits and nothing was sent. A wait of `none` or the
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
nothing, stay typeahead for the user's shell as before.

A command can also wait for input without a prompt that the daemon can see, such as a
program that reads a line after printing a newline. `efr` must not read keys just
because a command is silent: text typed then stays typeahead for the user's shell. So
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
130); the daemon stops the model and any running command. A prompt that still waits
behind another turn cannot be taken back yet, and the CLI says so. During a login,
Ctrl+C closes the connection. What arrived stays on the screen.

Exit codes: 0 success; 1 the daemon failed the request, the turn failed or was
interrupted elsewhere, the connection broke, or a config file has an error; 2 a usage
error; 3 no daemon listens, also because the socket path is longer than a socket
address holds; 130 Ctrl+C.

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
line), `signal-hook` (the default action of SIGQUIT once `efr`'s own handler is
installed, without unsafe code).

`NO_COLOR`, `TERM`, `COLORTERM`, `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`, `TMUX` and
`WT_SESSION` are read in `terminal.rs` with `std::env::var_os`,
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
- No key is read while a command is merely silent: only `Ctrl+\`, while the view offers
  it, opens an answer line; a SIGQUIT while no key is read and nothing is offered ends
  `efr` as it would without a handler.
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
SIGWINCH stands for, the cursor and the progress bar on every way out, the allowlist
of the progress bar, drafts that merge with persisted updates without a line twice
(and without a log line), a dropped draft that heals, and a proptest that pacing keeps
what `render` makes of the whole text. The call tests cover the running line with its
spinner, time and three lines of output at 40 and 80 columns, the one committed line
of a call (with no time under 1 s, the exit code, the refusal and the failure's last
lines), a call whose approval waits, and consecutive calls without blank lines. The
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
The integration tests are one test binary, `tests/it/main.rs`, with one module per
area (nextest names a test `efr-cli::it <module>::<test>`). `tests/it/binary.rs` runs the built `efr` against the same kind of fake daemon for exit
codes and the environment. `tests/it/plugin.rs` sources `shell/zsh/efr.plugin.zsh` in
`zsh -f` with a fake `efr` that records its command line from `/proc` and the
variables it was handed, so a test can prove that no typed text reaches a command
line; one test runs the built `efr` behind the plugin against a `TestDaemon`. The
fake prints canned output for `efr settings` and `efr models`, so the tests cover
`,mode`, `,model` and `,effort` (a value is kept only when `efr settings` accepts it,
`default` clears it, a bare one prints its line), the handover of `EFR_MODE`,
`EFR_MODEL` and `EFR_EFFORT` to `,` and `,new` and not to `,!`, completion after
`compinit`, and the runtime root of the notices (`EFR_RUNTIME_DIR`, `EFR_HOME`,
`XDG_RUNTIME_DIR`, then a private `/run/user/<uid>`, which a test points at a
temporary tree). The question about the background runs against a driver that plays the
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
