# efr-shell

## Purpose

One long-lived hidden zsh per conversation, and running commands in it. The model's
commands run in a real interactive shell with the user's own startup files, so they
show on a screen that the user (and later the phone) can watch, and interactive
prompts such as `sudo` stay visible.

`ShellSessions` is the manager:

- It spawns a conversation's shell on first use through `Arc<dyn PtyHolder>`, in the
  directory the caller names, and gives it a screen from the injected
  `ScreenFactory`, so the daemon chooses vt100 or ghostty. The screen thread is
  named `screen-<last eight hex digits of the conversation id>`.
- It reads and writes the PTY master through tokio's `AsyncFd`. The holder hands the
  master over in blocking mode; the session sets `O_NONBLOCK` itself. Every chunk read
  goes to the `RecordingSink` first (the daemon appends it to the PTY recording), then
  to the session's actor, then to the screen. Writes (typed lines, `pty.write` input,
  the screen's answers to terminal queries) go through one writer task that never
  makes a sender wait on the shell.
- The actor scans the stream for OSC 133 and OSC 7 marks with efr-screen's
  `ShellMarkScanner` and keeps a `ShellState`: the `Phase` (starting, prompting,
  ready, continuation, running, finished, unmarked), the working directory, the last
  exit status. It reports `ShellNotice::{Started, CwdChanged, Exited}` to the
  `ShellObserver`, and learns that the shell ended from `PtyHolder::wait`; it then
  releases the PTY, and the conversation's next run spawns a new shell.

`run_command(conversation, RunRequest, progress)` (also the `CommandRunner` trait that
`efr-tools` drives):

- With the integration, the command is typed as a key that empties the line editor
  (text typed at the attached screen and never sent must not join it), one bracketed
  paste and Enter, so a multi-line command is one command line. The output is the recording between the
  end of `C` (`OutputStart`) and the start of `D` (`CommandEnd`); the result has the
  exit status, the output as plain text (see "The output as text" below), a truncation
  flag (head and tail of `output_limit`, 1 MiB by default), the output's range in the
  recording and the directory after. `RunProgress` hears the output's size and tail as
  it grows.
- A run waits for the prompt: while the shell starts, while an earlier command still
  runs, and until the user answers what it asks; it fails with `NotReady` when the
  prompt does not come before its timeout. A run that overlaps another run of the same
  conversation, or a shell at a continuation prompt, is `Busy`. A caller that drops
  the run's future (a turn the user interrupted) lets go of it: a run still waiting is
  never typed, and a typed one goes on without a caller until its `D`.
- An unfinished line (an unclosed quote) shows a continuation prompt; the session
  cancels it with a key the integration binds to `send-break` and returns
  `Completion::NotStarted`. A line that does not parse is `NotStarted` too, with the
  shell's complaint as its output.
- When no `D` arrives before the timeout, the command keeps running and the result,
  with the screen's last lines, is `Completion::FullScreen` when a full-screen program
  is on the alternate screen (quiet or not: nobody can reach it until an attach
  exists), `Completion::Interactive` when it waits for input (it has been quiet for
  `quiet_period` and the cursor sits after some text), and `Completion::StillRunning`
  otherwise.
- Without the integration (a shell that is not a zsh, or a zsh whose first marked
  prompt did not come within `startup_timeout`), and for `RunMode::Sentinel` (a shell
  started inside the hidden one: `sudo -i`, `bash`, `ssh`), the command is delimited
  with a random-token sentinel: `printf '__efr_%s_b\n' TOKEN; eval 'COMMAND'; printf
  '\n__efr_%s_e:%s:%s\n' TOKEN "$?" "$PWD"`. The echo of the typed line never contains
  a marker, because the token is never next to `__efr_` there.

Waiting for input (`input.rs`, `modes.rs`), while a run's command runs (between `C`
and `D`, or the two sentinels):

