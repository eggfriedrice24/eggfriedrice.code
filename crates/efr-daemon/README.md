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
   (and prompts an earlier daemon held) recorded as not run with `turn_cancelled` and
   one notice per conversation to its terminal ("efr restarted; a queued prompt did not
   run; see it with efr history <id> and send it again"). The notice never quotes a
   prompt: after a reboot the terminal name may belong to another terminal by now.
   Running shells recorded as exited, process-bound outbox items
   cancelled. A prompt never runs by surprise after a restart.
5. The PTY table, the recording sink, the shells, the model catalog of
   `model.provider` (`catalog.rs`: its cache file in the state root, else the built-in
   table of OpenAI, or no list for Anthropic), the providers (`providers.rs`:
   `openai-subscription`, `openai-api` or `anthropic-api`), the tool registry and the
   settings tool (`tools.rs`), the permission engine and the conversation registry.
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
   The sandbox service (`sandbox.rs`) copies the installed launcher (`efr-sbx` next to
   `efrd`, else in `../lib/efr/`) to `$XDG_RUNTIME_DIR/efr/bin/efr-sbx` (mode 0500),
   checks its SHA-256 and runs the probe once before the socket opens. The
   conversations read the probe's status from a `watch` of `SandboxStatus`; while it
   says unavailable, an `auto` turn runs as `cautious` with the probe's reason. See
   "The auto sandbox" below.
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

`reload.rs` reads `config.toml` again while the daemon runs. These triggers ask its one
task for a reload: `admin.config_reload` (`efr config reload`), `admin.project_add` and
`admin.project_remove` after their write, SIGHUP
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
and a retargeted link all reload. A change of the project registry `projects.toml` in
the config root reloads too, so the engine knows a project from the turn that first
finds it in the registry on. When the kernel's queue overflows or a watched
directory is removed or moved, the watches are armed again and the file reloads once.

A reload checks the whole file with `efr-config`. A file with an error changes nothing:
the old settings stay, the error (with its line, column and key) is kept for
`admin.status`, and each terminal with a conversation active within
`conversation.tty_idle_hours` gets the notice "efr: config.toml has an error; the old
settings stay: ...", once per new error. A valid file is laid over the running settings
with `Settings::reloaded`: a restart key (`screen`, `model.provider`,
`openai.originator`, `openai.subscription_base_url`, `openai.api_base_url`,
`openai.websocket`) keeps its running value and is listed in `restart_needed` (and in
the notice "efr: restart efrd to apply: ..."), and a value from `EFR_LOG`,
`EFR_SCREEN` or a flag still wins. Then
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

### The model catalog

`catalog.rs` keeps the model catalog of the active provider (`Models`), and every
reader takes it from memory: a turn when it starts, a prompt when it arrives,
`models.list`, `admin.status` and the settings tool. A new list applies from the next
turn on. The catalog has the form of the provider's company (`ProviderCatalog`, an
OpenAI or an Anthropic catalog), and the provider reads the same shared catalog for
each request. Each reader sees it as one `ModelList`: the models, the default, where
the list came from and when.

- At start, efrd reads the cache file of the configured provider in its state root:
  `model_catalog.json` for OpenAI, `anthropic_model_catalog.json` for Anthropic, the
  last list that the configured backend sent. A file of another backend, a broken file
  or a list that offers no model is not used. Then the table built into efr stands in
  for OpenAI, and Anthropic has no list. So a start while offline offers the last
  list.
- In the background (a task of `run.rs`), efrd fetches the subscription's catalog
  with the login's token (`efr_provider_openai::CatalogClient`): at start, after a
  login, then every hour (`REFRESH_INTERVAL`). The list that efrd has goes with its
  tag in `If-None-Match`, so an unchanged list costs a 304, which confirms it. A new
  list or a confirmed one goes to the cache file in one step. Without a login, efrd
  waits for one. After a failed fetch it keeps the list that it has and tries again
  sooner (`retry_wait`): after 15 s, 30 s, 1 min and 2 min for the first four
  failures in a row, then every 5 min, so a start before the network is up gets a
  list soon. A good fetch starts the waits over. A list that offers no model that
  efr can use is refused with a debug line, and the current list stays. Today the
  backend sends an empty list: it lists each model only for a Codex `client_version`
  at or above the model's minimum, and efr sends its own version. So the built-in
  table is the list in use, and a new release of efr updates it.
