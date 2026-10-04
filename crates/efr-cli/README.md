# efr-cli

## Purpose

`efr`, the command-line relay to the daemon. The zsh plugin
(`shell/zsh/efr.plugin.zsh`) runs it for every `,` line; people run it for status,
history, login and settings. It parses arguments, calls the daemon over the Unix
socket through `efr-client`, and prints. The daemon decides everything; `efr` keeps no
state and never writes the daemon's database or credentials.

| Command | Protocol | Notes |
|---|---|---|
| `efr send --context-json <json> [--last-command <text>] -- <prompt>` | `prompt.send`, then `conversation.subscribe` after the prompt's `seq` | follows the turn until it ends |
| `efr send --steer --context-json <json> -- <text>` | `conversations.list` to find the tty's active conversation, `turn.steer` | `--conversation <id>` skips the lookup |
| `efr new --context-json <json> [--last-command <text>] -- [prompt]` | `prompt.send` with `new_conversation` | without a prompt, the text is empty and nothing is followed |
| `efr status` | `admin.status` | |
| `efr history [conversation] [--limit n] [--cursor c]` | `conversations.list`, `conversation.history` | a conversation is its id or the start of it (4 characters or more) |
| `efr login openai` | `admin.login_openai` (stream) | prints the authorize URL, opens it only when `EFR_OPEN_BROWSER` is on, waits for completion |
| `efr config show` | none | the client's effective settings with the source of each, as TOML |

`--last-command` travels as `prompt.send` `params.last_command`, never inside the
context: the daemon gives it to the turn's preamble and records it in no event.
`--context-json` is decoded as a `ShellContext`, so a `last_command` member that a
caller puts there is dropped. Without `--context-json` (typed by hand), the context is
the working directory and the terminal on stdin, and the connection's origin is `cli`
instead of `shell`.

Replies:

- When stdout is a terminal (and `TERM` is not `dumb`), each assistant message streams
  through an `efr_render::Renderer`. Committed output is written once; the live zone is
  redrawn in place: carriage return, cursor up over the old live zone's rows (counted
  again at the current width when the terminal was resized), erase to the end of the
  screen, then the new committed output and live zone, all inside synchronized output
  (`CSI ? 2026 h` and `l`). The live zone is clipped to one row less than the screen.
  Tool calls, answers and the end of a turn are dim notes, one line each.
- When stdout is not a terminal, the raw markdown is written, and notes and approval
  questions go to stderr, so stdout holds the reply alone.
- `RenderOptions` come from the window size (`TIOCGWINSZ` through rustix), `NO_COLOR`
  (no colour), `COLORTERM=truecolor` or `24bit` (24-bit colour, otherwise 16), `TERM`
  and whether stdout is a terminal, and the theme from `config.toml`:

  ```toml
  [render]
  theme = "catppuccin-mocha"   # any name efr_render::Theme::from_name accepts
  ```

  The daemon validates the whole file; the CLI reads only `[render]` and warns, without
  failing, when it cannot use it.

Approvals show inline. When stdin is a terminal, `y` allows and `n` denies with one key:
a named thread puts the terminal into non-canonical mode without echo, discards keys
typed before the question, and restores the terminal before the command goes on or
exits. Without a terminal on stdin, the question waits for another client (the phone).

Ctrl+C closes the connection; the daemon sees it close and cancels the turn (or the
login). What arrived stays on the screen.

Exit codes: 0 success; 1 the daemon failed the request, the turn failed or was
interrupted elsewhere, or the connection broke; 2 a usage error; 3 no daemon listens
(with the hint `systemctl --user start efrd`); 130 Ctrl+C.

Logs go to stderr, filtered by `EFR_LOG` (default `warn`, because stderr shares the
terminal with the reply).

## Tier

Tier 4: a binary.

## Allowed dependencies

`efr-client` (the protocol client), `efr-render` (markdown to ANSI), `efr-protocol`
(the wire types) and `efr-stdx` (paths, `Clock`, `Rng`, `EFR_*` variables, named
threads, `process::command`). `xtask/src/deps.rs` holds the allowlist. Not
`efr-transport`, not even in tests: the fake daemon of the tests speaks
`efr_protocol::framing` directly.

Third-party crates: `clap`, `tokio`, `futures`, `serde`, `serde_json`, `toml` (the
`[render]` table), `jiff`, `rustix` (window size, termios, ttyname), `unicode-width`
(row counting), `tracing`, `tracing-subscriber`, `thiserror`.

`NO_COLOR`, `TERM` and `COLORTERM` are read in `terminal.rs` with `std::env::var_os`:
they are terminal conventions that `efr_stdx::env::Var` does not name.

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
- The last command line never reaches the shell context, and so never an event.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-cli
```

Unit tests cover argument parsing (with `efr --help` snapshots), output formatting
(insta snapshots of status, listings, transcripts, config and rendered turns), the
live-zone redraw arithmetic for wrapped lines (including a proptest that the CLI's row
count agrees with the renderer's), and the key thread on a real pseudo-terminal. The
end-to-end tests run whole commands against a fake daemon on a socket in a temporary
directory, with a fixed screen, scripted keys and a Ctrl+C the test triggers.
`tests/binary.rs` runs the built `efr` against the same kind of fake daemon for exit
codes and the environment. `tests/plugin.rs` sources `shell/zsh/efr.plugin.zsh` in
`zsh -f` with a fake `efr` that records its arguments; its `e2e_` tests need zsh and
skip with a message unless `EFR_TEST_ZSH=1`:

```sh
EFR_TEST_ZSH=1 cargo nextest run -p efr-cli --test plugin
```

Nothing touches the network, the real home, config or runtime directory, and nothing
sleeps on real time. The `TestDaemon` smoke tests come with `efr-test-daemon`.
