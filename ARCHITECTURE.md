# Architecture

This document is the map. It says which crate owns what, which way dependencies
point, how threads are laid out and what happens at startup and restart. The full
design record is `research/rust-structure.md`; the rules a contributor follows inside a
crate are in `CONVENTIONS.md`.

## Shape

eggfriedrice.code is a Cargo workspace of one crate per bounded context plus `xtask`.
Two binaries ship at milestone 1:

- `efrd`, the daemon, runs as a systemd user service (`systemd/efrd.service`). It owns
  the conversations, the model loop, the hidden shells, the screens and the database.
- `efr`, the CLI, is a thin relay to the daemon. The zsh plugin
  (`shell/zsh/efr.plugin.zsh`) calls it for every `,` line.

All clients speak one protocol, defined in `efr-protocol`, over a Unix socket at
`$XDG_RUNTIME_DIR/efr/daemon.sock`. The phone client and the WebSocket listener come in
a later milestone and use the same frames.

## Crate map

Tiers are a reading aid; the enforced rule is the per-crate allowlist in
`xtask/src/deps.rs`. Kind `dev` means the crate is never a normal dependency of a
shipped binary.

| Crate | Kind | Tier | Owns | Allowed workspace dependencies |
|---|---|---|---|---|
| `efr-stdx` | lib | 0 | XDG paths, `Clock` and `Rng` traits, `process::command`, atomic and 0600 writes, named threads, UUIDv7 | none |
| `efr-protocol` | lib | 0 | everything on the wire: frames, `Method`, params and results, `Event`, ids, `Scope`, `ShellContext`, framing, `PROTOCOL_VERSION`; no tokio, no IO | `efr-stdx` |
| `efr-store` | lib | 1 | the only SQLite owner: migrations, the single writer, readers, events, projections, receipts, outbox, recording index | `efr-protocol`, `efr-stdx` |
| `efr-credentials` | lib | 1 | `SecretStore` and the 0600 file store; optional keyring | `efr-stdx` |
| `efr-permissions` | lib | 1 | pure policy: path classes, the built-in policy of each permission mode (`manual`, `cautious`, `auto`), config protection and the Allow / Ask / Deny decision | `efr-protocol` |
| `efr-scope` | lib | 1 | cwd to `Scope`: git discovery, dotfiles layouts, the project registry and its changes that keep comments | `efr-protocol`, `efr-stdx` |
| `efr-holder` | lib | 1 | the `PtyHolder` trait and holder wire types; no IO, no unsafe | `efr-protocol`, `efr-stdx` |
| `efr-http` | lib | 1 | the reqwest client, SSE parser, Unix-socket HTTP client, header redaction | `efr-stdx` |
| `efr-screen` | lib | 1 | the `Screen` trait, `ScreenActor` and `ScreenHandle`, the OSC 133 and OSC 7 scanner, the conformance suite | `efr-protocol`, `efr-stdx` |
| `efr-provider` | lib | 1 | the `Provider` and `TokenSource` traits, canonical messages | `efr-protocol`, `efr-stdx` |
| `efr-test-support` | dev | 1 | `TestClock`, seeded `TestRng`, temp dirs, in-memory store, NDJSON reader, `ReplayProvider`, `Wait` | `efr-protocol`, `efr-store`, `efr-provider`, `efr-stdx` |
| `efr-render` | lib | 1 | markdown and render events to ANSI: committed and live zones, syntax colours, OSC 8 links; no IO, the CLI passes `RenderOptions` | none |
| `efr-screen-vt100` | lib | 2 | `Screen` over vt100; the Zig-free default | `efr-screen` |
| `efr-screen-ghostty` | lib | 2 | `Screen` over libghostty-vt; the only crate that needs Zig | `efr-screen` |
| `efr-pty` | lib | 2 | `LocalPtyHolder`: openpty, `setsid` and `TIOCSCTTY` in `pre_exec`; the only unsafe code at milestone 1 | `efr-holder`, `efr-stdx` |
| `efr-shell` | lib | 2 | one hidden zsh per conversation, shell state from marks, `run_command` | `efr-holder`, `efr-screen`, `efr-protocol`, `efr-stdx` |
| `efr-tools` | lib | 2 | the `Tool` trait, the registry, the shell, read_file and write_file tools; knows nothing about permissions | `efr-shell`, `efr-scope`, `efr-protocol`, `efr-stdx` |
| `efr-provider-openai` | lib | 2 | the Responses API client; takes tokens only through `TokenSource` | `efr-provider`, `efr-http`, `efr-protocol`, `efr-stdx` |
| `efr-oauth-openai` | lib | 2 | the subscription login: PKCE, loopback callback, refresh, `OpenAiTokenSource` | `efr-http`, `efr-credentials`, `efr-provider`, `efr-stdx` |
| `efr-config` | lib | 2 | `config.toml` for `efrd` and `efr`: the schema of every key, defaults, validation, the effective view with sources, the JSON schema, the example file and the format-preserving writer; no async, no network | `efr-permissions`, `efr-protocol`, `efr-stdx` |
| `efr-conversation` | lib | 3 | one actor per conversation: queue, turn loop, the single permission check point, approvals, interrupt, steer; drives tools through its own `Toolbox` trait, implemented by `efr-daemon` | `efr-provider`, `efr-permissions`, `efr-scope`, `efr-store`, `efr-protocol`, `efr-stdx` |
| `efr-transport` | lib | 3 | the protocol edge: codec, Unix listener, connection table, subscriptions, the `Dispatcher` trait | `efr-protocol`, `efr-stdx` |
| `efr-client` | lib | 3 | the client side of the protocol for `efr`, tests and the proxy | `efr-protocol`, `efr-stdx` |
| `efr-daemon` | bin `efrd` | 4 | the composition root; one file per protocol method; the settings tool, which needs `efr-config` and so cannot live in `efr-tools` | every library crate above except `efr-client` and the test crates |
| `efr-cli` | bin `efr` | 4 | `efr send`, `new`, `status`, `history`, `settings`, `models`, `login openai`, `config` (show, check, edit, set, unset, schema, reload), `project` (list, add, remove, through the daemon), `paths`; renders replies through `efr-render` | `efr-client`, `efr-config`, `efr-render`, `efr-protocol`, `efr-stdx` |
| `efr-test-daemon` | dev | T | `TestDaemon` and scenario replay; used only from `tests/` of `efr-daemon` and `efr-cli` | `efr-daemon`, `efr-test-support`, `efr-client`, `efr-protocol` |

