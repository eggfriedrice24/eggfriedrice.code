# Architecture

This document is the map. It says which crate owns what, which way dependencies
point, how threads are laid out and what happens at startup and restart. The full
design record is `research/rust-structure.md`; the rules a contributor follows inside a
crate are in `CONVENTIONS.md`.

## Shape

eggfriedrice.code is a Cargo workspace of one crate per bounded context plus `xtask`.
Three binaries ship:

- `efrd`, the daemon, runs as a systemd user service (`systemd/efrd.service`). It owns
  the conversations, the model loop, the hidden shells, the screens and the database.
- `efr`, the CLI, is a thin relay to the daemon. The zsh plugin
  (`shell/zsh/efr.plugin.zsh`) calls it for every `,` line.
- `efr-sbx`, the sandbox launcher of the `auto` mode, is installed next to efrd in
  `~/.local/lib/efr` (`/usr/lib/efr` from the AUR packages) and is never on the `PATH`. The hidden shell runs a copy of it for
  each contained call.

All clients speak one protocol, defined in `efr-protocol`, over a Unix socket at
`$XDG_RUNTIME_DIR/efr/daemon.sock`. The phone client and the WebSocket listener come in
a later milestone and use the same frames.

## Crate map

Tiers are a reading aid; the enforced rule is the per-crate allowlist in
`xtask/src/deps.rs`. Kind `dev` means the crate is never a normal dependency of a
shipped binary.

