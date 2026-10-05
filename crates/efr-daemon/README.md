# efr-daemon

## Purpose

`efrd`, the efr daemon, and the composition root of the workspace. It runs as the
systemd user unit `systemd/efrd.service` and owns the conversations, the hidden shells
and their screens, the event log, the providers and the login. Every client speaks to
it over the Unix socket at `$XDG_RUNTIME_DIR/efr/daemon.sock`.

The crate is a library with a thin binary: `main.rs` parses `--log`, `--screen` and
`--print-config`, loads the config, sets up tracing and calls `run`. The library exists
so that `efr-test-daemon` can run the real daemon in-process with injected `Deps`
(directories, home, clock, generator, PTY holder, screen factory, provider factory, an
in-memory database and isolated git). It re-exports `PtyHolder` with the types its
methods name (`SpawnSpec`, `PtyHandle`, `PtyInfo`, `ChildStatus`, `Signal`,
`SignalTarget`, `HolderError`), so a test holder implements the trait without an
`efr-holder` edge of its own.

### Startup order

`run::start` follows ARCHITECTURE.md:

1. `lock.rs`: the exclusive `flock` on `$XDG_DATA_HOME/efr/daemon.lock`; a second
   daemon exits with "another efrd is running". The config is loaded just before, in
   `main.rs`, because tracing needs its `log` value; reading it changes nothing.
2. `config.rs`: a thin layer over `efr-config`, which owns the file
   (`$XDG_CONFIG_HOME/efr/config.toml`), its keys, defaults and checks, with unknown
   keys refused; this layer applies `EFR_LOG` and `EFR_SCREEN`, then the flags.
   `efrd --print-config` prints every value with its source. `[[permissions.rules]]`
   holds the user's permission rules in the `efr_permissions::Rule` form; a rule of
   the wrong shape or one that names a relative path, a program that is not one word
   or an action its resource never matches stops the start with an error that names
   `permissions.rules[N]`, counted from 0 (`docs/permissions.md`). `[model] name`,
   `[model] effort` and `permissions.mode` are the defaults of a turn's settings. A
   `[model] name` that is not in the model list, or a `[model] effort` that the
   default model does not take, costs a warning at start, because every turn that
   leaves them to the config then fails. The engine decides each tool call by the
   turn's mode.
3. The store: the backup copy in `backups/`, the forward-only migrations.
4. `reconcile.rs`: running turns cancelled, pending approvals expired, queued prompts
   held, running shells recorded as exited, process-bound outbox items cancelled.
5. The PTY table, the recording sink, the shells, the providers (`providers.rs`), the
   tool registry and the settings tool (`tools.rs`), the permission engine and the
   conversation registry.
   The engine is built in one place, `engine.rs`, from the settings, the project
   registry and the config directory. It holds one machine policy for each permission
   mode, `Policy::base(mode)` followed by the user's rules, so a user rule wins where
   both match; the user's rules are the machine policy and not a conversation's,
   because only the machine policy may open a secret or a system path. Each turn passes
   its mode (today `permissions.mode`, through `LiveSettings`). The config directory,
   its resolved form and what each symbolic link in it reaches (`protected_config`,
   such as a `config.toml` in a dotfiles repository) are write-sealed: no tool writes
   them in any mode, whatever the rules say. The hidden shells trust the programs of
   the `auto` policy, which names those of every mode.
   `State` holds the settings in a `watch` of `Arc<Settings>` and the engine in
   another: the conversations read the settings through `settings.rs` (`LiveSettings`)
   when a turn starts or a prompt arrives, and each tool call reads the engine.
   A change of the mode needs no new engine: a turn passes its own.
6. The background tasks: the shells' lifecycle events, the notices (`notices.rs`), the
   idle shell collector (`gc.rs`, which reads `shell.idle_minutes` at each look), the
   reload task (`reload.rs`), the config file watcher (`reload/watcher.rs`) and the
   SIGHUP listener.