- The run looks once per `quiet_period` (1 s), on the injected clock, and tells
  `RunProgress::input_changed` of each change of `efr_protocol::InputWait`, once:
  `Hidden` when the terminal has echo off and canonical input on (a getpass-style
  read: `sudo`, `ssh`, `su`, `passwd`, polkit's agent) and the output has been quiet
  for `quiet_period`; `Visible` when echo and canonical input are on, the output has
  been quiet for `visible_input_quiet` (3 s) and the main screen has the cursor after
  some text (a `[Y/n]` question); `None` otherwise. The screen is read only when the
  output is that quiet. A full-screen program on the alternate screen is judged only
  at the timeout, as above. A run that ends, is left at its timeout or is stopped
  while it waits reports `None` last.
- The modes come from `tcgetattr` on the master (Linux returns the slave's termios
  there) through `ShellDeps::modes`, a `TerminalModes` (`Termios` by default; a test
  injects a fake, because its PTY is a socketpair). The same trait reads the
  terminal's foreground process group (`tcgetpgrp` on the master). Only a job's modes
  count: while the shell's own group holds the terminal (its prompt, its hooks, or a
  builtin such as `read` or a function that it runs itself), nothing waits, because
  input typed then reaches the shell. Modes that cannot be read never count as
  waiting.
- While the command waits for hidden input, the run asks
  `RunProgress::can_answer_hidden` at the change and at every look. `false` (the daemon
  says it when no client that can type answers follows the conversation) detaches the
  run, sends `SIGINT` to the foreground process group, as `interrupt` does, and returns
  `Completion::Unanswered` with the output so far at once; the next run waits for the
  prompt. A visible wait never stops a command: it is a guess, and a slow command whose
  last line is unfinished looks the same.
- `ShellSessions::answer(conversation, call, text, hidden)` types an answer for the tool
  call that `RunRequest::call` named. The text is one line of at most 1024 bytes without
  control characters (U+0000 to U+001F, U+007F), else `InvalidAnswer`; the session's
  actor checks that this call's command runs now (`NoCall` when no call's command runs:
  nothing was typed, or the run was left at its timeout and its command goes on without
  a call; `NotWaiting` when another call's command runs or this call's command has not
  started yet), reads the foreground group and the modes, refuses (`NotWaiting`) while
  the shell's own group holds the terminal, when canonical input is off, and for a
  hidden answer when echo is on, then writes the text and `\r` in one `writev` on the
  master. Nothing awaits between the check and the write. The group check matters
  because zsh takes the terminal back when the job ends and runs its precmd hooks in
  cooked mode, before `D` reaches the session: canonical input alone would let an answer
  through there, to be read by the line editor. An answer counts as activity: the next
  look reports `None`, so a prompt that is asked again (`Sorry, try again.`) is a new
  change. The text is a `SecretText` throughout: no error, `Debug` output or log carries
  it, and a hidden answer never reaches the output or the recording.
- `sudo` keeps its usual credential cache on the hidden shell's terminal (about five
  minutes), so a `sudo` soon after an answered one may not ask again. Nothing here
  clears it (no `sudo -k`); a later setting will control that.

The output as text (`capture.rs`, `replay.rs`), for a finished run and for the output
so far of a run left running at its timeout:

- The run keeps at most `output_limit` bytes: the first half from the start of the
  output, the second half from its end. When bytes were dropped, the head and the
  tail are read apart and a line `[... N bytes omitted ...]` joins them, so no reader
  holds more than the kept bytes.
- Output that only prints text, colours, carriage-return progress bars and
  backspaces goes through the byte cleaner: escape sequences dropped, `\r\n` as `\n`,
  the text after a bare `\r` replacing the line, a backspace removing the character
  before it, tabs kept. This is most output, and it costs no screen. A screen would
  give the same lines, except that it turns tabs into spaces.