- The API key backend fetches its `/v1/models` the same way, with the stored key and
  the `[openai] organization` and `project` headers, at start, after a login and
  hourly. That list holds ids only, so it cuts the built-in table down to the models
  that the key lists (`efr_provider_openai::Catalog::from_api`); windows, efforts and
  tool forms stay those of the table. Without a stored key, efrd waits for a login.
- The Anthropic catalog comes from the API's `GET /v1/models`, page by page, with the
  stored `anthropic-api` key, the `anthropic-version` header and the workspace of
  `[anthropic]` (`efr_provider_anthropic::CatalogClient`), at the same times and with
  the same waits after a failure. Only `active` models are on offer, with the window,
  the output limit and the efforts that the API gives. efr has no table of Claude
  models, so while there is no list a prompt and `conversation.compact` first wait for
  one fetch (`Models::ready`, at most 60 s): a turn without a list cannot know the
  output limit of its model, and efr never guesses it. When the fetch gives no list
  either, the prompt fails with the cause (`Providers::check_list`): without a stored
  key, `unauthorized` with the login command (`anthropic-api has no model list yet,
  because no key is stored; log in with efr login anthropic`); after a failed fetch,
  `efr could not fetch Claude's model list:` and the fetch's error, such as the
  server's message for a refused key (`unauthorized`), with the code of that error. A
  prompt on a model whose output limit the config gives (`[anthropic] models` or
  `[model] max_output_tokens`) runs without a list, unless no key is stored.
- A daemon with a `ProviderFactory` (an in-process test) never fetches, so a test never
  reaches the network by accident.
- The effective list (`effective_models`) is the catalog's models on offer (OpenAI's
  best priority first, Anthropic's in the API's order), then the ids of the config's
  list of the company (`[openai] models` or `[anthropic] models`) that the catalog does
  not hold. A window that an entry gives raises or lowers the catalog's window up to
  the model's `max_context_window`; above it, efrd uses the largest one and warns once
  per entry (`efr config check` notes it too). The provider gets the entry's window
  and output limit as well (`openai_config`, `anthropic_config`).
- The default model (`default_model`) is `[model] name`, else the catalog's default
  (OpenAI's best priority; Anthropic's `claude-opus-5-5` when listed, else its first
  model), else the first id of the config's list, else `claude-opus-5-5` for
  Anthropic before its first list.

### The edit tool of a model

The registry (`tools.rs`) holds both tools that change files, `apply_patch` and
`edit`, but a request offers only one: `DaemonToolbox::definitions(edit)` leaves out
the one that the model does not know (`efr_provider::ModelInfo::edit_tool`, which the
turn passes). OpenAI models get `apply_patch`, Claude models `edit`. The registry keeps
both, so a model can still name the other one, such as an `apply_patch` call of an
earlier model in the history: `requirements` refuses such a call before the engine
judges it, from `CallContext::edit_tool`, and the model reads
`This model has no apply_patch tool, and the call did not run. Use the edit tool to
change a file.` Both tools write in the daemon (`FILE_WRITERS`): the plan lock, the
turn's first snapshot and the shown paths of the approval preview are the same.

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

- Writes (`prompt.send`, `prompt.withdraw`, `turn.interrupt`, `turn.steer`,
  `approval.respond`) answer a retried command id from its receipt. A refusal a retry cannot change (`invalid`,
  `not_found`, `conflict`) is kept as a rejected receipt; a busy or failed one is not.
- The input row of a turn in `efr` (the contract is in the README of efr-protocol):
  `turn.steer` with `if_late: {kind: "queue"}` turns a late steer into a queued prompt
  in the conversation's step that finds it late, with no `turn_steered`; a conversation
  without a live actor gets one for that. Without `if_late` a late steer is a
  conflict, as before. `prompt.withdraw` (`methods/prompt_withdraw.rs`) takes a queued
  prompt back by its turn or as the newest of a terminal: `conflict` when it no longer
  waits, `not_found` for a turn the conversation never queued or a terminal with no
  queued prompt. `turn.interrupt` with `withdraw`, `withdraw_steers` and
  `resend_steers` withdraws those prompts, takes back the unread steers of
  `withdraw_steers` (`steering_withdrawn`) and sends the unread steers of
  `resend_steers` again as one prompt that runs next, in the one append that records
  the request. A retry of either answers from its receipt with
  every sequence number, also those inside the result
  (`efr_conversation::completed_result`). A late steer that became a prompt and resent
  steers hold the notices of the terminal, as `prompt.send` does.
