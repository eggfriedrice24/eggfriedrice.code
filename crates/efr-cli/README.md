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
| `efr settings [--mode <m>] [--model <id>] [--effort <e>]` | `models.list` | the mode, model and effort that a prompt with these values would use, one `key = value  # source; choices: ...` line each; a value the daemon would refuse exits 2 with the choices |
| `efr models [--names]` | `models.list` | the daemon's models, `*` before the default, with the efforts of each; `--names` prints only the ids, for completion |
| `efr status` | `admin.status` | says on stderr how to log in when no provider is logged in; shows the config file, its last reload error and the keys that wait for a restart |
| `efr history [conversation] [--limit n] [--cursor c]` | `conversations.list`, `conversation.history` | a conversation is its id or the start of it (4 characters or more); each turn shows its mode, model and effort as a dim line after its prompt |
| `efr login openai` | `admin.login_openai` (stream) | prints the authorize URL, opens it only when `EFR_OPEN_BROWSER` is on, waits for completion |
| `efr config show` | `admin.status` when the daemon runs | every key of `config.toml` with its value and source, then what `efr` uses (theme, colour, roots), then the file the daemon reads, its reload error and `restart_needed`, with a warning when the daemon reads another file; as TOML |
| `efr config check [path]` | none | the file checked with the daemon's schema and the theme names; an error names its line, column and key; exit 0 or 1 |
| `efr config edit` | `admin.config_reload` | creates a missing file from the commented example (never through a link to nothing), runs `$VISUAL`, else `$EDITOR`, else `vi` (through `sh`, so an editor with arguments works) on the file behind a link when `config.toml` is one, so an editor that saves by replacing the file keeps the link, checks the file, offers to edit again on an error when stdin is a terminal, then asks the daemon to reload |
| `efr config set <key> <value>`, `efr config unset <key>` | `admin.config_reload` | one scalar or list key through `efr-config`'s writer: comments and layout stay, a link stays and its target is written, a value the daemon would refuse is never written; then a reload |
| `efr config schema` | none | the JSON schema of `config.toml` |
| `efr config reload` | `admin.config_reload` | applied, or the file's error (exit 1), and the keys that wait for a restart |
| `efr paths [--json]` | `admin.status` when the daemon runs | each root with its source (`EFR_<ROOT>_DIR`, `EFR_HOME`, XDG, `/run/user`) and whether it exists, `config.toml`, the database, `secrets/` and the socket; then the daemon's roots, with a warning on stderr for each one that differs |

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
  Tool calls, answers and the end of a turn are dim notes, one line each. While a tool
  call runs, the last line of its output with text in it (from `tool_call_output_updated`)
  sits dim in the live zone, cut to the width; it goes when the call completes and is
  never committed.
- When stdout is not a terminal, the raw markdown is written, and notes and approval
  questions go to stderr, so stdout holds the reply alone.
- `RenderOptions` come from the window size (`TIOCGWINSZ` through rustix), `NO_COLOR`
  (no colour), `COLORTERM=truecolor` or `24bit` (24-bit colour, otherwise 16), `TERM`
  and whether stdout is a terminal, and the theme from `config.toml`:

  ```toml
  [render]
  theme = "catppuccin-mocha"   # any name efr_render::Theme::from_name accepts
  ```

  `efr-config` reads and checks the whole file with the schema the daemon uses, unknown
  keys refused; the CLI uses `[render]`, and for `efr settings` `permissions.mode`,
  `model.name` and `model.effort`, and warns, without failing, when the file
  is not valid or names a theme `efr-render` does not have. The defaults apply then.

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