| Crate | Kind | Tier | Owns | Allowed workspace dependencies |
|---|---|---|---|---|
| `efr-stdx` | lib | 0 | XDG paths, `Clock` and `Rng` traits, `process::command`, atomic and 0600 writes, files changed through a link, named threads, UUIDv7 | none |
| `efr-protocol` | lib | 0 | everything on the wire: frames, `Method`, params and results, `Event`, ids, `Scope`, `ShellContext`, framing, `PROTOCOL_VERSION`; no tokio, no IO | `efr-stdx` |
| `efr-patch` | lib | 0 | the patch engine of `apply_patch` and the `edit` tool: the patch text to file operations, its Lark grammar, hunks matched with tolerant passes, every new content computed before anything is written, one exact replacement; no IO, no tokio | none |
| `efr-store` | lib | 1 | the only SQLite owner: migrations, the single writer, readers, events, projections, receipts, outbox, recording index, turn messages | `efr-protocol`, `efr-stdx` |
| `efr-credentials` | lib | 1 | `SecretStore` and the 0600 file store; optional keyring | `efr-stdx` |
| `efr-permissions` | lib | 1 | pure policy: path classes, the built-in policy of each permission mode (`manual`, `cautious`, `auto`), config protection, the exits of the `auto` sandbox and the Allow / Contain / Ask / Deny decision | `efr-protocol` |
| `efr-scope` | lib | 1 | cwd to `Scope`: git discovery, dotfiles layouts, the project registry and its changes that keep comments | `efr-protocol`, `efr-stdx` |
| `efr-holder` | lib | 1 | the `PtyHolder` trait and holder wire types; no IO, no unsafe | `efr-protocol`, `efr-stdx` |
| `efr-http` | lib | 1 | the reqwest client, SSE parser, WebSocket client, Unix-socket HTTP client, header redaction | `efr-stdx` |
| `efr-screen` | lib | 1 | the `Screen` trait, `ScreenActor` and `ScreenHandle`, the OSC 133 and OSC 7 scanner, the conformance suite | `efr-protocol`, `efr-stdx` |
| `efr-provider` | lib | 1 | the `Provider` and `TokenSource` traits, canonical messages, function and freeform tool definitions | `efr-protocol`, `efr-stdx` |
| `efr-test-support` | dev | 1 | `TestClock`, seeded `TestRng`, temp dirs, in-memory store, NDJSON reader, `ReplayProvider`, `Wait` | `efr-protocol`, `efr-store`, `efr-provider`, `efr-stdx` |
| `efr-render` | lib | 1 | markdown and render events to ANSI: committed and live zones, colour roles and the palette, syntax colours, OSC 8 links, widths by code point or grapheme cluster; no IO, the CLI passes `RenderOptions` | none |
| `efr-sandbox` | lib | 1 | the pure logic of the `auto` sandbox: `SandboxSpec`, `MountPlan` and the bwrap arguments, Landlock and seccomp as data, the environment and export filters, the records, the sandbox state, `result.json`, the surface guard, the worktree record, the probe's result types; file access only through `FsView`, no tokio, no unsafe | `efr-protocol` |
| `efr-screen-vt100` | lib | 2 | `Screen` over vt100; the Zig-free default | `efr-screen` |
| `efr-screen-ghostty` | lib | 2 | `Screen` over libghostty-vt; the only crate that needs Zig | `efr-screen` |
| `efr-pty` | lib | 2 | `LocalPtyHolder`: openpty, `setsid` and `TIOCSCTTY` in `pre_exec`; the only unsafe code at milestone 1 | `efr-holder`, `efr-stdx` |
| `efr-shell` | lib | 2 | one hidden zsh per conversation, shell state from marks, `run_command` | `efr-holder`, `efr-screen`, `efr-protocol`, `efr-sandbox`, `efr-stdx` |
| `efr-sbx` | bin `efr-sbx` | 2 | the launcher of the `auto` sandbox: `run` (one call in bwrap with Landlock and seccomp, or the exit child as a subreaper), `inner`, `probe`; checks what comes back and writes `result.json` last; no async runtime; its one `unsafe` module is `fds.rs` (ADR 0007) | `efr-sandbox`, `efr-protocol` |
| `efr-tools` | lib | 2 | the `Tool` trait, the registry, the shell, read_file, write_file, apply_patch and edit tools, the freeform tool spec; knows nothing about permissions | `efr-shell`, `efr-scope`, `efr-patch`, `efr-protocol`, `efr-stdx` |
| `efr-provider-openai` | lib | 2 | the Responses API client over HTTP or a WebSocket for each conversation; the model catalog (the fetch from the backend, its cache file and the built-in table), which also says which models take freeform (`custom`) tools and prefer WebSockets; takes tokens only through `TokenSource` | `efr-provider`, `efr-http`, `efr-protocol`, `efr-stdx` |
| `efr-provider-anthropic` | lib | 2 | the Messages API client with an API key (provider id `anthropic-api`): the request body with its prompt cache markers (a pure function), the stream mapping, the usage sums, the error and retry classifier, the model catalog from `GET /models` with its cache file and no built-in table, the key check; takes the key only through `TokenSource` | `efr-provider`, `efr-http`, `efr-protocol`, `efr-stdx` |
| `efr-oauth-openai` | lib | 2 | the subscription login: PKCE, loopback callback, refresh, `OpenAiTokenSource` | `efr-http`, `efr-credentials`, `efr-provider`, `efr-stdx` |
| `efr-snapshot` | lib | 2 | efr's own snapshot store: one bare git repository per project or `$SCRATCH` in the data root, hardened git through `efr_scope::Git::command`, the trees before and after each call that can write, the turn's `pre` and `post` refs, the changes of a call or a turn, the diff of a turn, the collector (phase 4 of the auto spec, without undo) | `efr-scope`, `efr-protocol`, `efr-stdx` |
| `efr-config` | lib | 2 | `config.toml` for `efrd` and `efr`: the schema of every key, defaults, validation, the effective view with sources, the JSON schema, the example file and the format-preserving writer; no async, no network | `efr-permissions`, `efr-protocol`, `efr-stdx` |
| `efr-conversation` | lib | 3 | one actor per conversation: queue, turn loop, the single permission check point, approvals, interrupt, steer, withdraw; in `auto` the launch of each shell call, exit questions and their records, the quarantine question and the fallback to `cautious`; drives tools through its own `Toolbox` trait, implemented by `efr-daemon` | `efr-provider`, `efr-permissions`, `efr-scope`, `efr-store`, `efr-sandbox`, `efr-protocol`, `efr-stdx` |
| `efr-transport` | lib | 3 | the protocol edge: codec, Unix listener, connection table, subscriptions, the `Dispatcher` trait | `efr-protocol`, `efr-stdx` |
| `efr-client` | lib | 3 | the client side of the protocol for `efr`, tests and the proxy | `efr-protocol`, `efr-stdx` |
| `efr-daemon` | bin `efrd` | 4 | the composition root; one file per protocol method; the settings tool, which needs `efr-config` and so cannot live in `efr-tools`; the `auto` sandbox service: the launcher's copy, the probe, the spec of each call, the plan lock, the facts of a line, the turn-end report and the read-only scope of model-side socket peers | every library crate above except `efr-client` and the test crates |
| `efr-cli` | bin `efr` | 4 | `efr send`, `new`, `status`, `history`, `diff`, `compact`, `settings`, `models`, `login openai`, `config` (show, check, edit, set, unset, schema, reload), `project` (list, add, remove, through the daemon), `paths`, `sandbox` (check, explain); renders replies through `efr-render` | `efr-client`, `efr-config`, `efr-render`, `efr-protocol`, `efr-stdx` |
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
   `efr-provider-anthropic -> efr-oauth-openai`,
   `efr-conversation -> efr-shell`, `efr-conversation -> efr-transport`,
   `efr-transport -> efr-store`, `efr-protocol -> tokio`,
   `efr-test-support -> efr-daemon`, `efr-sandbox -> tokio`, `efr-sbx -> tokio`,
   `efr-patch -> tokio`;
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
- The model providers never see a refresh token.
- The conversation reaches shells only through `ShellTool`.
- The engine does not know about transports, and the transport does not touch the
  database.
- The protocol crate stays free of a runtime, so any client can compile it.
- The sandbox logic and the launcher stay free of a runtime: the launcher is a small
  process that runs as a foreground job of the hidden shell, and every rule it applies
  is a pure function that tests run without a kernel.
