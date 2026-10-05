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
- The daemon changes how shells start while they run, when its config reloads:
  `set_start(program, login)` applies to shells spawned from then on, and
  `set_trusted_programs(programs)` to the trusted programs. A zsh reads that set once,
  when it starts, so a running one with another set would still run an alias of a
  newly trusted name. Its next `run_command` therefore restarts it first, in its
  current directory; while a command still runs in it, the run fails with
  `ShellError::Busy` instead, so no command ever runs with the old set.

`run_command(conversation, RunRequest, progress)` (also the `CommandRunner` trait that
`efr-tools` drives):

- With the integration, the command is typed as a key that empties the line editor
  (text typed at the attached screen and never sent must not join it), one bracketed
  paste and Enter, so a multi-line command is one command line. The output is the recording between the
  end of `C` (`OutputStart`) and the start of `D` (`CommandEnd`); the result has the
  exit status, the output as plain text (see "The output as text" below), a truncation
  flag (head and tail of `output_limit`, 1 MiB by default), the output's range in the
  recording and the directory after. `RunProgress` hears the output's size and tail as
  it grows (see "The live tail" below).
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
- A run with `RunRequest::interactive_limit` (the daemon sets it for a call that the
  user approved because it may wait for input) whose command runs keeps waiting past
  its timeout while `RunProgress::can_answer` says that a person who can type answers
  follows it, asked at the timeout and then once per `quiet_period`, up to the limit
  counted from the run's start. Once nobody follows, or the limit passes, the result
  is the one above. A run still waiting for its prompt is never kept past its timeout,
  and the stop of an unanswered hidden wait is the same with or without a limit.
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
  for `quiet_period`; `Visible` in any other modes (raw or cooked, echo on or off)
  when the output has been quiet for `visible_input_quiet` (3 s) and the main screen
  has the cursor after some text (a `[Y/n]` question); `None` otherwise. The screen is
  read only when the output is that quiet. A full-screen program on the alternate
  screen is judged only at the timeout, as above. A run that ends, is left at its
  timeout or is stopped while it waits reports `None` last.
- The modes cannot narrow a visible wait. A relay that runs a program on a terminal of
  its own, such as `sudo` with `use_pty` (which current releases turn on by default) or
  util-linux `script`, puts the hidden shell's terminal in raw mode with echo off and
  passes keys and output through: `pacman`'s `[Y/n]` question under `sudo` is asked
  there. The program on the inner terminal decides whether an answer is shown. So a
  password prompt that a program behind such a relay reads on its own terminal is
  `Visible` too, and its answer is shown only if that program echoes it; `sudo`'s own
  password prompt comes before the relay and is `Hidden`. A visible wait looks secret
  (`RunProgress::input_changed`'s `looks_secret`) when the terminal is not in line mode
  and the cursor's row names a password, a passphrase, a passcode, a PIN (as a word of
  its own), a verification code or a one-time code, in any case (`input::looks_secret`),
  so a client hides what the user types; the answer still goes as a visible one, so the
  kind check below holds. A change of the flag alone is a change too.
- A prompt that reads command lines (a shell's `$ `, a REPL's `>>> `) looks like a
  visible question, and an answer there would run as a command line. So two kinds of
  run report hidden waits only (`input::Offer`):
  - a sentinel run (`RunMode::Sentinel`), which types into a shell started inside the
    hidden one, whose prompt comes back after each command;
  - a run whose command line starts an interactive shell or a REPL, as its words show
    (`nested_shell.rs`): `zsh`, `bash`, `sh`, `dash`, `fish` or `ksh` with no command
    or script and input from the terminal, `su` without `-c`, `ssh` without a remote
    command, `sudo -i` or `-s`, `script` without `-c`, `python`, `node`, `irb`,
    `ghci`, `lua`, `psql` and `mysql` without code to run, `sqlite3` without SQL, and
    `docker`, `podman` or `kubectl exec -it` with a shell, a REPL or no command, also
    behind `sudo`, `env`, `exec`, `timeout` and the like.

  A shell's or a REPL's prompt is never a getpass-style read, so the password prompts
  of `sudo -i`, `su`, `ssh` and of a command in a nested shell are still reported. The
  rule holds for the whole run: `pacman -Syu && bash` reports no `[Y/n]` question
  either. What the words do not show (an alias, a function, a script, `docker run -it`)
  is not seen, and its prompt counts as a visible question.