7. The Unix socket (0600) and `daemon.json` (`discovery.rs`), then `READY=1` through
   `sd-notify`. The socket path is checked first of all: a path longer than the 107
   bytes a Unix socket holds, as a runtime root deep below `EFR_HOME` can make it,
   stops the start with an error that names the path and says to set
   `EFR_RUNTIME_DIR` to a shorter directory.

SIGTERM or SIGINT (`signals.rs`) cancels the serve: the connections end, the background
tasks stop, the conversation actors and the shells are shut down, the recordings are
closed, `daemon.json` is removed, the database is closed, and the lock is released last.

### Live reload

`reload.rs` reads `config.toml` again while the daemon runs. Four triggers ask its one
task for a reload: `admin.config_reload` (`efr config reload`), SIGHUP
(`systemctl --user reload efrd`, through `ExecReload` in the unit), the file watcher,
and the settings tool after it wrote the file. The watcher (`reload/watcher.rs`) is the daemon's own, on inotify through
rustix's safe API and tokio's `AsyncFd` (non-blocking, close-on-exec; efrd runs on
Linux only). It watches the config root and, when `config.toml` is a symbolic link,
the directory of its resolved target, and asks only for creates, closes after a write,
deletes and moves, so a read of the file never wakes it. It resolves the link again
before each reload, so a link pointed elsewhere moves the watch, and a missing
directory is watched through the nearest one above it that exists. A burst of events
becomes one reload after 200 ms of quiet on the injected clock. Saves by rename (vim,
nvim), writes in place, a removed file (no file: the defaults), a file created again
and a retargeted link all reload. When the kernel's queue overflows or a watched
directory is removed or moved, the watches are armed again and the file reloads once.