- `prompt.send` routes to the named conversation, to a new one with `new_conversation`
  (`,new`), or to the active conversation of the prompt's terminal (the context's tty,
  else the hello's), starting one when the terminal has none. The terminal's
  conversation ends after `conversation.tty_idle_hours` (12 by default) without
  activity, and when a prompt comes from another shell while the shell that took the
  terminal has exited, so a new tab that reuses a closed tab's `/dev/pts` number
  starts fresh.
  A prompt's `settings` (mode, model, effort) are checked by the conversation
  against the latest settings (`settings.rs` gives it the defaults and the model list
  of `catalog.rs`, both from memory): a model outside the list, or an effort the model does not take,
  is `invalid` with the setting, the value and the choices as data, and nothing is
  recorded. The result carries the effective settings; the turn resolves them again
  when it starts, records them on `turn_started` and sends the effort as the request's
  `effort`. A turn from a remote origin runs with at most `cautious`.
- `conversation.subscribe` subscribes to the store's commits, reads the high-water
  mark, replays a gap of at most 128 events and 1 MiB or sends a bounded snapshot with a
  history cursor, then forwards live events through a 64-item queue. With `drafts`,
  it also forwards the drafts of the running turn (`State::drafts`, a broadcast from
  the turns): after the commits that wait, through the queue's lossy room of 16. It
  drops a draft that is older than an event it sent that ends what the draft shows. A
  draft never closes the subscription. The drafts start with the status of the
  running turn (`ConversationHandle::live_drafts`: its newest `context` draft, and
  `compacting` while it compacts), so a client that attaches during a long model call
  shows the gauge at once.
- `pty.attach` registers for live output, then sends the output after `since_seq` from
  the recording (a gap of at most 1 MiB) or a screen snapshot plus what was recorded
  after it. The replay sends each size that the recording holds as `resized` at its
  place, and it stops at the mark of the registration (`ptys::AttachMark`): live steps
  follow from there, so none comes twice. `pty.resize` refuses a size without rows or
  columns, clamps a huge one, and stores the new size in the recording
  (`StoreRecording::resized`) before attached clients hear it.
- `admin.login_openai` streams the authorize URL, waits for the browser, records
  `login_completed`, makes the running provider forget its cached token and asks for a
  fetch of the model catalog of the new account.
- `admin.login_api_key` (`methods/admin_login_api_key.rs`, `providers/api_key.rs`)
  takes a key for `openai-api` or `anthropic-api`. It refuses another provider and a
  key that is empty or holds whitespace or a character outside ASCII (`invalid`), and
  an OpenAI admin key (`sk-admin-`). Unless `check` is false, it checks the key with
  one request that runs no model, sent once: `efr_provider_openai::check_key` (the
  API's `/models` with the organization and project headers) or
  `efr_provider_anthropic::check_key` (with the base URL and the workspace of
  `[anthropic]`). A refused key is `unauthorized` with the server's message, a busy
  provider `busy`, and a check without an answer `internal`. Then it stores the key in
  the 0600 file store under the provider's id, records `login_completed`, and, when
  the key belongs to the provider of `[model] provider`, asks for a fetch of the model
  list. The answer has the key's hint (the known prefix and the last four
  characters), whether the key was checked and whether the provider is the active
  one. The provider of new conversations never changes: another provider needs
  `[model] provider` and a restart. The key reaches no event, no log line and no
  error text; `StoredApiKey` reads it again at each request, so a new key works
  without a restart.
- `admin.logout` (`methods/admin_logout.rs`) deletes the credential of a provider and
  answers whether one was stored. After a logout of the subscription, the running
  provider forgets its cached token.
- `admin.status` lists every provider (`openai-subscription`, `openai-api`,
  `anthropic-api`) with its login (`subscription` or `api_key`), the expiry of a
  subscription token, the hint of a key and which one is active.
- `prompt.send` refuses a prompt that names no model of its own while `[model] name`
  is a model of another company than `[model] provider` (`efr_config::ForeignModel`):
  `invalid`, with the cause and the fix (set `[model] name` to a model of the provider,
  or remove it) and the setting, the value and the provider as data. efrd also warns
  about such a name at start and after each reload.
