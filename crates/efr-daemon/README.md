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
2. `config.rs`: one file, `$XDG_CONFIG_HOME/efr/config.toml`, unknown keys refused;
   defaults, then the file, then `EFR_LOG` and `EFR_SCREEN`, then the flags.
   `efrd --print-config` prints every value with its source.
3. The store: the backup copy in `backups/`, the forward-only migrations.
4. `reconcile.rs`: running turns cancelled, pending approvals expired, queued prompts
   held, running shells recorded as exited, process-bound outbox items cancelled.
5. The PTY table, the recording sink, the shells, the providers (`providers.rs`), the
   tool registry (`tools.rs`), the permission engine and the conversation registry.
6. The background tasks: the shells' lifecycle events, the notices (`notices.rs`) and
   the idle shell collector (`gc.rs`).
7. The Unix socket (0600) and `daemon.json` (`discovery.rs`), then `READY=1` through
   `sd-notify`.

SIGTERM or SIGINT (`signals.rs`) cancels the serve: the connections end, the background
tasks stop, the conversation actors and the shells are shut down, the recordings are
closed, `daemon.json` is removed, the database is closed, and the lock is released last.

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
  else the hello's), starting one when the terminal has none.
- `conversation.subscribe` subscribes to the store's commits, reads the high-water
  mark, replays a gap of at most 128 events and 1 MiB or sends a bounded snapshot with a
  history cursor, then forwards live events through a 64-item queue.
- `pty.attach` registers for live output, then sends the output after `since_seq` from
  the recording (a gap of at most 1 MiB) or a screen snapshot plus what was recorded
  after it; live output follows with any overlap cut by offset. `pty.resize` refuses a
  size without rows or columns and clamps a huge one.
- `admin.login_openai` streams the authorize URL, waits for the browser, records
  `login_completed` and makes the running provider forget its cached token.

### Notices

When a turn finishes or fails, or an approval waits, and no client in the
conversation's terminal follows it (an open subscription or a live lease from a
connection whose hello named that tty), the daemon appends one line to
`$XDG_RUNTIME_DIR/efr/notices/<tty>` (`docs/storage.md`), which the zsh plugin prints at
its next prompt.

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
`efr-provider-openai`, `efr-oauth-openai`, `efr-conversation` and `efr-transport`.
`xtask/src/deps.rs` holds the allowlist; `efr-test-daemon` is its only dev-dependent,
and only from `tests/`.

Third-party crates: `tokio`, `tokio-util` (`CancellationToken`), `async-trait`, `bytes`,
`serde`, `serde_json`, `toml` (the config), `jiff`, `nix` (`flock`), `base64` (the hello
challenge), `clap` (the flags), `sd-notify` 0.5.0 (`READY=1`, `STOPPING=1`), `tracing`,
`tracing-subscriber`, `tracing-journald`, `thiserror`, and `anyhow` in `main.rs` only.

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

Unit tests cover the config (precedence, refused keys and values, an insta snapshot of
the effective dump), the error mapping, the scope table, prompt routing, receipts,
reconciliation against the real store in memory, the idle collector's rule, the PTY
fan-out with overflow, the connection table, notices, the lock, `daemon.json`, the
providers and the tool adapter. The tests in `run/tests.rs` start the real daemon
in-process on temporary directories with a manual clock, a seeded generator, vt100
screens, an in-memory database and a scripted model, and talk to it over its socket in
raw frames: a prompt followed to the end of its turn, routing and receipts, refusals,
a notice for a terminal that does not follow its conversation, and a second daemon
refused by the lock. The `e2e_` test runs an approved command in a real hidden zsh and
attaches to its PTY; it skips with a message unless `EFR_TEST_ZSH=1`:

```sh
EFR_TEST_ZSH=1 cargo nextest run -p efr-daemon e2e_
```

No test uses the network, a real model, real time, the user's home, config or runtime
directory, or the git configuration of the machine. The integration tests over
`TestDaemon` (`tests/`) come with `efr-test-daemon`.