- Output that moves the cursor (cursor movement and positioning, erasing more than
  the rest of a line, scrolling, inserting or deleting, saving and restoring the
  cursor, the alternate screen) is replayed on a capture screen: one from the
  session's own `ScreenFactory`, named `replay-<last eight hex digits>`, at the
  shell's current width (it follows `resize`), with as many rows as the output can
  fill, at least the shell's height and at most 200, and read back with its
  scrollback. The text is the rows that the screen shows at the end: a soft-wrapped
  row joined to the next, colours dropped, wide characters kept, trailing blanks and
  blank rows at the end removed. So a multi-line progress display that redraws itself
  with cursor-up leaves only its last frame.
- A program that switched to the alternate screen leaves what is on the main screen
  and one line `[a full-screen program ran here; only what it left on the main screen
  is shown]`. Output that ends on the alternate screen is switched back first.
- A row that scrolled off the top is final, because no cursor movement reaches it.
  Every backend keeps at least 1000 rows of scrollback; a replay whose scrollback
  reaches that many may have lost its oldest rows, so it is replayed again in two
  halves split at the line end nearest the middle, as often as needed. A display that
  redraws itself across such a split shows one extra frame there.
- Output that sets a scroll region (`apt`'s progress bar) stays on the cleaner,
  because a screen drops the lines that scroll out of a region.
- The capture screen is shut down when its replay ends, and also when the caller
  drops the run, so a finished run keeps no thread. The replay runs in the caller's
  task, never in the session's actor. When the screen cannot start or stops early,
  the cleaner reads the bytes and the failure is logged at `debug`.
- A line that ran no command (`NotStarted`) is read by the cleaner: its text is the
  shell's complaint around the echo of the line, which the line editor drew relative
  to a prompt that a capture screen does not have.

Environment hygiene: the shell inherits `ShellConfig::base_env` (the user's
environment, passed in by the daemon; this crate reads no environment) without
`efr_stdx::process::SCRUBBED_ENV` (the daemon's systemd unit), the `EFR_*` and
`_EFR_*` variables, the session variables of terminals and multiplexers
(`TERM_PROGRAM`, `TERMINFO`, `TMUX`, `GHOSTTY_*`, `KITTY_*` and the like), `COLUMNS`,
`LINES`, `SHLVL`, `OLDPWD` and `_`. It sets `TERM` and `COLORTERM` (default
`xterm-256color` and `truecolor`), `PWD`, and `EFR_HIDDEN_SHELL=1`, which nothing in
efr reads but the user's startup files can test (to skip `exec tmux` or an instant
prompt). A zsh with the integration also gets `_EFR_HS_TRUSTED_PROGRAMS`, the
trusted programs separated by spaces, which the integration reads and unsets right
after the user's `.zshenv`, before `.zprofile` and `.zshrc` run. It sets `PAGER`, `GIT_PAGER`, `SYSTEMD_PAGER` and `MANPAGER` to `cat`:
nobody reads a pager on the hidden screen, so `git log` or `systemctl status` would
otherwise open `less` there and the run would wait until someone quit it. A zsh starts as an interactive login shell (`-l -i`, `login` in the config).

The zsh integration (`assets/zsh/`, embedded with `include_str!`, written to
`ShellConfig::integration_dir` on the first spawn):

- `.zshenv` is a ZDOTDIR shim. zsh reads it because the session points `ZDOTDIR` at
  that directory. It puts the user's `ZDOTDIR` back (saved in `_EFR_USER_ZDOTDIR`) or
  unsets it, so zsh reads the user's `.zprofile`, `.zshrc` and `.zlogin` next and child
  shells never see the shim; it sources the user's `.zshenv`, then the integration.
- `efr-integration.zsh` emits exactly the sequence set of ghostty's zsh integration:
  `OSC 133;A;cl=line` before each primary prompt (precmd), `OSC 133;P;k=s` before a
  continuation prompt and `OSC 133;B` where the input starts (zle-line-init),
  `OSC 133;C` from preexec, `OSC 133;D;<status>` from precmd, a bare `OSC 133;D` after
  a prompt whose line ran nothing, and `OSC 7;kitty-shell-cwd://<host><path>` on every
  change of directory. Its precmd hook runs first and its preexec hook last, so no
  other hook's output lands between `C` and `D`; the directory report goes out before
  `D`. It is an original script: ghostty's is GPLv3 and is never copied.
- The integration changes a few options in the hidden shell only: no `!` history
  expansion, no spelling correction prompts, no history file (`HISTFILE` is unset;
  efr keeps its own recording), no pager (the four pager variables are set to `cat`
  again, because a `.zshrc` often exports `PAGER=less`), no `NULL_GLOB` or
  `CSH_NULL_GLOB` (the permission engine counts a pattern that matches nothing as one
  word, so it must not vanish), no global or suffix aliases and no alias or function
  named like one of `ShellConfig::trusted_programs` (removed after the startup files
  and again by the key efr types before each command, because they change what a line
  that the permission engine allowed runs), and zsh's
  `PROMPT_SP` mark is printed after `D` instead of before the precmd hooks, so it
  never counts as output.
- When a command ends, the precmd hook throws away input that reached the terminal
  and that the command never read (`builtin read -s -t 0 -k 1` until nothing is left),
  before it prints `D`. Without it, an answer written just as `sudo` gave up, or keys
  typed at the attached screen, would be read by the line editor as the next command
  line: `hunter2` and Enter would run, show on the screen, land in the recording and
  reach the model. The drain cannot eat a command efr types: an `Auto` run is typed only
  at a ready prompt (after `B`, which comes after `D`), a run left at its timeout holds
  the next one until its `D`, and a sentinel run typed into a nested shell that just
  ended is meant for that shell and is rightly dropped.

## Tier

Tier 2.

## Allowed dependencies

`efr-holder` (the `PtyHolder` trait, spawn specs, child status), `efr-screen` (the
screen handle, the mark scanner, `row_text`), `efr-protocol` (ids, `Seq`, the screen
snapshot types) and `efr-stdx` (`Clock`, `Rng`, UUIDv7 ids, atomic writes, the
scrubbed variable list). `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `tokio` (tasks, channels, `AsyncFd`), `bytes`, `rustix` (`fcntl`
for `O_NONBLOCK`, `tcgetattr` for the input modes, `writev` for an answer), `which` (finding zsh on the given `PATH`), `jiff` (the clock's
timestamps), `async-trait`, `thiserror` and `tracing`.

Dev-dependencies: `efr-pty` and `efr-screen-vt100` for the e2e tests (vt100 also
backs the capture screens of the replay tests), `efr-test-support` for the manual
clock and the seeded generator.

## Invariant

- The shell is reached only through `Arc<dyn PtyHolder>` and a `PtyHandle`, so the
  holder milestone (efr-ptyd) changes no code here.
- Marks come from the stream itself, scanned in order with the bytes around them, so a
  command's output is exactly `recording[C.end .. D.start]` whatever the screen
  backend.
- A capture screen lives for one replay and no longer, so a finished run keeps no
  thread; only output that moves the cursor starts one.
- Time and randomness are injected: every timeout runs on the `Clock`, and PTY ids
  and sentinel tokens come from the `Rng`.
- No error, notice or `Debug` output carries a command line or an environment value,
  or the text of an answer.
- An answer reaches a terminal only while the same call's command runs and the
  terminal reads a line; a hidden one only while echo is off.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-shell
EFR_TEST_ZSH=1 cargo nextest run -p efr-shell   # also the e2e_ tests over a real zsh
just test-shell                                  # the same, with zsh installed
```

The unit tests drive the manager over a fake holder whose PTY is a socketpair: the
test plays the shell with scripted bytes, sets the terminal modes through a fake
`TerminalModes` and moves a manual clock, so nothing waits on real time. `modes.rs`
reads the modes of real PTY pairs it opens. The `e2e_` tests (module `e2e_zsh`) spawn a real zsh through
`efr_pty::LocalPtyHolder`, watched through a vt100 screen, in a throwaway home with
empty startup files; they skip with a message unless `EFR_TEST_ZSH=1`, and nextest
runs them one at a time in the `shell` test group. No test uses the network, the
user's home or the user's zsh configuration.