- `models.list` answers the effective model list of the latest settings over the
  current catalog (`catalog.rs`, `effective_models`), and where the catalog came from
  (`catalog`: the provider, and `backend`, `cache`, `builtin` or `missing`, with the
  time of the fetch). See "The model catalog" above. `admin.status` says where the
  catalog came from too.
- `conversation.compact` (`methods/conversation_compact.rs`) answers a retried command
  id from its receipt, else starts the conversation's actor when none runs and asks it
  to compact: the actor writes the summary, records `conversation_compacted` with the
  receipt and answers with its `seq` and the compaction. While a turn or another
  compaction runs, and when nothing lies before the verbatim tail, the refusal is
  `conflict` (kept as a receipt); a failed summary request has the code of the
  provider's error (`unauthorized`, `busy`, `invalid` or `internal`) and its message
  says why. The contract is in the README of efr-conversation, section "Context".
  `settings.rs` hands `[compaction]` to the conversations as `CompactionConfig`. For
  the fresh context block after a compaction, the toolbox names the running jobs of
  the hidden shell (`Toolbox::jobs`, `tools/jobs.rs`): the command lines of the
  shell's child processes from `/proc`, at most 10, each cut at 200 characters.
- `admin.config_reload` reloads at once and answers with the outcome: applied, or the
  file's error with the old settings kept, and the keys that wait for a restart.
- `projects.list`, `admin.project_add` and `admin.project_remove` (`projects.rs`) read
  and change the project registry for `efr project`, because the CLI may not depend on
  `efr-scope`. A change runs one at a time, through `efr_scope::RegistryEdit`: it keeps
  the file's comments and its link, changes no file with an error, and plans again when
  the file changed under it. `admin.project_add` registers a directory with links
  resolved; with `git_root`, the root of the git work tree that holds it (guarded
  discovery, so a dotfiles `~/.git` never counts), else the directory, and then it
  refuses the home directory, `/` and the directories above the home directory, which
  only an explicit path registers. `admin.project_remove` matches the root as given and
  with links resolved. Both reload after the write, so the engine trusts the new set of
  projects from the next tool call on, and answer with that reload's outcome; when
  `config.toml` has an error, the project counts for the engine from the next reload
  that succeeds. A broken file or project is `invalid`, a second registration and a
  race are `conflict`, an unknown root is `not_found`. The model can reach these
  methods only by running `efr` in its shell, which no built-in rule allows.
- `admin.status` reports the four roots with where each came from (its own variable,
  `EFR_HOME`, XDG, or `/run/user/<uid>`), and the config file: its path, whether it
  exists, a symbolic link's target, the last reload's error and `restart_needed`; and
  the sandbox's status and paths (the launcher's copy, its source and whether their
  SHA-256 match).
- `admin.sandbox_check` runs the probe now and answers each check; the new status
  counts from then on. `sandbox.explain` (scope `read`) answers what a contained call
  of a turn in `cwd` can do with a path, from the same plan that the launcher builds.
  `sandbox.surface_respond` (scope `approve`, refused for a model-side peer, and by the
  conversation for a phone) answers the quarantine question, with receipts as for
  `approval.respond`.