None of these crates exists in the first commit; they land in the order of the
milestone 1 file map (`research/rust-structure.md`, section 10).

## The dependency rule

Dependencies point down. The only edges inside one tier are `efr-tools -> efr-shell`
and `efr-daemon -> (everything)`.

`cargo xtask deps` reads `cargo metadata` (all features) and fails when:

1. a member depends on a workspace crate that is not on its allowlist, or a member
   has no allowlist entry;
2. one of these edges exists, directly or through any chain of normal dependencies:
   `efr-tools -> efr-permissions`, `efr-provider-openai -> efr-oauth-openai`,
   `efr-conversation -> efr-shell`, `efr-conversation -> efr-transport`,
   `efr-transport -> efr-store`, `efr-protocol -> tokio`,
   `efr-test-support -> efr-daemon`;
3. any member other than `efr-screen-ghostty` reaches `libghostty-vt`, or any member
   other than `efr-store` reaches `rusqlite`, except through that owner;
4. `efr-test-daemon` is a dev-dependency of anything except `efr-daemon` and
   `efr-cli`, or anything has `efr-daemon` as a direct dev-dependency.

CI runs the same rules a second time with literal `cargo tree` commands, so the two
implementations check each other. A new edge is one line in `xtask/src/deps.rs` plus a
sentence in the crate's README.

What each forbidden edge protects:

- The permission check happens in exactly one place, `efr-conversation/src/turn.rs`,
  so tools cannot ask for or bypass it.
- The OpenAI provider never sees a refresh token.
- The conversation reaches shells only through `ShellTool`.
- The engine does not know about transports, and the transport does not touch the
  database.
- The protocol crate stays free of a runtime, so any client can compile it.
- Zig is a build requirement of one crate, and SQLite has one owner.

The test for splitting a module into a crate, or folding one back: it gets heavy
dependencies of its own, or a second binary needs it.

## Threading model

- One tokio runtime per binary, started in its `main`: `efrd`'s is multi-thread with
  one worker per core and at most four (`efr-daemon/src/runtime.rs`), because its
  work is waiting and the screens and the store writer have threads of their own;
  `efr`'s is current-thread (`efr-cli/src/run.rs`). Library crates never create a
  runtime or call `block_on`.
- State belongs to actors: one actor per conversation, one `StoreWriter`, one
  `ShellSessions`, one `ScreenActor` per screen. Actors talk over bounded `mpsc`
  channels (default capacity 64) with `oneshot` replies; a handle type is the only
  public API of an actor.