A reload checks the whole file with `efr-config`. A file with an error changes nothing:
the old settings stay, the error (with its line, column and key) is kept for
`admin.status`, and each terminal with a conversation active within
`conversation.tty_idle_hours` gets the notice "efr: config.toml has an error; the old
settings stay: ...", once per new error. A valid file is laid over the running settings
with `Settings::reloaded`: a restart key (`screen`, `model.provider`,
`openai.originator`, `openai.subscription_base_url`, `openai.api_base_url`) keeps its
running value and is listed in `restart_needed` (and in the notice "efr: restart efrd
to apply: ..."), and a value from `EFR_LOG`, `EFR_SCREEN` or a flag still wins. Then
the appliers take the new values:

- the settings watch: the next turn's model, effort, system prompt, output limit and
  approval and streaming settings, the queue limit and the terminal idle hours of the
  next prompt, and the idle time of the shell collector;
- the permission engine, built again on every reload (with the project registry and
  the links in the config directory) and sent when `[permissions]` or what it protects
  changed, so a `config.toml` link that points elsewhere is write-sealed from the next
  tool call on; and, when `[permissions]` changed, the hidden shells' trusted
  programs: a running zsh with the old set restarts in its directory before its next
  command (`efr-shell`);
- `shell.program` and `shell.login` for the hidden shells started from then on;
- `log`, through the reload layer of the tracing filter (`telemetry.rs`). An invalid
  filter is an error of the file.

Everything that can fail runs before the first send, so a refused file changes nothing.

### The settings tool

`tools/settings_tool.rs` is the model's way to efr's own settings. It lives here and not
in `efr-tools`, because it needs `efr-config` and the daemon's settings, and
`efr-config` reaches `efr-permissions`, which `efr-tools` must never reach. The toolbox
offers it after the registry's tools, as `settings`.

- `read` lists the file (a file, missing or a link), the last reload's error,
  `restart_needed`, every key with its value, source and when a change applies, the
  rules with their numbers and the models with their efforts. It declares nothing, so
  it runs without a question for a local turn.
- `set`, `unset`, `add_rule` and `remove_rule` are planned with `efr-config`'s writer
  against the file as it is (the example when there is none), and the whole new file
  is checked like a load; the default model must be in the model list and the default
  effort one that model takes. A rule that names secrets (`Engine::names_secrets`) is
  refused, to add and to remove. A change that fails any of this goes back to the
  model, and nobody is asked.
- A valid change declares a `SettingsChange` (a summary and whether it loosens
  permissions: a new allow rule, a removed deny or ask rule, a mode toward `auto`,
  `per_call` to `keep`, a removed secret path). The engine asks about it in every mode
  and whatever the rules say, and denies it for a remote turn. The approval shows the
  unified diff of the file (`efr_tools::unified_diff`, as for `write_file`; for a new
  file, the diff from the example), headed by "This change loosens permissions." when
  it does.
- The answer runs the plan again and writes only when the file and the new text are
  what the user saw; otherwise nothing is written and the model is told to call the
  tool again, which plans against the new file and asks again. The writer compares the
  file's hash once more right before it writes, and keeps a link and the comments.
  Then the tool asks the reload task to reload, so the change applies from the next
  turn (rules from the next tool call), and its result says when it applies and
  whether a restart is needed.
- The tool never declares the file as a path: config protection still denies every
  tool's write to it, `write_file` and the shell included.

### Methods

`methods.rs` implements `efr_transport::Dispatcher`. It matches `Method` exhaustively
twice, once for the scope and once for the handler, so a new protocol method does not
compile until it has both. One handler per method lives in `methods/<noun_verb>.rs`.
Connections on the Unix socket hold every scope, `admin` included; a phone connection
(`Origin::Phone`, the tailnet listener of a later milestone) holds `read`, `operate` and
`approve`.

- Writes (`prompt.send`, `turn.interrupt`, `turn.steer`, `approval.respond`) answer a
  retried command id from its receipt. A refusal a retry cannot change (`invalid`,
  `not_found`, `conflict`) is kept as a rejected receipt; a busy or failed one is not.
- `prompt.send` routes to the named conversation, to a new one with `new_conversation`
  (`,new`), or to the active conversation of the prompt's terminal (the context's tty,
  else the hello's), starting one when the terminal has none. The terminal's
  conversation ends after `conversation.tty_idle_hours` (12 by default) without
  activity, and when a prompt comes from another shell while the shell that took the
  terminal has exited, so a new tab that reuses a closed tab's `/dev/pts` number
  starts fresh.
  A prompt's `settings` (mode, model, effort) are checked by the conversation
  against the latest settings (`settings.rs` gives it the defaults and the model list
  of `providers.rs`): a model outside the list, or an effort the model does not take,
  is `invalid` with the setting, the value and the choices as data, and nothing is
  recorded. The result carries the effective settings; the turn resolves them again
  when it starts, records them on `turn_started` and sends the effort in the request's
  `provider_options`. A turn from a remote origin runs with at most `cautious`.
- `conversation.subscribe` subscribes to the store's commits, reads the high-water
  mark, replays a gap of at most 128 events and 1 MiB or sends a bounded snapshot with a
  history cursor, then forwards live events through a 64-item queue.
- `pty.attach` registers for live output, then sends the output after `since_seq` from
  the recording (a gap of at most 1 MiB) or a screen snapshot plus what was recorded
  after it; live output follows with any overlap cut by offset. `pty.resize` refuses a
  size without rows or columns and clamps a huge one.
- `admin.login_openai` streams the authorize URL, waits for the browser, records
  `login_completed` and makes the running provider forget its cached token.
- `models.list` answers the effective model list of the latest settings
  (`providers.rs`, `effective_models`): the provider's built-in models with their
  efforts and default effort, then the ids of `[openai] models` that the list does not
  hold, with the default model marked.
- `admin.config_reload` reloads at once and answers with the outcome: applied, or the
  file's error with the old settings kept, and the keys that wait for a restart.
- `admin.status` reports the four roots with where each came from (its own variable,
  `EFR_HOME`, XDG, or `/run/user/<uid>`), and the config file: its path, whether it
  exists, a symbolic link's target, the last reload's error and `restart_needed`.
- `input.respond` types the line a user gave for a running tool call that waits for
  input into the conversation's hidden shell, through `ShellSessions::answer`: only
  while that call's command runs, a wait of it was reported, the job that waited (by
  its process group) still holds the terminal and the terminal reads a line, and for a
  hidden answer only while echo is off, all checked by the shell's actor right before
  its one write;
  the shell appends `\r`. A text that is not one line of at most
  `efr_protocol::InputRespond::MAX_TEXT_BYTES` bytes without control characters is
  `invalid`; a conversation without a shell, or in which no call's command runs (the
  call's command ended, or was left at its timeout and goes on without a call), is
  `not_found`; and a command that does not wait for that input (another call's command,
  one not started yet, one with no wait reported, a terminal that does not read a line,
  echo on for a hidden answer, the shell itself or another job than the one that waited
  holding the terminal, as when the command's job ended and zsh's precmd hooks, or a
  command one of them started, run before the command's `D` arrived) is `conflict`;
  nothing is written then. There is no receipt. The text is a `SecretText`: it reaches
  no log, error message, event or receipt, and the handler logs only its length. The transport zeroes the frame's bytes once it has read and decoded
  them, the `SecretText` and the clone that the shell's actor gets are zeroed when they
  drop, and the shell writes the answer to the PTY from that clone's buffer;
  serde_json's scratch buffer for a text that holds an escape (a quote, a backslash,
  `\u`) is not zeroed.