- A wait belongs to a job: the look records the process group in the terminal's
  foreground with it, and tells the session's actor before it tells
  `RunProgress::input_changed`, so an answer that a client sends for the wait is
  checked against that job. A look that finds another job in the foreground reports
  `None`, and a wait of that job is a new change at the look after.
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
  prompt. Right before the signal it reads the foreground group again, and it sends the
  signal only while the job whose wait was reported holds the terminal. The command may
  have ended meanwhile: the shell's own group then holds the terminal (zsh in its
  precmd hooks, before `D`), where the signal could cut short the hook that prints `D`,
  or a command that a later precmd hook started does, which never waited. What is left
  is the moment between that read and the holder's own read of the group as it
  signals. A visible wait never stops a command: it is a guess, and a slow command whose
  last line is unfinished looks the same.
- `ShellSessions::answer_manual(conversation, call, text, hidden)` types an answer the
  user chose to type for a command that reported no wait (after `Ctrl+\` at a command
  that printed nothing for a while). It skips the check against the reported wait and
  its kind, and keeps the rest of the checks below: the text rules, this call's command
  runs, a job and not the shell itself holds the terminal, and for a hidden answer a
  line read with echo off. A visible manual answer can reach whatever runs in the
  foreground, also a nested shell's prompt; the user typed it there.
- `ShellSessions::answer(conversation, call, text, hidden)` types an answer for the
  tool call that `RunRequest::call` named. The text is one line of at most
  `efr_protocol::InputRespond::MAX_TEXT_BYTES` bytes without control characters
  (U+0000 to U+001F, U+007F), else `InvalidAnswer`; the session's actor checks that
  this call's command runs now (`NoCall` when no call's command runs: nothing was
  typed, or the run was left at its timeout and its command goes on without a call;
  `NotWaiting` when another call's command runs or this call's command has not started
  yet), reads the foreground group and the modes, refuses (`NotWaiting`) while the
  shell's own group holds the terminal, while the run reports no wait, while the group
  in the foreground is not the one recorded with the wait, when the answer is not of
  the wait's kind (a visible answer for a `Hidden` wait, a hidden one for a `Visible`
  wait), and for a hidden answer unless the terminal reads a line (canonical input on)
  with echo off, then writes the text and `\r` in one `writev` on the master. A
  visible answer takes any modes. Nothing awaits between the check and the write. The
  kind check matters because a visible answer passes the modes anywhere: once `sudo
  -i` took its password, its relay would run a visible answer sent for the password
  prompt as a command line, until the next look ends the hidden wait. The group checks
  matter because zsh takes the terminal back when the job ends and runs its precmd
  hooks in cooked mode and its line editor in raw mode, before `D` reaches the
  session: no check of the modes could keep an answer from the line editor there. An
  answer written before zsh takes the terminal back is thrown away by the
  integration's drain (below), which runs first among the precmd hooks. A precmd hook
  that runs after the integration's and starts an external command puts it in the
  foreground, in a process group of its own and in cooked mode, after the drain; an
  answer tried then, before the session has read `D`, would wait there for the line
  editor, which would run it as the next command line once the hook ends, so the group
  recorded with the wait is what refuses it. What is left is a new wait: while such a
  command holds the terminal and `D` still has not reached the session, a look reports
  `None`, the look after reports a wait of that command when it looks like one (the
  cursor after text, or a getpass-style read), and an answer to that wait is typed for
  that command. That takes the reader held up on the chunk with `D` for two looks, as
  a slow `RecordingSink` can hold it, while the hook's command holds the terminal. An
  answer counts as activity: the next look reports `None`, so a prompt that is asked
  again (`Sorry, try again.`) is a new change. The text is a `SecretText` throughout:
  no error, `Debug` output or log carries it, and the terminal does not echo a hidden
  answer into the output or the recording (the program that reads it can still print
  it), with the one exception below.
- Two limits of an answer are known and left as they are. The modes are read and the
  answer written in one step, with no await in between, but nothing locks the terminal:
  in the microseconds between `tcgetattr` and `writev` the program can change its
  modes, and when `sudo`'s own password timeout (five minutes by default) turns echo
  back on right then, the terminal echoes the hidden answer into the output, the
  recording and what the model reads. And an answer is tied to its call and to the job
  whose wait was reported, not to the prompt that the user saw: when one hidden prompt
  of that job ends and another of the same job starts, a client learns of it only at
  the next look, up to `quiet_period` later (never, when the new prompt prints nothing,
  because the wait stays `Hidden`), so an answer typed for the first prompt in that
  time goes to the second.
- `sudo` keeps its usual credential cache on the hidden shell's terminal (about five
  minutes), so a `sudo` soon after an answered one may not ask again, unless the run
  asks to forget it (`RunRequest::forget_credentials`, the daemon's
  `shell.sudo_cache = "per_call"`). Then, once the run's `D` mark came, the session
  types the key `\e[efr-forget~` at the next prompt's `B`, ahead of any line; the
  integration binds it to a widget that runs `sudo -k` and `doas -L` (each when it is
  on the `PATH`) with every stream redirected, so nothing reaches the screen, the
  recording or a tool result. The key waits for `B` because before the line editor
  reads, the terminal is in cooked mode and would echo it. A run that was left at its
  timeout (sudo at its password prompt) still forgets when it ends. A run delimited
  by sentinels (a nested shell) forgets at the outer zsh's next `B` too: right after
  its end marker when it was typed at that prompt, and only when the nested shell
  exits when it was typed into one. A sentinel run that comes after the end and
  before that `B` waits for the `B` as well, so its line follows the key: the key
  behind the line would reach the line's command, which could read it as part of an
  answer, or the drain before the next `D` would throw it away and sudo would keep
  its credentials. A shell without the integration (not a zsh, or
  the integration did not load) has no binding and no marks, so it keeps the cache;
  the session logs a warning once when such a shell gets a run that should forget.

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

The live tail (`live_tail.rs`), the `tail` of each `OutputUpdate` while a command runs
(a client shows its last line, and the daemon records it as
`ToolCallOutputUpdated.tail`):

- It reads the last 4 KiB of the output (`PREVIEW_BYTES`) by the rule of the finished
  output. Bytes that the cleaner reads right are cleaned at every change, which costs
  no screen: a carriage-return bar shows its current line.
- Bytes that move the cursor are replayed on a screen from the session's own
  `ScreenFactory`, named `tail-<last eight hex digits>`, of the shell's current size,
  and read without its scrollback: the tail is the rows that the screen shows, as
  `screen_text` reads them. So a multi-line progress display redrawn with cursor-up
  shows its current frame, where the cleaner would show every old frame after it. When
  the 4 KiB start inside the output, the screen reads them from after their first
  line feed, so it never starts inside an escape sequence. A full-screen program
  still running ends on the copy, so the tail shows the main screen and the note line.
- A screen is read at most once per `ShellConfig::tail_interval` (200 ms, the
  conversation's default update interval) on the injected clock: the first change that
  needs one is read at once, a change within the interval is held, and the newest held
  change is read when the interval has passed. A change that needs no screen replaces
  a held one. The screen lives for one read, in the caller's task, and is shut down
  when the read ends or the caller drops the run.
- When the screen cannot start or stops early, the cleaner reads the bytes, as for
  the finished output.

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
after the user's `.zshenv`, before `.zprofile` and `.zshrc` run. It sets `PAGER`,
`GIT_PAGER`, `SYSTEMD_PAGER`, `MANPAGER`, `GH_PAGER` and `BAT_PAGER` to `cat` and
`AWS_PAGER` to the empty value: nobody reads a pager on the hidden screen, so `git log`
or `systemctl status` would otherwise open `less` there and the run would wait until
someone quit it. A zsh starts as an interactive login shell (`-l -i`, `login` in the
config).

Nobody can use an editor on the hidden screen either, so the shell sets `EDITOR`,
`VISUAL`, `GIT_EDITOR`, `GIT_SEQUENCE_EDITOR`, `SUDO_EDITOR` and `SYSTEMD_EDITOR` to
the path of `efr-editor` in `ShellConfig::integration_dir` (`assets/efr-editor`, a
`/bin/sh` script written there with mode 0700 before the first shell starts, a zsh or
not). It prints `efr: there is no editor in the hidden shell; pass the text another
way, such as git commit -m, a file, or ask the user to edit it` on stderr and exits 1,
so `git commit` without `-m`, `git merge` without `--no-edit`, `git rebase -i`,
`crontab -e`, `systemctl edit` and `sudoedit` fail at once instead of waiting for the
run's timeout. `GIT_EDITOR` and `GIT_SEQUENCE_EDITOR` win over the user's
`core.editor` and `sequence.editor`. A program that `sudo` starts sees the variables
only when `sudo` keeps them (`visudo` under `sudo`'s default `env_reset` does not),
and a path with spaces in the integration directory would split the variables.

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
  efr keeps its own recording), no pager and no editor (the pager and editor
  variables are set again, because a `.zshrc` often exports `PAGER=less` or
  `EDITOR=vim`; the script finds `efr-editor` next to itself), no `NULL_GLOB` or
  `CSH_NULL_GLOB` (the permission engine counts a pattern that matches nothing as one
  word, so it must not vanish), no global or suffix aliases and no alias or function
  named like one of `ShellConfig::trusted_programs` (removed after the startup files
  and again by the key efr types before each command, because they change what a line
  that the permission engine allowed runs), and zsh's
  `PROMPT_SP` mark is printed after `D` instead of before the precmd hooks, so it
  never counts as output.
- `compinit` runs as `compinit -i` in the hidden shell, whoever calls it: a startup
  file, or a plugin that loads after the first prompt. Without `-i` it asks before it
  loads completions from a directory of `fpath` that other users can write to, and
  waits for a key that nobody types, so the first prompt never comes. Ubuntu's
  `/etc/zsh/zshrc` calls it in every interactive shell, and GitHub's Ubuntu runner
  image makes all of `/usr/share` writable by everyone. `-i` leaves those directories
  out of `fpath`, as the answer `y` does; a caller's `-u` or `-C` still wins.
- When a command ends, the precmd hook throws away input that reached the terminal and
  that the command never read (`builtin read -s -t 0 -k 1` until nothing is left),
  before it prints `D`. Without it, an answer written just as `sudo` gave up, or keys
  typed at the attached screen, would be read by the line editor as the next command
  line: `hunter2` and Enter would run, show on the screen, land in the recording and
  reach the model. The drain cannot eat a marked command efr types: an `Auto` run is
  typed only at a ready prompt (after `B`, which comes after `D`), and a run left at its
  timeout holds the next one until its `D`. A sentinel run is typed at once, without
  waiting for `D`, so a sentinel line that arrives as a command ends is drained: one
  typed into a nested shell that just ended was meant for that shell and is rightly
  dropped, and its run waits for its timeout without an end marker.

## Tier

Tier 2.

## Allowed dependencies

`efr-holder` (the `PtyHolder` trait, spawn specs, child status), `efr-screen` (the
screen handle, the mark scanner, `row_text`), `efr-protocol` (ids, `Seq`, the screen
snapshot types) and `efr-stdx` (`Clock`, `Rng`, UUIDv7 ids, atomic writes, the
scrubbed variable list). `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `tokio` (tasks, channels, `AsyncFd`), `bytes`, `rustix` (`fcntl` for
`O_NONBLOCK`, `tcgetattr` and `tcgetpgrp` for the input modes and the foreground group,
`writev` for an answer), `which` (finding zsh on the given `PATH`), `jiff` (the clock's
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
  thread; only output that moves the cursor starts one. The same holds for the screen
  of a live tail, which a run reads at most once per `tail_interval`.
- Time and randomness are injected: every timeout runs on the `Clock`, and PTY ids
  and sentinel tokens come from the `Rng`.
- No error, notice or `Debug` output carries a command line or an environment value,
  or the text of an answer.
- An answer reaches a terminal only while the same call's command runs, as far as the
  session has read the stream (`D` not yet read), the run reports a wait of the
  answer's kind, the job in the foreground is the one that was there at the look that
  reported it (by its process group), and, for a hidden answer, the terminal reads a
  line with echo off, all as read right before the write. A manual answer needs no
  reported wait and no kind, but still a job, not the shell itself, in the foreground. A sentinel run and a run
  whose command line starts a shell or a REPL report no visible wait. The stop of an
  unanswered hidden wait signals only while that job holds the terminal, as read right
  before the signal.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-shell
EFR_TEST_ZSH=1 cargo nextest run -p efr-shell   # also the e2e_ tests over a real zsh
just test-shell                                  # the same, with zsh installed
just test-shell-ubuntu                           # CI's shell job in an Ubuntu container
```

The startup files of the system still run in the `e2e_` tests, and they differ between
distributions: Ubuntu's `/etc/zsh/zshrc` binds keys, defines `zle-line-init` and calls
`compinit`. `just test-shell-ubuntu` runs CI's shell job in an Ubuntu 24.04 container
(`.github/ubuntu-shell.Dockerfile`) set up like GitHub's runner, with zsh from apt and
`/usr/share` writable by everyone, so these tests can be checked there before a push.
It needs Docker and keeps its cargo registry and target directory in `efr-ci/` under
`$XDG_CACHE_HOME` (`~/.cache` when unset).

The unit tests drive the manager over a fake holder whose PTY is a socketpair: the
test plays the shell with scripted bytes, sets the terminal modes through a fake
`TerminalModes` and moves a manual clock, so nothing waits on real time. `modes.rs`
reads the modes of real PTY pairs it opens. `nested_shell.rs` is tested with tables of
command lines that do and do not leave a shell or a REPL at its prompt. The test
recording sink can hold back the chunk that holds a given mark, as a slow store holds
the reader, which is how an `e2e_` test answers while a later precmd hook's command
holds the terminal and `D` has not reached the session. The `e2e_` tests (module
`e2e_zsh`) spawn a real zsh through `efr_pty::LocalPtyHolder`, watched through a vt100
screen, in a throwaway home with empty startup files; they skip with a message unless
`EFR_TEST_ZSH=1`, and nextest runs them one at a time in the `shell` test group. No
test uses the network, the user's home or the user's zsh configuration.