- No lock is held across an `.await` (clippy denies it).
- Every `Screen` runs on its own named std thread. libghostty-vt types are
  `!Send + !Sync` and run callbacks synchronously on the calling thread, so a screen
  cannot live in a tokio task. `ScreenActor::spawn` starts the thread and builds the
  screen inside it from a `Send` factory. Commands go in over a bounded
  `std::sync::mpsc::sync_channel(64)`; replies to terminal queries (DA, DSR, DECRQM,
  OSC 10/11) and shell marks come out as `ScreenEvent`s on a tokio channel to the
  shell's async PTY writer. Only the daemon answers terminal queries.
- vt100 screens run through the same actor, so the conformance suite tests the same
  code for both backends.
- Cost: N+1 screen threads for N open conversations, each with a 512 KiB stack. A
  single shared screen thread was rejected because one slow `vt_write` would stall
  every conversation. A command whose output moves the cursor also gets a capture
  screen (`replay-<conversation short id>`) that replays the output and is shut down
  as soon as it is read, so a finished run keeps no thread. While it runs, its live
  tail is read the same way on a screen of the shell's size (`tail-<conversation short
  id>`), at most once per update interval.
- Non-goals: no `Mutex<Terminal>`, no `spawn_blocking` with a `Terminal`, no `LocalSet`
  inside the main runtime, no `Screen` built outside its actor thread.
- Blocking work (SQLite reads, file hashing) runs in `spawn_blocking`. Time and
  randomness are injected through `Clock` and `Rng`.
- The daemon's settings live in a `watch` of `Arc<Settings>` (`efr-config`), and the
  permission engine in another. A reader takes the latest value when its unit of work
  starts and keeps it: a turn when it starts, a prompt when it arrives, a tool call for
  the engine. A running turn never changes its settings. One reload task
  (`efr-daemon/src/reload.rs`) sends new values: the file watcher (the daemon's own,
  on inotify through rustix), SIGHUP and `admin.config_reload` ask it, so reloads
  never interleave. A file with an error changes nothing, and the keys that need a
  restart keep their running values.
- Every fan-out has a bounded queue per consumer. Overflow closes that consumer with
  `Overflow { last_seq }`; it never slows the producer.

## Startup and restart order

At milestone 1 there is one unit, `efrd.service` (`Type=notify`, `Restart=always`,
`RestartSec=5`, `OOMPolicy=continue`, and `ExecReload` sending SIGHUP, which reloads
the config). The hidden shells are children of the daemon, so they end when the daemon
stops. `efrd` starts in this order (the roots come from `EFR_<ROOT>_DIR`, else
`$EFR_HOME/<root>`, else XDG; the socket path is checked first):

1. Take the exclusive `flock` on `$XDG_DATA_HOME/efr/daemon.lock`; exit if another
   daemon holds it. The lock, not `daemon.json`, decides single instance.
2. Load the config with `efr-config` (defaults, then `config.toml`, then `EFR_*`
   variables, then flags).
3. Copy the database to `backups/efr.sqlite.<user_version>`, then run the forward-only
   migrations.
4. Reconcile: mark in-flight turns cancelled, expire pending approvals as not
   resumable, record queued prompts as not run (`turn_cancelled`) with a notice to
   their terminal to send them again, cancel process-bound outbox rows. Nothing
   continues automatically.
5. Start the store writer, providers, the tool registry and the shell sessions.
6. Open the Unix socket (0600) and write `daemon.json` for discovery.
7. Send `READY=1` to systemd.

Clients that connect during a restart get a refused connection and retry; writes carry
a `command_id`, so a retried write returns the stored receipt instead of running twice.

From milestone 5, a second unit, `efr-ptyd.service`, holds the PTY masters and the
shell processes. It starts before `efrd` and survives `efrd` restarts; on start,
`efrd` reattaches the surviving PTYs and replays recording tails into fresh screen
actors. `efr-ptyd` itself restarts through a handoff manifest (validated, restored,
committed, owned). The milestone 5 change removes `efr-daemon`'s `efr-pty` dependency
and adds `efr-daemon -> efr-pty` to the forbidden edges.

## Where to find things

- Wire types and the protocol version: `crates/efr-protocol/src/`.
- One file per protocol method in the daemon: `crates/efr-daemon/src/methods/`, with
  the exhaustive scope match in `methods.rs`.
- The permission check point: `crates/efr-conversation/src/turn.rs`. The permission
  modes, the built-in read-only commands, the `auto` table, config protection and how a
  command line is read: `docs/permissions.md`.
- The settings tool, the model's only way to change `config.toml`, after an approval
  with the diff: `crates/efr-daemon/src/tools/settings_tool.rs`.
- The OSC 133 and OSC 7 scanner: `crates/efr-screen/src/shell_marks/`.
- On-disk layout and schema: `docs/storage.md`. The libghostty pin: `docs/ghostty-pin.md`.
- Decisions that are expensive to reverse: `docs/adr/`.