- `conversation.subscribe` with `answers_input` from a connection that holds the
  `terminal` scope counts, while its stream lasts, as a client at which a person can
  answer a waiting command (`Connections::answerers`); the count drops when the stream
  ends for any reason, or the connection closes. The toolbox (`tools.rs`) answers the
  shell's question who can answer hidden input from it, when the command starts to
  wait and at every look after: with no such client, a command that waits for hidden
  input, such as a password, is interrupted at once (`SIGINT` to the foreground process
  group, sent only while the job that waited holds the terminal) and the model reads
  that nobody could answer it, with the advice to have the user run it in their own
  terminal or follow the turn while it retries. With one, the
  command waits until it ends or its timeout passes. A visible wait (a `[Y/n]`
  question) never stops a command.
- `shell.sudo_cache` decides what happens to sudo's credential cache on the hidden
  shell's terminal, read from the latest settings at each call (`tools.rs` sets
  `ToolContext::forget_credentials`). `keep`, the default, leaves it to sudo (about
  five minutes), so a second `sudo` soon after an answered one may not ask again.
  `per_call` makes the hidden shell forget the credentials (`sudo -k`, `doas -L`) after
  each call, before anything else runs there, so the next `sudo` asks for the password
  again. It needs the zsh integration: a hidden shell without it keeps the cache, and
  a call inside a nested shell forgets only when that shell exits. Every command with `sudo` still needs the user's approval with either value.

### Notices

When a turn finishes or fails, or an approval waits, and no client in the
conversation's terminal follows it (an open subscription or a live lease from a
connection whose hello named that tty), the daemon appends one line to
`$XDG_RUNTIME_DIR/efr/notices/<tty>` (`docs/storage.md`), which the zsh plugin prints at
its next prompt. A subscription that ends leaves the highest sequence number it was
handed for its terminal, and an event at or below it gets no notice: `efr` exits as
soon as it has shown the end of a turn, often before the notices decide on that event,
and its terminal must not hear about a turn it just showed.

### Features

- `local-pty` (default): hidden shells on PTYs opened in this process through
  `efr_pty::LocalPtyHolder`. Without it, and unless a holder is injected, every shell
  start fails with a clear error. The PTY holder milestone deletes the feature.
- `screen-ghostty`: the libghostty-vt screen backend (needs Zig 0.16.0). With it, the
  screens are ghostty unless `EFR_SCREEN=vt100`; without it, vt100.

`cfg(feature = ...)` appears only in `screens.rs` and `shells.rs` (a tidy rule).

## Tier

Tier 4: a binary, the composition root.

## Allowed dependencies

Every library crate except `efr-client` and the test crates: `efr-stdx`,
`efr-protocol`, `efr-store`, `efr-credentials`, `efr-permissions`, `efr-scope`,
`efr-holder`, `efr-http`, `efr-screen`, `efr-provider`, `efr-screen-vt100`,
`efr-screen-ghostty` (optional), `efr-pty` (optional), `efr-shell`, `efr-tools`,
`efr-provider-openai`, `efr-oauth-openai`, `efr-config`, `efr-conversation` and
`efr-transport`. `xtask/src/deps.rs` holds the allowlist; `efr-test-daemon` is its only dev-dependent,
and only from `tests/`.

