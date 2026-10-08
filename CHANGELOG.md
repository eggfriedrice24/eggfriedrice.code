# Changelog

All notable changes to efr are in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions follow
[Semantic Versioning](https://semver.org/). Before 1.0.0, a minor or patch version can
change behaviour.

Each release has its own section. The release workflow copies the section of the tag
into the GitHub release notes, and it stops when the section is missing.

## [Unreleased]

### Added

- An input row below a running turn, in the terminal that follows it. Type there while
  the turn runs: Enter steers the turn, Tab queues the text as a prompt behind it, Esc
  interrupts it and Alt+Up takes back the newest prompt that you queued there. The row
  edits like a shell line (arrows, Home, End, Ctrl+A/E/U/W/K, Alt+B/F), Ctrl+J adds a
  newline, and a paste keeps its newlines. Keys that you type while the prompt goes
  out land in the row too. `render.turn_input = false` turns the row off.
- Steers that the model did not read yet show above the status row as `↳ steer:`, and
  prompts that you queued as `↳ queued:`. When a model call reads a steer, it moves
  into the reply as your message. A steer that comes too late for its turn waits in
  the queue instead.
- `efr` follows each prompt that you queue from the input row after the turn before
  it, and ends after the last one. Esc sends the steers that the model did not read as
  a new prompt, and puts the prompts that you queued back into the row, so nothing
  runs that you did not see. Ctrl+C clears the row; on an empty row it interrupts the
  turn and also takes back the prompts that you queued there.
- Text that is still in the input row when `efr` ends goes back to your zsh command
  line as `, <text>`, so you can edit it and send it again. Without the zsh plugin,
  `efr` shows it as one muted line.
- The diff of each file write shows in its call block, in the diff colours: the first
  `render.diff_lines` lines (default 20), then a muted `… N more lines`. This works in
  every directory.
- A shell call that changed files gets one muted row under its result, such as
  `changed src/a.rs +3 −1 · deleted old.rs · new notes.md (+2 more)`. A turn that
  changed files gets a muted line such as `3 files changed, +24 −7` before its end.
  This works in registered projects (git or not) and in `$SCRATCH`. The row and the
  line show what changed while the call or the turn ran, also a file that another
  conversation or your editor saved at the same time.
- `efr diff` prints what a turn changed: by default the last turn of this terminal's
  conversation, `--turn <id>` for another, `--stat` for the list of files. In a pipe it
  writes the plain diff. An ignored file such as `.env` shows in the list, but its
  content stays out of the diff.
- efr's own snapshot store: one bare git repository per registered project or
  `$SCRATCH` below the data directory. It never writes the project's `.git` or index.
  The `[snapshot]` table sets it (`snapshot.enabled`, size limits, `keep_turns`,
  `max_age_days`).
- efrd has the parts that the input row of a turn needs. A steer that comes too late
  for the running turn can become a queued prompt (`turn.steer` with `if_late`). You
  can take back a prompt that waits in the queue before it starts (`prompt.withdraw`).
  An interrupt can take back the queued prompts of one terminal and send its unread
  steers again as one new prompt, which runs next. efrd records when a model call
  reads a steer (`steering_delivered`). `efr history` shows a prompt that you took
  back as `withdrawn before it ran`.

### Changed

- A long command no longer leaves a short first word alone in its row (`cp \`). A row
  breaks at a space only when it is at least half full; else it breaks inside the word
  with `↩`.

### Fixed

- efrd no longer loses a steer (`,!`) at the end of a turn. Before, efrd accepted a
  steer after the last model call of the turn or after an interrupt request, and no
  model call read it. Now efrd refuses such a steer with a conflict, and you can send
  the text as a new prompt.
- efrd now ends a turn before it tells the clients that the turn ended. Before, a
  steer or an interrupt that you sent just after you saw the end could go to the
  ended turn, and a prompt could wait behind it. Now efrd refuses the steer and the
  interrupt, and the prompt starts at once.
- Ctrl+C while the auto sandbox starts a call now stops the call, with
  status 130. Before, the call could fail with status 125 and the message "the sandbox
  could not start", and efrd checked the sandbox again for no reason. A Ctrl+C that
  came just before bwrap started was lost, and the command ran to its end.
- The sandbox launcher no longer hangs when Ctrl+C ends bwrap while bwrap starts the
  sandbox.

## [0.0.2] - 2026-10-08

The reply view is new: replies stream without lag, a status row shows what runs, and
tool calls and questions have a clear layout. This release also fixes two sandbox
bugs.

### Added

- Streaming drafts. efrd sends the text of a running turn to the reply view every
  16 ms, and never writes these drafts to the event log. A reply now reaches the
  screen about 7 ms after the model sends it (it was about 100 ms).
- A status row under the reply while a turn runs: a spinner, the state (waiting for
  the model, thinking with the title of the reasoning, writing, preparing a tool call,
  stalled, waiting for you) and the time. The clock stops while a question waits.
  `render.motion = false` turns the shimmer off.
- An end-of-turn line, such as `done in 24s, 18.2k tokens in, 1.1k out`
  (`render.turn_summary`).
- A progress indicator in the terminal tab during a turn (OSC 9;4), in Ghostty 1.2 or
  newer, kitty 0.47 or newer and Windows Terminal, but not in tmux, GNU screen or
  zellij (`render.progress`).
- Colour roles for every part of the view: text, muted, accent, heading, link, code,
  success, warning, error, quote and the diff roles. `[render.colors]` sets one role,
  and `render.palette` loads a theme file with a `[colors]` table and an optional
  `code_theme` (a `.tmTheme` file). The eggfriedrice.nvim colour scheme ships such a
  file as `extras/efr/eggfriedrice.toml`.
- `render.theme = "auto"` with `render.theme_dark` and `render.theme_light`. The zsh
  plugin asks the terminal for its background once and sets `EFR_TERMINAL_BG`.
- Character widths by grapheme cluster in Ghostty, so emoji and flags no longer shift
  the live zone.
- Debug log lines with the time of each step of a call (`phase=... elapsed_ms=...`).
  `docs/sandbox.md` tells how to read them.
- `CHANGELOG.md`, and release notes that come from it.

### Changed

- Tool calls are blocks: `· $ command` in the code colour, a muted `│` output tail
  while it runs, and a result row (`✓ 0.4s` or `✗ exit 3 · 2.0s`). A failed call keeps
  its last 3 output lines. A long command wraps at the width and is never cut.
- Questions are cards with a thin muted bar and a title such as
  `? allow outside the sandbox`. After the answer, a card gives its place to one line,
  `✓ allowed` or `✗ denied`, and the command shows only once.
- One blank line between blocks, and none inside a block.
- Markdown: no label above plain code blocks, a file name when the fence names one,
  inline code in the code colour, one heading colour, quotes in italic, and a rule of
  at most 40 columns.
- The default accent colour is yellow (ANSI slot 3).
- `sudo -k` (and `-K`, `--reset-timestamp`, `--remove-timestamp`) can stand beside the
  one command of a line that runs outside the sandbox.
- A failed call no longer says `(sandbox)` in its result; the end-of-turn note says
  that the turn ran in the sandbox.
- A question no longer says that its target exists. It still says when a target is a
  directory, because a write grant then opens all of it.
- The reply view no longer prints `answer sent` after a password answer.

### Fixed

- A redirect to `/dev/null` (and to the other nodes of the sandbox's own `/dev`) was a
  write exit in `auto`: the line asked you and then ran outside the sandbox with your
  full rights. It now runs in the sandbox with no question.
- A command line that wraps inside a word no longer looks like two words; such a cut
  is marked with `↩`.
- When stdout is not a terminal, a call's rows come after its question and answer, as
  on a terminal.
- The status row no longer shows the reasoning title of an earlier model call.
- Ctrl+C during a turn says how long the turn ran.
- The cursor always comes back, also after a signal.

### Security

- A path below `/dev/fd/<n>/` or `/dev/pts/<n>/` counted as a node of the sandbox's
  own `/dev`, so a write such as `echo x > /dev/fd/3/authorized_keys` could run with no
  question. Now only `/dev/fd/<n>` and `/dev/pts/<n>` themselves count; a path below
  them is a write exit that only you can allow.

## [0.0.1] - 2026-10-07

The first release, on the AUR as `efr-code` and `efr-code-bin`.

### Added

- efrd, a user daemon with an event log in SQLite, one hidden zsh for each
  conversation and a terminal screen model (libghostty-vt).
- The efr CLI and the zsh plugin: a line that starts with a comma goes to the agent,
  and the reply streams below it. Sticky agent mode, `,new`, `,!` to steer a running
  turn, and `,mode`, `,model` and `,effort` for each terminal.
- The OpenAI provider, with the ChatGPT subscription login (`efr login openai`) or an
  API key.
- The tools `shell`, `read_file`, `write_file` and the settings tool, with approval
  questions in the terminal.
- Answers to programs that wait for input, such as `sudo` and `pacman`, also behind a
  relay, with hidden input for passwords.
- Permission modes `manual`, `cautious` and `auto`, with rules in `config.toml`.
- The `auto` sandbox: each command runs in bubblewrap with Landlock and seccomp; it
  writes only in the project, `$SCRATCH`, a private `/tmp` and private caches, and every
  action that leaves the sandbox asks you. `efr sandbox check` and
  `efr sandbox explain`.
- `efr project add`, `efr project list` and `efr project remove`.
- Settings in `config.toml` with live reload, `efr config` commands and `efr paths`.
- The AUR packages `efr-code` and `efr-code-bin`.

[Unreleased]: https://github.com/eggfriedrice24/eggfriedrice.code/compare/v0.0.2...HEAD
[0.0.2]: https://github.com/eggfriedrice24/eggfriedrice.code/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/eggfriedrice24/eggfriedrice.code/releases/tag/v0.0.1