- The patch engine stays pure: it computes every new content from texts that the
  caller read, so a patch over several files is checked whole before the tool writes
  one of them.
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
  id>`), at most once per `ShellConfig::tail_interval` (200 ms, fixed; the default of
  `conversation.update_interval_ms` is the same, but the key does not change it).
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
  `Overflow { last_seq }`; it never slows the producer. Drafts of a running turn are the
  one lossy item: they have a small room of their own in that queue, and a full room
  drops the draft without closing the consumer.

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
5. Start the store writer, providers, the tool registry and the shell sessions; copy
   the sandbox launcher to `$XDG_RUNTIME_DIR/efr/bin/efr-sbx` and run the sandbox
   probe, whose status decides whether `auto` turns run as `auto` or as `cautious`.
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
  modes, the built-in read-only commands, the `auto` sandbox and its exits, config
  protection and how a command line is read: `docs/permissions.md`.
- The settings tool, the model's only way to change `config.toml`, after an approval
  with the diff: `crates/efr-daemon/src/tools/settings_tool.rs`.
- What a call or a turn changed in files: the snapshot store in `crates/efr-snapshot/`,
  which roots a call snapshots in `crates/efr-daemon/src/tools/snapshot.rs`, and the
  diff of a turn in `crates/efr-daemon/src/methods/conversation_diff.rs`.
- The OSC 133 and OSC 7 scanner: `crates/efr-screen/src/shell_marks/`.
- Freeform tools, whose input is text: the definition in
  `crates/efr-provider/src/request.rs`, which models take the freeform form in
  `crates/efr-provider-openai/src/catalog.rs` (`ModelInfo::freeform_tools` from the
  catalog's `apply_patch_tool_type`), the `custom` items in `crates/efr-provider-openai/src/convert.rs`. The patch engine of
  `apply_patch`: `crates/efr-patch/`; the tool: `crates/efr-tools/src/apply_patch.rs`
  and its contract in `crates/efr-tools/README.md`; the question for each delete and
  move: `Requirements::destructive` in `crates/efr-permissions/src/engine.rs`.
- The `edit` tool of Claude models: `crates/efr-tools/src/edit.rs` on
  `efr_patch::replace`. Which model gets `apply_patch` and which `edit`:
  `ModelInfo::edit_tool` in `crates/efr-provider/src/provider.rs`, read by
  `turn::edit_tool` in `crates/efr-conversation/src/turn.rs`; the list that offers one
  of the two and the refusal of the other in `crates/efr-daemon/src/tools.rs`.
- The model's context window and its compaction: the contract in
  `crates/efr-conversation/README.md`, section "Context"; the numbers and the estimate in
  `crates/efr-conversation/src/context.rs`; pruning, the cut, the summary request and
  its prompt (`compaction/prompt.md`) in `crates/efr-conversation/src/compaction.rs`;
  the fresh context block in `crates/efr-conversation/src/fresh.rs`; the guards and the
  compaction inside a turn in `crates/efr-conversation/src/turn/compact.rs`; the manual
  one in `crates/efr-conversation/src/actor/compact.rs`; the newest compactions in
  `crates/efr-store/src/compactions.rs`; the wire types in
  `crates/efr-protocol/src/compaction.rs`; `[compaction]` in
  `crates/efr-config/src/tables/compaction.rs`.
- The history that only grows, so each request starts with the request before it: the
  contract in `crates/efr-conversation/README.md`, section "The history only grows";
  the list of turns and the byte limit in `crates/efr-conversation/src/history.rs`;
  the redaction of the last command in the kept preamble in
  `crates/efr-conversation/src/preamble/secrets.rs`; the measurement on the OpenAI path
  in `crates/efr-daemon/tests/it/cache_prefix.rs`.
- The model catalog: the fetch, the cache file and which models are on offer in
  `crates/efr-provider-openai/src/catalog.rs`; the built-in table, the last fallback, in
  `crates/efr-provider-openai/src/models.rs`; when efrd fetches, the effective model
  list, the default model and the windows of `[openai] models` in
  `crates/efr-daemon/src/catalog.rs`.
- The Responses WebSocket transport: the contract in
  `crates/efr-provider-openai/README.md`, section "WebSocket transport"; the
  connection of each conversation and the fallback to HTTP in
  `crates/efr-provider-openai/src/websocket.rs`; incremental input in
  `crates/efr-provider-openai/src/websocket/continuation.rs`; the WebSocket client in
  `crates/efr-http/src/websocket.rs`; `[openai] websocket` in
  `crates/efr-config/src/tables.rs`.
- The Anthropic provider: the contract (the request, the prompt cache markers, the
  catalog, the token counts and the failures) in `crates/efr-provider-anthropic/README.md`;
  the placement of the cache markers in
  `crates/efr-provider-anthropic/src/convert/breakpoints.rs`; which error answers are
  sent again in `crates/efr-provider-anthropic/src/failure.rs`; the fake Messages API
  and model list for daemon tests, `MessagesServer`, in
  `crates/efr-test-daemon/src/test_daemon/messages.rs`; the canonical token counts
  of every provider in `crates/efr-provider/README.md`, section "Token counts".
- On-disk layout and schema: `docs/storage.md`. The libghostty pin: `docs/ghostty-pin.md`.
- Decisions that are expensive to reverse: `docs/adr/`.