Third-party crates: `tokio`, `tokio-util` (`CancellationToken`), `async-trait`, `bytes`,
`serde`, `serde_json`, `jiff`, `nix` (`flock`), `base64` (the hello
challenge), `clap` (the flags), `sd-notify` 0.5.0 (`READY=1`, `STOPPING=1`), `rustix`
(inotify, for the config file watcher), `toml_edit` (the values that the settings tool
sets and the one-line text of a rule), `tracing`, `tracing-subscriber` (with its reload
layer for the log filter), `tracing-journald`, `thiserror`, and `anyhow` in `main.rs`
only.

`HOME`, `JOURNAL_STREAM` and the shells' environment are read with `std::env` here, the
one crate besides `efr-stdx` that may read the environment: they are POSIX and systemd
conventions that `efr_stdx::env::Var` does not name.

## Invariant

- One daemon per data directory: the `flock` decides, never `daemon.json`.
- The socket opens only after the migrations and the reconciliation, and nothing that
  was in flight continues on its own after a restart.
- `methods.rs` is the only place that knows every method end to end, and
  `From<DaemonError> for ErrorFrame` in `error.rs` is the only mapping of daemon errors
  to wire codes.
- A retried write never runs twice: receipts answer it.
- The last command of a prompt reaches the turn in memory only; it never enters an
  event, a receipt or a log field.
- Shutdown releases the lock last, after the database is closed.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-daemon
```

Unit tests cover the config layer (precedence of the variables and flags, refused
values, an insta snapshot of the effective dump; the file's own checks are tested in
`efr-config`), the error mapping, the scope table, prompt routing, receipts,
reconciliation against the real store in memory, the idle collector's rule, live
reload (applied files, refused files with their place, restart keys, the engine sent on
a rules change and on a retargeted link, the notices, the status, SIGHUP, and the watcher on real inotify events
with saves by rename and in place, a removed and recreated file, a symlinked file, a
retargeted link, a removed and recreated target directory and a forced queue
overflow), the PTY
fan-out with overflow, the connection table, notices, the lock, `daemon.json`, the
providers, the tool adapter and its answer to who can answer hidden input. The tests
in `run/tests.rs` start the real daemon
in-process on temporary directories with a manual clock, a seeded generator, vt100
screens, an in-memory database and a scripted model, and talk to it over its socket in
raw frames: a prompt followed to the end of its turn, routing and receipts, refusals
(an answer with nothing to answer among them, its text never repeated), a notice for a terminal that does not follow its conversation and none for one that
followed its turn to the end, and a second daemon refused by the lock. The tool adapter's tests run shell calls through the real
toolbox and the engine with the defaults and with user rules: read-only commands run,
other commands ask, a named secret is denied, and relative paths resolve where the
hidden shell is. The `e2e_` tests run an approved command in a real hidden zsh and
attach to its PTY, run `ls && cat` there without approval, and let a configured rule
allow `seq 3` but not `seq 4`; they skip with a message unless `EFR_TEST_ZSH=1`:

```sh
EFR_TEST_ZSH=1 cargo nextest run -p efr-daemon e2e_
```

The integration tests in `tests/` (`hello`, `subscribe`, `prompt_send`, `shell_tool`,
`approvals`, `interrupt`, `receipts`, `reconcile`, `pty_attach`, `login`,
`input_respond`) run the daemon through `efr-test-daemon`'s `TestDaemon` and replay
its fourteen NDJSON scenarios, each with the assertions of its case: the fake PTY
holder plays the hidden shell, the replay provider or a local Responses server plays
the model. The `shell_` tests run a real zsh and skip with a message unless
`EFR_TEST_ZSH=1`: a command runs and is recorded, a password typed through
`input.respond` reaches only the program (not the model's next request, the event log,
any file of the daemon's tree or any log line at any level), and a password prompt
that no client can answer is stopped within seconds.

No test uses the network, a real model, real time, the user's home, config or runtime
directory, or the git configuration of the machine.