- `input.respond` types the line a user gave for a running tool call that waits for
  input into the conversation's hidden shell, through `ShellSessions::answer`: only
  while that call's command runs, a wait of it of the answer's kind was reported
  (`hidden` for a hidden wait, not for a visible one) and the job that waited (by its
  process group) still holds the terminal, and for a hidden answer only while the
  terminal reads a line with echo off, all checked by the shell's actor right before
  its one write. A visible answer takes any terminal modes, because behind a relay
  such as `sudo`'s own pty they are the relay's; the shell appends `\r`. A text that
  is not one line of at most `efr_protocol::InputRespond::MAX_TEXT_BYTES` bytes
  without control characters is `invalid`; a conversation without a shell, or in which
  no call's command runs (the call's command ended, or was left at its timeout and
  goes on without a call, so an answer after the call's `tool_call_completed` always
  is), is `not_found`; and a command that does not wait for that
  input (another call's command, one not started yet, one with no wait reported, an
  answer of the other kind than the wait, a hidden answer while the terminal does not
  read a line or echoes, the shell itself or another job than the one that waited
  holding the terminal, as when the command's job ended and zsh's precmd hooks, or a
  command one of them started, run before the command's `D` arrived) is `conflict`;
  nothing is written then. A `manual` answer, which the user typed after `Ctrl+\` for a
  command that reported no wait, goes through `ShellSessions::answer_manual`: it needs
  no reported wait and skips the kind check, and keeps the rest (that call's command
  runs, a job and not the shell itself holds the terminal, the modes for a hidden
  answer, the text rules). There is no receipt. The text is a `SecretText`: it reaches
  no log, error message, event or receipt, and the handler logs only its length. The
  transport zeroes the frame's bytes once it has read and decoded them, the
  `SecretText` and the clone that the shell's actor gets are zeroed when they drop,
  and the shell writes the answer to the PTY from that clone's buffer; serde_json's
  scratch buffer for a text that holds an escape (a quote, a backslash, `\u`) is not
  zeroed.
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
  question) never stops a command. The same count keeps a call that the user approved
  because it may wait for input (`CallContext::approved_interactive`) running past the
  model's `timeout_seconds`: `tools.rs` sets `ToolContext::interactive_limit` from
  `shell.interactive_timeout_minutes` of the latest settings (60 by default), and the
  shell asks once per quiet period past the timeout whether such a client still
  follows. Without one, or once the limit passes, the call answers as at its timeout
  (`Interactive` or `StillRunning`), and its command goes on without a call. A visible
  wait whose prompt looks like a password prompt behind a relay carries `looks_secret`
  on its `tool_call_input_changed` event.
- `shell.sudo_cache` decides what happens to sudo's credential cache on the hidden
  shell's terminal, read from the latest settings at each call (`tools.rs` sets
  `ToolContext::forget_credentials`). `keep`, the default, leaves it to sudo (about
  five minutes), so a second `sudo` soon after an answered one may not ask again.
  `per_call` makes the hidden shell forget the credentials (`sudo -k`, `doas -L`) after
  each call, before anything else runs there, so the next `sudo` asks for the password
  again. It needs the zsh integration: a hidden shell without it keeps the cache, and
  a call inside a nested shell forgets only when that shell exits. Every command with `sudo` still needs the user's approval with either value.

### The auto sandbox

`sandbox.rs` and `sandbox/` hold the daemon's side of the `auto` mode (efr's auto
spec; `docs/sandbox.md` for the user's view):

- The probe (`sandbox/probe.rs`): efrd's own checks first (`sandbox.enabled`, the
  launcher found, outside every write root, its copy equal to its source), then
  `efr-sbx probe --json` from the copy, with the bwrap of `sandbox.bwrap`, the hidden
  shell's zsh and `PATH`, the cache mode and every registered project as a write root.
  It runs at start, after a reload that changes `[sandbox]` or the projects, after a
  call whose sandbox could not start, before an `auto` prompt while the last probe
  failed, and for `admin.sandbox_check`. A change to unavailable after an earlier
  probe is recorded once as `sandbox_unavailable`.
- The spec of a call (`sandbox/plan.rs`): the turn's project, the registered projects
  that the line names (`sandbox.write_projects`), never one at or above the home
  directory; the git dirs of a worktree project when its `.git` file still matches the
  record that `admin.project_add` wrote (`sandbox/projects.rs`, in
  `$XDG_STATE_HOME/efr/sandbox/projects/`); the user's roots and caches; the engine's
  secrets, the sandbox-only masks and the project `.env` files as masks; efr's config
  and its links, the persistence paths of the home directory, `$ZDOTDIR`'s startup
  files, dotfile link targets in a write root (`sandbox/links.rs`), efr's programs, the
  protected names in each project and `sandbox.protect` as floors; the call's grants.
- `prepare` takes the plan lock of the call's projects (`sandbox/lock.rs`), makes the
  target of an approved `MakeFile` or `MakeDir` grant through its parent's descriptor
  (`sandbox/fs.rs`, `openat2` with no link on the way), and writes the call dir
  `$XDG_RUNTIME_DIR/efr/sbx/<conversation>/<call>` (0700) with `spec.json` and `nonce`
  (0600). The toolbox (`tools.rs`) hands the run to the shell tool and lets the lock go
  once the launcher wrote `started`, so a call that runs on past its timeout blocks
  no other plan and no `write_file`, `apply_patch` or `edit`, which take the same lock
  for their own writes.
  Before `prepare`, the toolbox waits until the conversation's shell has no other run
  (`ShellSessions::until_free`, up to the call's timeout), so a call that queues behind
  a command still running holds no lock while it waits.
- Before the engine decides a shell call of an `auto` turn, the toolbox collects its
  facts (`sandbox/facts.rs`): what each target is, the tracked files below each `rm -r`
  directory through the hardened `git ls-files`, and where each program word leads
  and whether it changed in the turn. A word that the shell runs itself
  (`FactRequest::builtins`) leads to `builtin`, never to a file of its name on the
  `PATH`.
- After a call, the quarantine question's "keep" moves the changes back
  (`sandbox/quarantine.rs`), and at the end of the turn the report lists the files
  that run code later outside the sandbox (`sandbox/report.rs`, from the hardened
  `git status` before the turn's first launcher call and at its end).
- An hourly task deletes cache layers idle for `sandbox.cache_days` and the oldest
  above `sandbox.cache_max_gib` (`sandbox/gc.rs`), never those of a running call.
- Socket peers (`sandbox/peers.rs`): a process that descends from a hidden zsh, or
  shares a hidden zsh's session, and a process that is gone, get the `read` scope
  only (`methods.rs`, `granted(surface, peer)`).

### What a call changed

`tools/snapshot.rs` (`CallSnapshots`) uses `efr-snapshot`, efr's own snapshot store in
`$XDG_DATA_HOME/efr/snapshots/`, which the sandbox masks:

- A `shell` call: a snapshot of each root before and after it, and the changes go to
  `tool_call_completed`. The roots are the turn's registered project and `$SCRATCH`
  in every mode, and in `auto` the registered projects that the line names (the
  plan's project roots; that snapshot is taken under the plan lock). Nothing outside
  these roots is snapshotted, git repository or not.
- A `write_file`, `apply_patch` or `edit` call: the tool's own diffs and line counts (the
  `efr_tools::WrittenFile` of each file it changed) go to `tool_call_completed` as
  `changes` and `diff`, in every directory. A created file is `added`, a deleted one
  `deleted` and a moved one `renamed` with the path it came from. The diff holds the
  diff of each file in the order the call changed them, one after the other, with
  each header naming the shown paths (`/dev/null` for the side that is missing). A
  shown path that is absolute keeps its `/` after `a/` and `b/`, such as
  `+++ b//etc/x.conf`, so a client does not show it as a path in the project.
  The approval preview of an `apply_patch` or `edit` call names the same shown
  paths, in its headers and in the `delete` and `move` lines of a patch; the lines of
  each hunk stay as they are.
  Before the call writes into a root, the turn gets its first snapshot of that
  root.
- At the end of a turn (`Toolbox::turn_changes`), the last snapshot of each root, the
  refs `refs/efr/<conversation>/<turn>/pre` and `/post`, and the turn's changes for
  `turn_completed`.
- `conversation.diff` (`methods/conversation_diff.rs`) reads the refs back. Without a
  `turn_id` it takes the newest finished turn of the conversation; without a
  `conversation_id` it takes the named turn's conversation, else the terminal's. A
  turn without snapshots changed no files, so its list is empty.
- An hourly task keeps the refs of the newest `snapshot.keep_turns` turns of each
  conversation and deletes a store without a snapshot for `snapshot.max_age_days`.
  `snapshot.enabled = false` takes no snapshot; a file tool still shows its diff.
- At start, efrd reads the user's own `core.excludesFile` once, because the store's
  git reads no global config.

### Notices

When a turn finishes or fails, or an approval waits, and no subscription from the
conversation's terminal (a connection whose hello named that tty) was handed the event,
nor a live lease from it names the conversation, the daemon appends one line to
`$XDG_RUNTIME_DIR/efr/notices/<tty>` (`docs/storage.md`), which the zsh plugin prints at
its next prompt. The notices decide after the commit, on a task of their own, and
`efr` both subscribes after its prompt, when a quick turn may have ended already, and
exits as soon as it has shown the end, often before that task reads the commit. So
`connections.rs` keeps, behind one lock, the highest sequence number handed to each
subscription, open or ended, per terminal and conversation, and holds a notice while a
client in the terminal may still show its event: a subscription to the conversation
that is open, or a connection that sent it a prompt (counted from the moment
`prompt.send` arrives) and is still open. When the last of them ends, a held notice
whose event no subscription from the terminal was handed goes back to the notices task
and is written; any other is dropped. A terminal whose subscription received a turn's
last event never hears about that turn.

### Features

- `local-pty` (default): hidden shells on PTYs opened in this process through
  `efr_pty::LocalPtyHolder`. Without it, and unless a holder is injected, every shell
  start fails with a clear error. The PTY holder milestone deletes the feature.
- `screen-ghostty`: the libghostty-vt screen backend (needs Zig 0.16.0). With it, the
  screens are ghostty unless `EFR_SCREEN=vt100`; without it, vt100.

- `test-sandbox-fake`: the test seams of the `auto` sandbox in `sandbox/seams.rs`:
  `Deps::with_sandbox_launcher` names the `efr-sbx` to copy, and
  `Deps::with_probe_override` replaces the probe's result. Only `efr-test-daemon`
  turns it on; `just install` refuses an `efrd` whose hidden `--test-seams` says `on`.

`cfg(feature = ...)` appears only in `screens.rs`, `shells.rs` and `sandbox/seams.rs`
(a tidy rule).

## Tier

Tier 4: a binary, the composition root.

## Allowed dependencies

Every library crate except `efr-client` and the test crates: `efr-stdx`,
`efr-protocol`, `efr-store`, `efr-credentials`, `efr-permissions`, `efr-scope`,
`efr-holder`, `efr-http`, `efr-screen`, `efr-provider`, `efr-screen-vt100`,
`efr-screen-ghostty` (optional), `efr-pty` (optional), `efr-shell`, `efr-tools`,
`efr-provider-openai`, `efr-provider-anthropic` (the provider of `anthropic-api`, its
catalog and the key check of a login), `efr-oauth-openai`,
`efr-config`,
`efr-conversation`,
`efr-transport`, `efr-sandbox` (the spec of a sandboxed call, the worktree record,
the probe's report and the plan that `sandbox.explain` reads) and `efr-snapshot` (the
snapshots before and after each call that can write, the turn's changes and
`conversation.diff`). `xtask/src/deps.rs` holds the allowlist; `efr-test-daemon` is its only dev-dependent,
and only from `tests/`.

Third-party crates: `tokio`, `tokio-util` (`CancellationToken`), `async-trait`, `bytes`,
`serde`, `serde_json`, `jiff`, `nix` (`flock`), `base64` (the hello
challenge), `clap` (the flags), `sd-notify` 0.5.0 (`READY=1`, `STOPPING=1`), `rustix`
(inotify, for the config file watcher), `toml_edit` (the values that the settings tool
sets and the one-line text of a rule), `tracing`, `tracing-subscriber` (with its reload
layer for the log filter), `tracing-journald`, `thiserror`, `sha2` (the check of the
sandbox launcher's copy), and `anyhow` in `main.rs` only.

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
- An API key reaches the credential file only; it never enters an event, a receipt,
  a log field or the text of an error.
- Shutdown releases the lock last, after the database is closed.
- A turn, a prompt and `models.list` never wait for a fetch of the model catalog,
  with one exception: while the Anthropic catalog has no list at all, a prompt and a
  manual compaction wait for one fetch.
- efrd names itself to the backend as efr, with efr's own version.
- A daemon with an injected provider never fetches the model catalog.

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
providers, the model catalog (the default, the effective list, the windows that
`[openai] models` raises, lowers or clamps with one warning, the cache read at an
offline start and the fallback to the built-in table, a fetch against `wiremock` with
its tag and a 304, a failed fetch, a list that offers nothing, the task that fetches at
start, after a login and hourly, and a fetch that hangs while a reader goes on; for
Anthropic, the cache file of its own name, no list at a start without one, the default
before a list, a fetch against `wiremock` that goes to the cache, a failed fetch, and a
prompt that waits for one fetch while there is no list and never with one), the
providers that efrd builds for each id with their settings, the tool adapter and its answer to who can answer hidden input. The tests
in `run/tests.rs` start the real daemon
in-process on temporary directories with a manual clock, a seeded generator, vt100
screens, an in-memory database and a scripted model, and talk to it over its socket in
raw frames: a prompt followed to the end of its turn, routing and receipts, refusals
(an answer with nothing to answer among them, its text never repeated), a notice for a terminal that does not follow its conversation and none for one that
followed its turn to the end, with a model that answers at once, nor for a view that
opens after its turn ended and closes right after the last event, and a second daemon
refused by the lock. The tool adapter's tests run shell calls through the real
toolbox and the engine with the defaults and with user rules: read-only commands run,
other commands ask, a named secret is denied, and relative paths resolve where the
hidden shell is. The `e2e_` tests run an approved command in a real hidden zsh and
attach to its PTY, run `ls && cat` there without approval, and let a configured rule
allow `seq 3` but not `seq 4`; they skip with a message unless `EFR_TEST_ZSH=1`:

```sh
EFR_TEST_ZSH=1 cargo nextest run -p efr-daemon e2e_
```

The integration tests are one test binary, `tests/it/main.rs`, so the daemon is
linked once; its modules (`hello`, `subscribe`, `prompt_send`, `shell_tool`,
`approvals`, `interrupt`, `receipts`, `reconcile`, `pty_attach`, `login`,
`input_respond`, `sandbox`, `drafts`, `turn_input`, `catalog`, `anthropic`) run the daemon through `efr-test-daemon`'s `TestDaemon` and replay
its fourteen NDJSON scenarios, each with the assertions of its case: the fake PTY
holder plays the hidden shell, the replay provider or a local Responses server plays
the model. The `shell_` tests run a real zsh and skip with a message unless
`EFR_TEST_ZSH=1`: a command runs and is recorded, a password typed through
`input.respond` reaches only the program (not the model's next request, the event log,
any file of the daemon's tree or any log line at any level), and a password prompt
that no client can answer is stopped within seconds.

The `login` module also runs `admin.login_api_key` and `admin.logout` against the
local server's `/v1/models`: a key that is checked with its organization and project
headers and stored, a refused key with the server's message, a check without an
answer and a store without a check, keys and providers refused before a check, an
Anthropic key whose check fails, a logout and the status of every provider, a new
key for the running provider that brings the model list of that key. With every log
line of the process captured, no key shows in a log line, an error, the status or
any file of the daemon's tree but its credential.

The `catalog` module runs the real subscription provider against the local server's
`/models`: the backend's list applies with its default, windows and tool form, a
restart while the backend is down offers the cached list and asks with its tag, a 304
confirms the cached list, and a prompt completes while a fetch hangs.

The `anthropic` module runs the real Anthropic provider against the local Messages
API (`MessagesServer`): one conversation with an `edit` call and its approval, a second
turn and an auto compaction before its call, where every request keeps the request
before it as its prefix outside the compaction, carries the cache markers S, A, P and
T with their times to live, the `drop_block` beta, the effort `medium` and no member of
another provider, and the turn's usage carries the cache writes; the first prompt that
waits for the model list, which a restart reads back from its own cache file while the
API is down; a prompt without a key or a list that fails as `unauthorized` with the
login and calls no model; a failed fetch with the server's answer; a model with its
output limit in the config that runs without a list; a `[model] name` of OpenAI that fails a prompt with the cause and the fix, and a
warning at start, while a prompt's own model runs;
a login to the running provider that fetches the list with the new key; and a restart
between two turns, after which the next request still starts with the one before it,
the signed thinking of the stored answer unchanged.
No file but the credential and no log line holds the key.

The `turn_input` module holds each turn in a model that waits for the test: a late
steer that becomes a prompt (also for a conversation with no live actor after a
restart), withdraws by terminal and by turn with their refusals, the interrupt of Esc
with a steer and a queued prompt, and a retry of each that answers from its receipt
with the same sequence numbers.

The `drafts` module checks the drafts end to end with a model that streams text: only
a subscriber that asked gets them, each after the events that its `after_seq` names;
the log is the same with and without them; with the test clock following real time,
a delta reaches the client within 35 ms at the 95th percentile (16 ms of draft
interval); a subscriber that attaches during a model call gets the turn's `context`
draft first; and a subscriber on a raw socket that does not read loses drafts but
keeps its subscription and gets every event.

The `sandbox` module runs the `auto` mode end to end with a scripted model: the
probe's failure and the fallback with its reason, a launcher in a write root, a project
at the home directory, the one-command rule, `sandbox.explain`, and, with a real zsh and
a fake launcher (a script that runs the child shell directly), a routine command that
runs contained without a question, an exit that asks and runs with its grant, and a
failed start that runs the probe again; and a double-forked process of an approved
command, which python3 plays, that reaches the socket and may only read. With
`EFR_TEST_SBX_BIN` (`just test-sandbox`) one call runs in the real sandbox. A finished
call's dir is removed. The unit tests of `sandbox/` cover the plan, the
probe with a fake launcher, the lock, the worktree record, the facts, the report, the
quarantine and the peer check on real process trees.

No test uses the network, a real model, real time, the user's home, config or runtime
directory, or the git configuration of the machine.
