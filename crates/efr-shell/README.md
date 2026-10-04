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

- With the integration, the command is typed as one bracketed paste and Enter, so a
  multi-line command is one command line. The output is the recording between the
  end of `C` (`OutputStart`) and the start of `D` (`CommandEnd`); the result has the
  exit status, the output as plain text (escape sequences dropped, `\r\n` as `\n`,
  carriage-return progress bars collapsed), a truncation flag (head and tail of
  `output_limit`, 1 MiB by default), the output's range in the recording and the
  directory after. `RunProgress` hears the output's size and tail as it grows.
- A run waits for the prompt: while the shell starts, while an earlier command still
  runs, and until the user answers what it asks; it fails with `NotReady` when the
  prompt does not come before its timeout. A run that overlaps another run of the same
  conversation, or a shell at a continuation prompt, is `Busy`.
- An unfinished line (an unclosed quote) shows a continuation prompt; the session
  cancels it with a key the integration binds to `send-break` and returns
  `Completion::NotStarted`. A line that does not parse is `NotStarted` too, with the
  shell's complaint as its output.
- When no `D` arrives before the timeout, the command keeps running and the result
  is `Completion::Interactive` with the screen's last lines when it waits for input
  (it has been quiet for `quiet_period` and the cursor sits after some text, or a
  full-screen program is on the alternate screen), and `Completion::StillRunning`
  otherwise.
- Without the integration (a shell that is not a zsh, or a zsh whose first marked
  prompt did not come within `startup_timeout`), and for `RunMode::Sentinel` (a shell
  started inside the hidden one: `sudo -i`, `bash`, `ssh`), the command is delimited
  with a random-token sentinel: `printf '__efr_%s_b\n' TOKEN; eval 'COMMAND'; printf
  '\n__efr_%s_e:%s:%s\n' TOKEN "$?" "$PWD"`. The echo of the typed line never contains
  a marker, because the token is never next to `__efr_` there.

Environment hygiene: the shell inherits `ShellConfig::base_env` (the user's
environment, passed in by the daemon; this crate reads no environment) without
`efr_stdx::process::SCRUBBED_ENV` (the daemon's systemd unit), the `EFR_*` and
`_EFR_*` variables, the session variables of terminals and multiplexers
(`TERM_PROGRAM`, `TERMINFO`, `TMUX`, `GHOSTTY_*`, `KITTY_*` and the like), `COLUMNS`,
`LINES`, `SHLVL`, `OLDPWD` and `_`. It sets `TERM` and `COLORTERM` (default
`xterm-256color` and `truecolor`), `PWD`, and `EFR_HIDDEN_SHELL=1`, which nothing in
efr reads but the user's startup files can test (to skip `exec tmux` or an instant
prompt). A zsh starts as an interactive login shell (`-l -i`, `login` in the config).

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
  efr keeps its own recording), and zsh's `PROMPT_SP` mark is printed after `D`
  instead of before the precmd hooks, so it never counts as output.

## Tier

Tier 2.

## Allowed dependencies

`efr-holder` (the `PtyHolder` trait, spawn specs, child status), `efr-screen` (the
screen handle, the mark scanner, `row_text`), `efr-protocol` (ids, `Seq`, the screen
snapshot types) and `efr-stdx` (`Clock`, `Rng`, UUIDv7 ids, atomic writes, the
scrubbed variable list). `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `tokio` (tasks, channels, `AsyncFd`), `bytes`, `rustix` (`fcntl`
for `O_NONBLOCK`), `which` (finding zsh on the given `PATH`), `jiff` (the clock's
timestamps), `async-trait`, `thiserror` and `tracing`.

Dev-dependencies: `efr-pty` and `efr-screen-vt100` for the e2e tests,
`efr-test-support` for the manual clock and the seeded generator.

## Invariant

- The shell is reached only through `Arc<dyn PtyHolder>` and a `PtyHandle`, so the
  holder milestone (efr-ptyd) changes no code here.
- Marks come from the stream itself, scanned in order with the bytes around them, so a
  command's output is exactly `recording[C.end .. D.start]` whatever the screen
  backend.
- Time and randomness are injected: every timeout runs on the `Clock`, and PTY ids
  and sentinel tokens come from the `Rng`.
- No error, notice or `Debug` output carries a command line or an environment value.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-shell
EFR_TEST_ZSH=1 cargo nextest run -p efr-shell   # also the e2e_ tests over a real zsh
just test-shell                                  # the same, with zsh installed
```

The unit tests drive the manager over a fake holder whose PTY is a socketpair: the
test plays the shell with scripted bytes and moves a manual clock, so nothing waits on
real time. The `e2e_` tests (module `e2e_zsh`) spawn a real zsh through
`efr_pty::LocalPtyHolder`, watched through a vt100 screen, in a throwaway home with
empty startup files; they skip with a message unless `EFR_TEST_ZSH=1`, and nextest
runs them one at a time in the `shell` test group. No test uses the network, the
user's home or the user's zsh configuration.