A command can also wait for input without a prompt that the daemon can see, such as a
program that reads a line after printing a newline. `efr` must not read keys just
because a command is silent: text typed then stays typeahead for the user's shell. So
while a shell call of the followed turn runs, reports no wait, no key is read for it
and keys can be read here, ten seconds without output (`follow::SILENCE`, timed on the
injected clock from the call's last output, wait or answer) bring one dim line: "no
output for 10 s; press `Ctrl+\` to type an input for the command". Only while that line
is shown does `efr` take SIGQUIT (`quit.rs`): `Ctrl+\` opens an answer line, shown as it
is typed under the visible note, which goes with `input.respond` `manual: true`; after
one answer the keys stop and the silence starts again. Output, a wait or an approval
takes the line away. Any other SIGQUIT keeps its default meaning: tokio's handler
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
error; 3 no daemon listens; 130 Ctrl+C.

A failure that a first run meets gets a second line with the command that fixes it:
no daemon (`systemctl --user start efrd`, or `just run` for one in the foreground), a
turn that fails as `unauthorized` because the provider has no usable credentials
(`efr login openai`), a turn that fails as `invalid` because the provider does not
serve the model (`name` under `[model]` in the daemon's `config.toml`, then a restart),
a daemon that does not answer (`journalctl --user -u efrd`), a missing
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
`efr_protocol::framing` directly.

Third-party crates: `clap`, `tokio`, `futures`, `serde`, `serde_json`, `toml` (strings
in the output of `efr config show`), `jiff`, `rustix` (window size, termios, ttyname), `unicode-width`
(row counting), `tracing`, `tracing-subscriber`, `thiserror`, `zeroize` (the answer
line), `signal-hook` (the default action of SIGQUIT once `efr`'s own handler is
installed, without unsafe code).

`NO_COLOR`, `TERM` and `COLORTERM` are read in `terminal.rs` with `std::env::var_os`,
and `VISUAL` and `EDITOR` in `context.rs`: they are terminal and POSIX conventions that
`efr_stdx::env::Var` does not name.

## Invariant

- No business logic: the daemon decides; `efr` turns arguments into protocol calls and
  replies into text. It never writes the daemon's database or credentials, and the
  login runs in the daemon.
- `output.rs` is the only module that writes to stdout or stderr.
- The terminal is never left in non-canonical mode: every path out of a turn stops the
  key thread, which restores the settings before it reports done.
- Text from the daemon or the model cannot drive the terminal: markdown goes through
  `efr-render`, and everything else the CLI prints passes through `format::one_line` or
  `format::lines`, which turn control characters into visible stand-ins.
- A hidden answer, and one whose prompt looks secret, is never written to stdout or
  stderr, never logged and never handed to the view; it leaves the process only inside
  `input.respond`.
- No key is read while a command is merely silent: only `Ctrl+\`, while the view offers
  it, opens an answer line; other SIGQUITs end `efr` as they would without a handler.
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
editing of an answer line. The end-to-end tests run whole commands against a fake
daemon on a socket in a temporary directory, with a fixed screen, scripted keys and a
Ctrl+C the test triggers; the input tests check the `input.respond` params and that no
byte written to the fake terminal holds a hidden answer. The silence tests use a clock
whose sleeps end when the test opens a gate and a `Ctrl+\` the test presses, and check
that no key reader starts before the key and that the key is waited for only while the
line offers it. `quit.rs` is tested with SIGQUITs that the test sends to its own process,
with a stand-in for the default action, which would end it.
`tests/binary.rs` runs the built `efr` against the same kind of fake daemon for exit
codes and the environment. `tests/plugin.rs` sources `shell/zsh/efr.plugin.zsh` in
`zsh -f` with a fake `efr` that records its command line from `/proc` and the
variables it was handed, so a test can prove that no typed text reaches a command
line; one test runs the built `efr` behind the plugin against a `TestDaemon`. The
fake prints canned output for `efr settings` and `efr models`, so the tests cover
`,mode`, `,model` and `,effort` (a value is kept only when `efr settings` accepts it,
`default` clears it, a bare one prints its line), the handover of `EFR_MODE`,
`EFR_MODEL` and `EFR_EFFORT` to `,` and `,new` and not to `,!`, completion after
`compinit`, and the runtime root of the notices (`EFR_RUNTIME_DIR`, `EFR_HOME`,
`XDG_RUNTIME_DIR`, then a private `/run/user/<uid>`, which a test points at a
temporary tree). The widgets (the lone `,` and Ctrl+Space toggles, sticky mode, the
tag of the terminal's settings before the robot) are tested by typing into
an interactive `zsh -f -i` on a pseudo-terminal through zsh's own `zsh/zpty` module,
so ZLE reads every key as it does for a person. Its `e2e_` tests need zsh and skip
with a message unless `EFR_TEST_ZSH=1`:

```sh
EFR_TEST_ZSH=1 cargo nextest run -p efr-cli --test plugin
```

`tests/smoke.rs` runs the built `efr` against a real daemon, `efr-test-daemon`'s
`TestDaemon`, in the test's process: the `efr --help` snapshot, `efr status`, and an
`efr send` round trip whose model is a local Responses server.

Nothing touches the network, the real home, config or runtime directory, and nothing
sleeps on real time.
