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
  each contained call, and efrd starts a copy for each background job
  (`efr-sbx run --job`), with no terminal, in a session of its own.

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
| `efr-store` | lib | 1 | the only SQLite owner: migrations, the single writer, readers, events, projections, receipts, outbox, recording index, turn messages | `efr-protocol`, `efr-stdx` |
| `efr-credentials` | lib | 1 | `SecretStore` and the 0600 file store; optional keyring | `efr-stdx` |
| `efr-permissions` | lib | 1 | pure policy: path classes, the built-in policy of each permission mode (`manual`, `cautious`, `auto`, `full`), config protection, the exits of the `auto` sandbox, the settle pass of full mode and its delete guard, the views of the host that a line gets with no exit, the rules of the bus filter (the built-in read methods, the checks of `[[dbus.allow]]` and the proxy's arguments), the always-allow rules with their floor checks and their match, and the Allow / Contain / Ask / Deny decision | `efr-protocol` |
| `efr-scope` | lib | 1 | cwd to `Scope`: git discovery, dotfiles layouts, the project registry and its changes that keep comments | `efr-protocol`, `efr-stdx` |
| `efr-holder` | lib | 1 | the `PtyHolder` trait and holder wire types; no IO, no unsafe | `efr-protocol`, `efr-stdx` |
| `efr-http` | lib | 1 | the reqwest client, SSE parser, WebSocket client, Unix-socket HTTP client, header redaction | `efr-stdx` |
| `efr-screen` | lib | 1 | the `Screen` trait, `ScreenActor` and `ScreenHandle`, the OSC 133 and OSC 7 scanner, the conformance suite; `vt100`: `Screen` over vt100, the Zig-free default | `efr-protocol`, `efr-stdx` |
| `efr-provider` | lib | 1 | the `Provider` and `TokenSource` traits, canonical messages, function and freeform tool definitions | `efr-protocol`, `efr-stdx` |
| `efr-test-support` | dev | 1 | `TestClock`, seeded `TestRng`, temp dirs, in-memory store, NDJSON reader, `ReplayProvider`, `Wait` | `efr-protocol`, `efr-store`, `efr-provider`, `efr-stdx` |
| `efr-render` | lib | 1 | markdown and render events to ANSI: committed and live zones, colour roles and the palette, syntax colours, OSC 8 links, widths by code point or grapheme cluster; no IO, the CLI passes `RenderOptions` | none |
| `efr-sandbox` | lib | 1 | the pure logic of the `auto` sandbox: `SandboxSpec`, `MountPlan` and the bwrap arguments, Landlock and seccomp as data, the environment and export filters, the records, the sandbox state, `result.json`, the surface guard, the worktree record, the probe's result types, the mount table parse and the `df` and `findmnt --df` renderer of the mount view; file access only through `FsView`, no tokio, no unsafe | `efr-protocol` |
| `efr-screen-ghostty` | lib | 2 | `Screen` over libghostty-vt; the only crate that needs Zig | `efr-screen` |
| `efr-pty` | lib | 2 | `LocalPtyHolder`: openpty, `setsid` and `TIOCSCTTY` in `pre_exec`; the only unsafe code at milestone 1 | `efr-holder`, `efr-stdx` |
| `efr-shell` | lib | 2 | one hidden zsh per conversation, shell state from marks, `run_command` | `efr-holder`, `efr-screen`, `efr-protocol`, `efr-sandbox`, `efr-stdx` |
| `efr-sbx` | bin `efr-sbx` | 2 | the launcher of the `auto` sandbox: `run` (one call in bwrap with Landlock and seccomp, or the exit child as a subreaper; with `--job` a background job with a lifeline, low priority and a stop), `inner`, `probe`, `guard-check` (the surface guard of a running job), `statvfs` (efrd's helper that measures the mounts, which efrd can kill), `mounts` (the mount view's `df` and `findmnt --df` tables inside a call) `bridge` (the network proxy's bridge inside a call, from `127.0.0.1:3128` to the call's proxy socket, with its own Landlock domain and seccomp filter) and `dbus-proxy` (runs `xdg-dbus-proxy` for one filtered bus of a call, with its own Landlock domain, seccomp filter and fd limit); checks what comes back and writes `result.json` last; no async runtime; its one `unsafe` module is `fds.rs` (ADR 0007) | `efr-sandbox`, `efr-protocol` |
| `efr-tools` | lib | 2 | the `Tool` trait, the registry, the shell, read_file, write_file, apply_patch and edit tools, the freeform tool spec; knows nothing about permissions; `patch`: the patch engine of apply_patch and edit (the patch text to file operations, its Lark grammar, hunks matched with tolerant passes, every new content computed before anything is written, one exact replacement; no IO, no tokio) | `efr-shell`, `efr-scope`, `efr-protocol`, `efr-stdx` |
| `efr-providers` | lib | 2 | `openai`: the Responses API client over HTTP or a WebSocket for each conversation; the model catalog (the fetch from the backend, its cache file and the built-in table, which the API key backend cuts down to the ids that its key lists), which also says which models take freeform (`custom`) tools and prefer WebSockets; the check of an API key; takes tokens only through `TokenSource`; `anthropic`: the Messages API client with an API key (provider id `anthropic-api`): the request body with its prompt cache markers (a pure function), the stream mapping, the usage sums, the error and retry classifier, the model catalog from `GET /models` with its cache file and no built-in table, the key check; takes the key only through `TokenSource`; the two modules share no code | `efr-provider`, `efr-http`, `efr-protocol`, `efr-stdx` |
| `efr-oauth-openai` | lib | 2 | the subscription login: PKCE, loopback callback, refresh, `OpenAiTokenSource` | `efr-http`, `efr-credentials`, `efr-provider`, `efr-stdx` |
| `efr-snapshot` | lib | 2 | efr's own snapshot store: one bare git repository per project or `$SCRATCH` in the data root, hardened git through `efr_scope::Git::command`, the trees before and after each call that can write, the turn's `pre`, `last` and `post` refs and the paths that its calls touched, the mirror of a project's refs and objects from the git dirs that efrd trusts, the check that a snapshot covers each file below a target, the changes of a call or a turn, the diff of a turn, the plan of an undo or a redo (pure: it writes no file of a root), the home store of the files outside the roots, the collector | `efr-scope`, `efr-protocol`, `efr-stdx` |
| `efr-config` | lib | 2 | `config.toml` for `efrd` and `efr`: the schema of every key, defaults, validation, the effective view with sources, the JSON schema, the example file and the format-preserving writer; no async, no network | `efr-permissions`, `efr-protocol`, `efr-stdx` |
| `efr-proxy` | lib | 2 | the network proxy of the `auto` sandbox, as tokio tasks in efrd: one listener per contained call, the request head and ClientHello parsers, the allow and deny lists before any lookup, the call's host grants, the address guard, the SNI check, plain `GET` and `HEAD` for http hosts, the upload budget and the refusal records; the resolver, the connector and the host's addresses are seams | `efr-protocol`, `efr-stdx`, `efr-permissions` |
| `efr-conversation` | lib | 3 | one actor per conversation: queue, turn loop, the single permission check point, approvals, interrupt, steer, withdraw; in `auto` the launch of each shell call, exit questions and their records (in full mode too, with the judge `full`, the timed question of a guarded delete and the gate before each call), the review of an exit by the `ExitJudge` before the user with the allow rule and the naming rule, the denial pause, the away wait through `PresenceReader`, the cache delete questions, the quarantine question and the fallback to `cautious`; the undo of a turn between turns; drives tools through its own `Toolbox` trait, implemented by `efr-daemon` | `efr-provider`, `efr-permissions`, `efr-scope`, `efr-store`, `efr-sandbox`, `efr-protocol`, `efr-stdx` |
| `efr-transport` | lib | 3 | the protocol edge: codec, Unix listener, connection table, subscriptions, the `Dispatcher` trait; `client`: the client side of the protocol for `efr`, tests and the proxy | `efr-protocol`, `efr-stdx` |
| `efr-daemon` | bin `efrd` | 4 | the composition root; one file per protocol method; the settings tool, which needs `efr-config` and so cannot live in `efr-tools`; the `auto` sandbox service: the launcher's copy, the probe, the spec of each call, the plan lock, the facts of a line (with the git remotes), the turn-end report, the read-only scope of model-side socket peers, the gate of full mode (`full.rs`: whether efrd allows or refuses it, the count of running full work) and the config hold (`loosens.rs`, `reload.rs`), and the network proxy's socket for each call with its lifetime, its host list and the hosts that a token in a project file denies; the bus filter: an `xdg-dbus-proxy` for each filtered bus of a call or a job, with its self-test and the refusals of its log; the writer of always-allow rules, which checks a rule and appends it to `config.toml`; the judge of `auto` exits (the reviewer's request and its chain of models), the presence of each conversation, the table of background jobs (the launch, the output file, the time limit, the stops and the sweep of a job's session), the home copies of the files that an approved exit, or an exit that full mode let through, writes outside the roots, the apply of an undo or a redo with the undo trash, and the deletes in the real tool caches | every library crate above except the test crates |
| `efr-cli` | bin `efr` | 4 | `efr send`, `new`, `status`, `history`, `diff`, `compact`, `jobs` (list, stop), `settings`, `models`, `login` (openai, openai-api, anthropic), `logout`, `config` (show, check, edit, set, unset, schema, reload), `project` (list, add, remove, through the daemon), `paths`, `sandbox` (check, explain, allow), `undo`; renders replies through `efr-render` | `efr-transport`, `efr-config`, `efr-render`, `efr-protocol`, `efr-stdx` |
| `efr-test-daemon` | dev | T | `TestDaemon` and scenario replay; used only from `tests/` of `efr-daemon` and `efr-cli` | `efr-daemon`, `efr-test-support`, `efr-transport`, `efr-protocol` |

None of these crates exists in the first commit; they land in the order of the
milestone 1 file map (`research/rust-structure.md`, section 10).

## The dependency rule

Dependencies point down. The only edges inside one tier are `efr-tools -> efr-shell`
and `efr-daemon -> (everything)`.

`cargo xtask deps` reads `cargo metadata` (all features) and fails when:

1. a member depends on a workspace crate that is not on its allowlist, or a member
   has no allowlist entry;
2. one of these edges exists, directly or through any chain of normal dependencies:
   `efr-tools -> efr-permissions`, `efr-providers -> efr-oauth-openai`,
   `efr-conversation -> efr-shell`, `efr-conversation -> efr-transport`,
   `efr-transport -> efr-store`, `efr-protocol -> tokio`,
   `efr-test-support -> efr-daemon`, `efr-sandbox -> tokio`, `efr-sbx -> tokio`,
   `efr-proxy -> efr-daemon`;
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
- The proxy is a library that efrd composes. It gets the host list, the call's grants
  and its seams from efrd, and knows nothing of the daemon.
- The patch engine stays pure: it computes every new content from texts that the
  caller read, so a patch over several files is checked whole before the tool writes
  one of them. The engine is the module `efr_tools::patch`, so no crate edge can
  guard it; `cargo xtask tidy` does (`PURE_MODULES` in `xtask/src/tidy.rs`).
- Zig is a build requirement of one crate, and SQLite has one owner.

The test for splitting a module into a crate, or folding one back: it gets heavy
dependencies of its own, or a second binary needs it.

## The project's git dir and undo

- A snapshot never writes a project's git dir. git runs on a project's own git dir
  only through `ProjectGit` (`crates/efr-snapshot/src/project_git.rs`), with hooks,
  fsmonitor and automatic gc off, and only on the git dirs that efrd trusts: the work
  tree record of `crates/efr-daemon/src/sandbox/projects.rs` when it matches, else
  `<root>/.git` when it is a directory. The store pins both dirs by their `(dev, ino)`
  and never follows a `.git` file in a root. A repository with alternates, or with a
  `commondir` file where none belongs, gets no mirror and no undo of its refs.
- Only an applied undo writes a project's git dir, and only its refs (compare and swap)
  and the objects that the project lost, while efrd holds the plan lock of the root.
  It never writes the project's config, its hooks or its index.
- `efr-snapshot` plans an undo and writes no file in a root or in the home directory.
  efrd writes them (`crates/efr-daemon/src/tools/undo.rs`): through `DaemonFs`, never
  through a link, after it reads each path again and compares it with the plan. Each
  file that undo overwrites or removes goes to the undo trash
  (`$XDG_DATA_HOME/efr/undo-trash/<undo-id>/`) first.

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
  diff of a turn in `crates/efr-daemon/src/methods/conversation_diff.rs`. The groups
  of the changed files (generated, tests, docs, code) in
  `crates/efr-cli/src/format/groups.rs`, the groups of `[diff.groups]` and their globs
  in `crates/efr-config/src/tables/diff.rs`.
- The reviewer of the `auto` exits (the classifier): the approver of each exit
  (`Approver`) in `crates/efr-permissions/src/exits.rs`, and the words that name an
  exit and the one check of them (`names_match`) in
  `crates/efr-permissions/src/exits/names.rs`; every user message of a conversation,
  which that check reads, in `crates/efr-store/src/user_messages.rs`; the seam
  `ExitJudge` in `crates/efr-conversation/src/judge.rs`; who decides an exit (`exit::route`) and the
  allow rule and the naming rule (`exit::effective`) in
  `crates/efr-conversation/src/exit.rs`; the review and the denial pause in
  `crates/efr-conversation/src/turn/review.rs`. efrd's judge, with the request, the
  chain of models and the check of the answer, is in `crates/efr-daemon/src/judge.rs`.
  What it keeps between exits is in `judge/memory.rs`, and its prompt is in
  `judge/prompt.md`. The small model of each company is `Vendor::classifier_model` in
  `crates/efr-daemon/src/catalog/list.rs`, from the `CLASSIFIER_MODEL` constant of each
  provider crate. `[classifier]` is in `crates/efr-config/src/tables/classifier.rs`. The
  eval of the corpus lines and of the auto prompt is in
  `crates/efr-daemon/tests/it/classifier_eval.rs`.
- Presence and the away wait: the seam `PresenceReader` in
  `crates/efr-conversation/src/presence.rs`; the wait of a question while the user is
  away in `crates/efr-conversation/src/turn.rs`; the state of each conversation in
  `crates/efr-daemon/src/presence.rs`; `presence.set` in
  `crates/efr-daemon/src/methods/presence_set.rs`; `efr away` in
  `crates/efr-cli/src/commands/away.rs`; `[presence]` in
  `crates/efr-config/src/tables/presence.rs`.
- Undo and the home store: the store's refs, the mirror (`mirror.rs`), git on the
  project's own git dir (`project_git.rs`), the check that a snapshot covers a target
  (`cover.rs`), the plan of an undo or a redo (`undo.rs`) and the home store
  (`home.rs`) in `crates/efr-snapshot/src/`. What a destructive line or a file tool
  asks to be covered (`CoverAsk`) in `crates/efr-permissions/src/request.rs`; which
  exits get a home copy (`ExitNeed::copies`, `ExitNeed::backup_before_write`) and the
  rule that a covered destructive part is routine (`destructive`) in
  `crates/efr-permissions/src/exits.rs`. efrd's answer to the asks before the decision
  in `crates/efr-daemon/src/tools/snapshot.rs` (`CallSnapshots::covers`) and
  `crates/efr-daemon/src/home.rs` (`coverage`), from the facts of a call
  (`DaemonToolbox::facts`) in `crates/efr-daemon/src/tools.rs`; the home copies before
  and after a call in `crates/efr-daemon/src/home.rs`, from the one hook before a call
  runs (`DaemonToolbox::invoke`); the end of a turn (`TurnUndoInfo`) in
  `crates/efr-daemon/src/tools/snapshot.rs`; the apply of an undo and the undo trash
  in `crates/efr-daemon/src/tools/undo.rs`, through `DaemonFs` in
  `crates/efr-daemon/src/sandbox/fs.rs`; `turn.undo` in
  `crates/efr-daemon/src/methods/turn_undo.rs`; the actor's undo and its refusals in
  `crates/efr-conversation/src/actor/undo.rs`; `home_file_changed` in
  `crates/efr-conversation/src/turn.rs`; `efr undo` in
  `crates/efr-cli/src/commands/undo.rs`; the lines at the end of a turn in
  `crates/efr-cli/src/format/changes.rs` and `crates/efr-cli/src/format/sandbox.rs`.
  Named persistence that is no file edit: the routes of a named unit, cron or linger
  change in `crates/efr-permissions/src/exits/persist.rs`; the links that a unit
  change makes in `crates/efr-daemon/src/sandbox/units.rs`; the copies of the crontab
  and the linger setting in `crates/efr-daemon/src/home/state.rs`; the `crontab` and
  `loginctl` that efrd runs itself in `crates/efr-daemon/src/trusted_programs.rs`.
  The collector of the phase A backups, which efrd no longer writes, is in
  `crates/efr-daemon/src/backups.rs`. The tests from the prompt to the files are in
  `crates/efr-daemon/tests/it/undo.rs` and `crates/efr-daemon/tests/it/sandbox/undo.rs`.
- Deletes that reach the real tool caches: the walk of the whiteouts in an upper layer
  (`CacheDeletes`) and the read path of a layer (`LayerView`) in
  `crates/efr-sandbox/src/layers/deletes.rs`; the launcher's reads of the uppers that
  the layer helper passed in `crates/efr-sbx/src/deletes.rs`; the engine's decision
  (`Engine::decide_cache_delete`) in `crates/efr-permissions/src/engine.rs`; the
  question in `crates/efr-conversation/src/turn/cache.rs`; efrd's check and delete in
  `crates/efr-daemon/src/tools/cache_deletes.rs`, through `DaemonFs::remove_beneath` in
  `crates/efr-daemon/src/sandbox/fs.rs`.
- Background jobs (ADR 0010): the wire types in `crates/efr-protocol/src/jobs.rs`;
  `[jobs]` in `crates/efr-config/src/tables/jobs.rs`; the `jobs` projection, the
  notices that no turn took and the wake turns of the last hour in
  `crates/efr-store/src/jobs.rs`; the kinds that cannot run as a job (`job_refusal`) in
  `crates/efr-permissions/src/jobs.rs`; the launcher's job mode in
  `crates/efr-sbx/src/job.rs` and `guard-check` in `crates/efr-sbx/src/guard_check.rs`;
  the `env` file of the hidden shell in `crates/efr-shell/assets/zsh/efr-integration.zsh`.
  In efrd: the job table in `crates/efr-daemon/src/jobs.rs`, the spawn with the lifeline
  in `jobs/launch.rs`, the output file in `jobs/output.rs`, the sweep by session id in
  `jobs/sweep.rs`, the guard of a running job in `jobs/guard.rs`, the jobs that keep
  an undo from running in `jobs/undo.rs`, the programs that a job wrote in
  `jobs/since.rs`, the daemon-wide window of wake turns in `jobs/wakes.rs` and the
  removal of old job outputs in `jobs/retention.rs`; a job's writes join its start turn
  through `Snapshots::after_job` in `crates/efr-snapshot/src/job.rs`; the start of a job
  and the tools `jobs` and `job_stop` in `crates/efr-daemon/src/tools/job_tools.rs`;
  the running jobs of the hidden shell (the `jobs` builtin) in
  `crates/efr-daemon/src/tools/shell_children.rs`; `jobs.list` and `jobs.stop` in
  `crates/efr-daemon/src/methods/jobs_list.rs` and `jobs_stop.rs`. In the actor: the
  text of a notice in `crates/efr-conversation/src/notice.rs`, the wake turns and their
  limits in `crates/efr-conversation/src/wake.rs` and
  `crates/efr-conversation/src/actor/notices.rs`. `efr jobs` in
  `crates/efr-cli/src/commands/jobs.rs`, and the lines of the view and of `efr history`
  in `crates/efr-cli/src/format/jobs.rs`. The fake launcher of the tests is
  `crates/efr-test-daemon/src/fake_launcher.rs`.
- The OSC 133 and OSC 7 scanner: `crates/efr-screen/src/shell_marks/`.
- Freeform tools, whose input is text: the definition in
  `crates/efr-provider/src/request.rs`, which models take the freeform form in
  `crates/efr-providers/src/openai/catalog.rs` (`ModelInfo::freeform_tools` from the
  catalog's `apply_patch_tool_type`), the `custom` items in `crates/efr-providers/src/openai/convert.rs`. The patch engine of
  `apply_patch`: `crates/efr-tools/src/patch/`; the tool: `crates/efr-tools/src/apply_patch.rs`
  and its contract in `crates/efr-tools/README.md`; the question for each delete and
  move: `Requirements::destructive` in `crates/efr-permissions/src/engine.rs`.
- The `edit` tool of Claude models: `crates/efr-tools/src/edit.rs` on
  `efr_tools::patch::replace`. Which model gets `apply_patch` and which `edit`:
  `ModelInfo::edit_tool` in `crates/efr-provider/src/provider.rs` (and
  `Provider::default_edit_tool` for a model that the list does not name), read by
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
  `crates/efr-conversation/src/preamble/secrets.rs`; the terminal's recent commands
  in the preamble in `crates/efr-conversation/src/preamble/recent.rs`, from the
  plugin's ring in `shell/zsh/efr.plugin.zsh` through `EFR_RECENT_COMMANDS` and
  `prompt.send` `recent_commands`; the measurement on the OpenAI path
  in `crates/efr-daemon/tests/it/cache_prefix.rs`.
- The model catalog: the fetch, the cache file and which models are on offer in
  `crates/efr-providers/src/openai/catalog.rs` and
  `crates/efr-providers/src/anthropic/catalog.rs`; the built-in OpenAI table, the last
  fallback, in `crates/efr-providers/src/openai/models.rs` (Claude models have none);
  the catalog of either company and its cache file name in
  `crates/efr-daemon/src/catalog/list.rs`; when efrd fetches, the wait of a prompt for
  a first Claude list (`Models::ready`), the effective model list, the default model
  and the windows of `[openai] models` and `[anthropic] models` in
  `crates/efr-daemon/src/catalog.rs`; which provider efrd builds for `[model] provider`
  in `crates/efr-daemon/src/providers.rs`.
- Logins with an API key: where `efr` reads a key (a hidden prompt, stdin or
  `--from-env`) in `crates/efr-cli/src/commands/login/key.rs`; the method in
  `crates/efr-daemon/src/methods/admin_login_api_key.rs`; the checks of the key's text,
  its hint and the check with its provider in
  `crates/efr-daemon/src/providers/api_key.rs`; the requests of the check in
  `crates/efr-providers/src/openai/catalog/client.rs` and
  `crates/efr-providers/src/anthropic/catalog/client.rs` (`check_key`); when the
  provider refused a stored key, for `admin.status`, in
  `crates/efr-daemon/src/providers/refusals.rs`. The user's
  guide to the providers, their logins and the Claude settings: `docs/providers.md`.
- The Responses WebSocket transport: the contract in
  `crates/efr-providers/README.md`, section "WebSocket transport"; the
  connection of each conversation and the fallback to HTTP in
  `crates/efr-providers/src/openai/websocket.rs`; incremental input in
  `crates/efr-providers/src/openai/websocket/continuation.rs`; the WebSocket client in
  `crates/efr-http/src/websocket.rs`; `[openai] websocket` in
  `crates/efr-config/src/tables.rs`.
- The Anthropic provider: the contract (the request, the prompt cache markers, the
  catalog, the token counts and the failures) in `crates/efr-providers/README.md`;
  the placement of the cache markers and their debug line in
  `crates/efr-providers/src/anthropic/convert/breakpoints.rs`, and the gap before each
  model call (`gap_ms`, for the measurement of the time to live) in
  `crates/efr-conversation/src/gap.rs`; which error answers are
  sent again in `crates/efr-providers/src/anthropic/failure.rs`; the fake Messages API
  and model list for daemon tests, `MessagesServer`, in
  `crates/efr-test-daemon/src/test_daemon/messages.rs` (`TestDaemonBuilder::messages`
  runs the real provider against it), and a whole Claude conversation over it in
  `crates/efr-daemon/tests/it/anthropic.rs`; the canonical token counts
  of every provider in `crates/efr-provider/README.md`, section "Token counts".
- The views of the host (ADR 0011, phase D1). The network view: `Grant::NetView` and
  `SandboxStatus::netview` in `crates/efr-protocol/src/sandbox.rs` and
  `sandbox/status.rs`; `[network]` in `crates/efr-config/src/tables/network.rs`; the
  line rule in `crates/efr-permissions/src/exits/view.rs`, the view reason
  (`Subject::View`) in `crates/efr-permissions/src/decision.rs` and its place in the
  engine in `crates/efr-permissions/src/engine.rs`; the union of view and exit grants
  (`exit::grant`) in `crates/efr-conversation/src/exit.rs`; the host namespace and
  the profile in `crates/efr-sandbox/src/plan/build.rs` and
  `crates/efr-sandbox/src/seccomp.rs` (`SeccompProfile::net_view`), the Landlock TCP
  rights in `crates/efr-sandbox/src/landlock.rs`; the launcher's compile of the
  netlink rule in `crates/efr-sbx/src/seccomp.rs`, its self-test in
  `crates/efr-sbx/src/self_test.rs` and the probe's second launch in
  `crates/efr-sbx/src/probe.rs`; efrd's engine after each probe (`Probed`) in
  `crates/efr-daemon/src/engine.rs` and `crates/efr-daemon/src/reload.rs`. The mount
  view: the parse, the rules of the sizes and the renderers in
  `crates/efr-sandbox/src/mounts.rs`; `efr-sbx statvfs` and `efr-sbx mounts` in
  `crates/efr-sbx/src/statvfs.rs` and `crates/efr-sbx/src/mounts.rs`; efrd's snapshot
  with its deadline and the check of its mount namespace in
  `crates/efr-daemon/src/sandbox/mounts.rs`; the `df` and `findmnt` functions of the
  child shell in `crates/efr-shell/assets/zsh/efr-child.zsh`. The tests on the real
  launcher are in `crates/efr-sbx/tests/it/netview.rs`, `mount_view.rs` and
  `crates/efr-daemon/tests/it/sandbox/netview.rs`, `mount_view.rs`; the eval of the
  view prompts in `crates/efr-daemon/tests/it/classifier_eval.rs`.
- The network proxy (ADR 0012, phase D2). Trust: efrd parses bytes that come from the
  sandbox here, in an unconfined process, so `efr-proxy` parses only the request head
  (`crates/efr-proxy/src/head.rs`) and the TLS ClientHello
  (`crates/efr-proxy/src/hello.rs`) with hand parsers, hard caps and proptests, and
  forbids `unsafe`; after the checks it copies opaque bytes. The host list, the
  presets, the pacman mirror parser and the token scanners are pure, in
  `crates/efr-permissions/src/network.rs` and `network/tokens.rs`; the config keys in
  `crates/efr-config/src/tables/network.rs`. The proxy itself: the listener of a call
  with its budget and records in `crates/efr-proxy/src/call.rs`, the order of the
  checks of a `CONNECT` in `crates/efr-proxy/src/tunnel.rs`, plain HTTP in
  `crates/efr-proxy/src/plain.rs`, the lookup and the connect in
  `crates/efr-proxy/src/upstream.rs`, the address guard in
  `crates/efr-proxy/src/guard.rs`, and the system resolver, connector and addresses in
  `crates/efr-proxy/src/system.rs` (the only place that resolves or connects). In efrd:
  the proxy service, the bind of each call's socket, its lifetime and the refusals in
  `crates/efr-daemon/src/sandbox/proxy.rs` (with `proxy/bind.rs`, `proxy/mirrors.rs`
  and `proxy/tokens.rs`), the network plan of a call in
  `crates/efr-daemon/src/sandbox/plan.rs`, the egress of the engine in
  `crates/efr-daemon/src/engine.rs`, the git remote facts in
  `crates/efr-daemon/src/sandbox/facts/remotes.rs`, and the test seams of the proxy in
  `crates/efr-daemon/src/sandbox/seams.rs`. In the launcher: the read-only bind of the
  socket and the proxy variables in `crates/efr-sandbox/src/plan/build.rs` and
  `crates/efr-sandbox/src/plan.rs`, the bridge's Landlock domain and seccomp profile in
  `crates/efr-sandbox/src/landlock.rs` and `crates/efr-sandbox/src/seccomp.rs`, the
  bridge in `crates/efr-sbx/src/bridge.rs`, its start in `crates/efr-sbx/src/inner.rs`
  and its self-test in `crates/efr-sbx/src/self_test.rs`. The engine's rules under the
  proxy are in `crates/efr-permissions/src/exits/programs.rs` and
  `crates/efr-permissions/src/exits/needs.rs`, and the naming rule with the allow
  list, `project_remote` and `refused_by_proxy` in
  `crates/efr-conversation/src/exit.rs`. The tests on the real launcher are in
  `crates/efr-sbx/tests/it/proxy.rs` and `crates/efr-daemon/tests/it/sandbox/proxy.rs`.
- The bus filter and always-allow rules (ADR 0013, phase E). The bus filter: the
  built-in read methods, the checks of `[[dbus.allow]]` and the proxy's arguments in
  `crates/efr-permissions/src/bus.rs`, with the recorded calls in
  `crates/efr-permissions/tests/fixtures/bus-recording.toml`; the read forms and the
  view in `crates/efr-permissions/src/exits/programs.rs` and `exits/view.rs`; the keys
  in `crates/efr-config/src/tables/dbus.rs`; the plan step with the `/run/dbus` tmpfs
  and `RESOLVE_UNIX` in `crates/efr-sandbox/src/plan/build.rs`, the wrapper's Landlock
  domain and seccomp profile in `crates/efr-sandbox/src/landlock.rs` and `seccomp.rs`;
  the wrapper in `crates/efr-sbx/src/dbus_proxy.rs` and its self-test in
  `crates/efr-sbx/src/self_test/bus_proxy.rs`; the `systemctl` function of the child
  shell in `crates/efr-shell/assets/zsh/efr-child.zsh`; efrd's supervisor, its
  self-test and its status in `crates/efr-daemon/src/sandbox/bus.rs`, the log parser
  in `crates/efr-daemon/src/sandbox/bus/log.rs`, the trusted program in
  `crates/efr-daemon/src/trusted_programs.rs` and the seams of the test buses in
  `crates/efr-daemon/src/sandbox/seams.rs`; the model's note in
  `crates/efr-tools/src/shell_tool.rs` (`bus_note`). Always-allow: the rule type, its
  floor checks and its match in `crates/efr-permissions/src/always.rs`,
  `always/check.rs` and `always/cover.rs`; the key in
  `crates/efr-config/src/tables/sandbox.rs` and the writer's `add_table_entry` in
  `crates/efr-config/src/writer.rs`; the check in the turn in
  `crates/efr-conversation/src/turn/always.rs`; efrd's writer in
  `crates/efr-daemon/src/always.rs`, the reload in `crates/efr-daemon/src/reload.rs`
  and the real-path facts in `crates/efr-daemon/src/sandbox/facts.rs`; the card and
  `efr sandbox allow` in `crates/efr-cli`. The tests on the real launcher and the real
  proxy are in `crates/efr-sbx/tests/it/dbus.rs`,
  `crates/efr-daemon/tests/it/sandbox/dbus.rs` (private `dbus-daemon` buses with the
  fake services of `dbus/fake_bus.py`) and `crates/efr-daemon/tests/it/sandbox/always.rs`;
  the turn-level tests with a fake proxy in `crates/efr-daemon/tests/it/bus.rs`; the
  eval of the bus prompts in `crates/efr-daemon/tests/it/classifier_eval.rs`.
- Full mode (ADR 0015): the user guide in `docs/permissions.md`, section "Full mode";
  the settle pass of a full call in `crates/efr-permissions/src/full.rs` and its place
  in `Engine::decide` (`crates/efr-permissions/src/engine.rs`); the support of a machine
  with no sandbox (`AutoSupport::for_full`) in `crates/efr-permissions/src/support.rs`;
  the delete guard in `crates/efr-permissions/src/exits/critical.rs`, with its types in
  `crates/efr-permissions/src/critical.rs`; the gate key `permissions.allow_full` and
  its load check in `crates/efr-config/src/tables.rs` and `validate.rs`; the gate at
  each resolve, the phone cap and the wake cap in
  `crates/efr-conversation/src/settings.rs`; the gate before each call, the record of
  what full mode let through and the timed question in
  `crates/efr-conversation/src/turn/full.rs`; the model's text in
  `crates/efr-conversation/src/preamble.rs`; efrd's refusals (root, undo off, no zsh)
  and the count of full work in `crates/efr-daemon/src/full.rs`; the config hold in
  `crates/efr-daemon/src/reload.rs` with the keys that loosen in
  `crates/efr-daemon/src/loosens.rs`; the forget key of sudo before a call in
  `crates/efr-shell/src/session.rs`; the marker in `crates/efr-cli/src/follow/` and
  `crates/efr-cli/src/format/sandbox.rs`, and the plugin's tag in
  `shell/zsh/efr.plugin.zsh`. The tests over the socket are in
  `crates/efr-daemon/tests/it/full.rs` and, on the real launcher,
  `crates/efr-daemon/tests/it/sandbox/full.rs`.
- On-disk layout and schema: `docs/storage.md`. The libghostty pin: `docs/ghostty-pin.md`.
- Decisions that are expensive to reverse: `docs/adr/`.
