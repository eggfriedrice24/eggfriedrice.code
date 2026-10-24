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
   checks its SHA-256 and runs the probe once before the socket opens. The engine is
   built a second time right after that probe, with what the probe found
   (`engine::Probed`: the network view and proxy self-tests, efrd's mount check and
   the pacman mirror hosts), so no
   request sees the engine of before the probe; a reload builds with the same
   `Probed`. The
   conversations read the probe's status from a `watch` of `SandboxStatus`; while it
   says unavailable, an `auto` turn runs as `cautious` with the probe's reason. See
   "The auto sandbox" below.
6. The background tasks: the shells' lifecycle events, the notices (`notices.rs`), the
   idle shell collector (`gc.rs`, which reads `shell.idle_minutes` at each look), the
   reload task (`reload.rs`), the task that follows the probe's status
   (`reload::follow_probes`), the config file watcher (`reload/watcher.rs`) and the
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
  tool call on, or when the always rules of `sandbox.allow` or their disabled marks
  changed (phase E), so the rule of an `a` answer counts from the next call on; and,
  when `[permissions]` changed, the hidden shells' trusted
  programs: a running zsh with the old set restarts in its directory before its next
  command (`efr-shell`);
- `shell.program` and `shell.login` for the hidden shells started from then on;
- `log`, through the reload layer of the tracing filter (`telemetry.rs`). An invalid
  filter is an error of the file;
- `jobs.forward_ports` from true to false: after the settings went out, efrd stops the
  port forwards of every running job, with a `stopped` event for each port
  (`JobTable::forwards_off`). A job that read the old settings and starts later
  forwards nothing; a later `true` does not open the forwards of a running job again.

Everything that can fail runs before the first send, so a refused file changes nothing.

The answer of a reload carries its warnings (`AdminConfigReloadResult::warnings`,
DV44), which went only to the log before: a `[model] name` of another company than
the running `[model] provider` (only when the reload changed what
`Settings::foreign_model` gives, so the same name warns once, C3.1), each always rule
that the new engine disables and the old one did not (`always::warn_disabled` returns
them), and a bus filter that the reload turned off: `dbus.proxy` went to false, or it
went to true while the last bus check of this efrd found the filter off for a reason
that warns (`reload::bus_warning`; the check that a reload to true starts runs after
the answer). A held change is the answer's `error`, as before, never a warning.
`efr config reload` prints each warning as a dim line
(`reload_reply_carries_the_foreign_model_warning`, `unchanged_foreign_model_warns_once`).

The config hold (full mode): a command of a full turn runs as the user and can write
`config.toml`. `full::FullTurns` counts the full work that runs: a turn from its first
full call to its end (`Toolbox::turn_changes`, which every ending of a turn calls),
and a job that a full call started until the job ends (the job table). While it runs,
a reload whose new settings loosen permissions against the running ones
(`loosens::loosening`) is held: the old settings stay, one WARN goes to the log,
each terminal with a recent conversation hears it once, and `admin.status` names the
keys in `ConfigStatus::held`. The text: `config.toml changed while a full turn ran,
and the change loosens permissions.rules. If you did not make this change, take it out
of the file. Then run efr config reload.` The hold is sticky: after the full work
ended, every reload holds again while the file still loosens against the running
settings: the file watcher, SIGHUP, `admin.project_add` and `admin.project_remove`, and
also the settings tool and the writer of an always rule, whose reload reads the whole
file. Only `admin.config_reload` (the user's `efr config reload`) applies a held change,
and only when no full work runs; while it runs, it is refused with `efr does not load a
change that loosens permissions while a full turn or its job runs; run efr config
reload again after it ends`. A reload that loosens nothing applies as before and ends
the hold. A held reload still applies each key that turns full mode off or down
(`reload::full_lowered`: `permissions.allow_full` off, a lower `permissions.mode`,
`undo.enabled` or `snapshot.enabled` off), so the gate goes off at once and the
running full turn stops at its next call (`gate_off_applies_while_a_change_is_held`).
While a change is held, `reload::kept` keeps the running settings in
`held-config.toml` in the state dir; `start` reads it, and while `config.toml` still
loosens against it, efrd starts with the kept settings and the change stays held
(`held_change_stays_held_after_a_restart`). The file goes when no change is held.
The hold works against a line or a script of a full turn; a process that the
turn left behind runs as the user and can call `admin.config_reload` itself.
A reload runs the sandbox probe again when `[sandbox]` changed in any key but `allow`,
or when the new engine has other projects or another support (`reload::probes_again`);
a change of the always rules alone, as an `a` answer makes, starts no probe.

The same task rebuilds the engine after a probe. `reload::follow_probes` watches the
probe's status (`SandboxService::status`, which every probe sends: at start, after a
reload, after a call whose sandbox could not start, before an `auto` prompt and for
`admin.sandbox_check`). When `network.netview` and the last self-test say other than
the running engine's `AutoSupport::netview`, it asks the reload task for a rebuild
(`Reloads::rebuild`). A rebuild builds the engine with the running settings and
`Probed::of` the sandbox, and sends it only when what the sandbox supports changed. It
never reads `config.toml` and never starts a probe, so a probe and a rebuild cannot
start each other in a loop. The reload task is the only sender on the engine watch
after the start, and a reload also builds with `Probed::of`, so a reload of another
key keeps the network view. The proxy (phase D2) follows the same way: a rebuild runs
when `network.mode = "proxy"` and the proxy self-test say other than the engine's
egress, and `reload::follow_mirrors` asks for one when a read of the pacman files found
other mirror hosts. Each engine that efrd sends gives its host list to the proxy
(`ProxyService::set_hosts`), so a reload reaches every running listener at once; a
reload also reads the pacman files again, and an engine whose list differs is sent.

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
  with the login's token (`efr_providers::openai::CatalogClient`): at start, after a
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
  that the key lists (`efr_providers::openai::Catalog::from_api`); windows, efforts and
  tool forms stay those of the table. Without a stored key, efrd waits for a login.
- The Anthropic catalog comes from the API's `GET /v1/models`, page by page, with the
  stored `anthropic-api` key, the `anthropic-version` header and the workspace of
  `[anthropic]` (`efr_providers::anthropic::CatalogClient`), at the same times and with
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
- A fetch with a stored key reports to the key's refusals (`providers/refusals.rs`) as
  `KeySource::ModelList`: a 401 marks the key as refused at that time, and a fetch
  that works clears that mark, never the mark of a model call.
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
  permissions, from `loosens.rs`, which the config hold of a reload reads too: a new
  allow rule, a removed deny or ask rule, a mode toward `full`, the gate
  `permissions.allow_full` turned on, `per_call` to `keep`, a removed secret path,
  and the rows of `[sandbox]`, `[snapshot]`, `undo`, the classifier, `[jobs]`,
  `[network]` (`netview` on, `mode` to `proxy`, an added `allow` entry, a removed
  `deny` entry, a `[[network.hosts]]` entry that no earlier entry covers, a higher
  `max_upload_bytes`) and `[dbus]`; `loosens::loosening` names each such key). The engine asks about it in every mode
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
  whether a restart is needed. While full work runs or a held change waits (the config
  hold, `reload.rs`), a reload that loosens is held, and the result says so.
- The tool refuses `permissions.allow_full` and `permissions.mode = "full"`: the user
  sets them by hand or with `efr config set`.
- The tool never declares the file as a path: config protection still denies every
  tool's write to it, `write_file` and the shell included.

### Methods

`methods.rs` implements `efr_transport::Dispatcher`. It matches `Method` exhaustively
twice, once for the scope and once for the handler, so a new protocol method does not
compile until it has both. One handler per method lives in `methods/<noun_verb>.rs`.
Connections on the Unix socket hold every scope, `admin` included; a phone connection
(`Origin::Phone`, the tailnet listener of a later milestone) holds `read`, `operate` and
`approve`. efrd walks `/proc` for each peer on the Unix socket, also for one that
declares `phone`, so a model-side process holds `read` only, whatever origin it
declares. The tailnet listener must grant the phone scopes by its own pairing.

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
- `prompt.send` routes to the named conversation; for a prompt from a terminal (the
  context's tty, else the hello's) with no conversation id, to the conversation of the
  terminal's slot for the prompt's scope key (`routing.rs` holds the rules,
  `conversations.rs` the slots). The key is the registered project whose root holds
  the prompt's `pwd` (as given, or with its links resolved; the deepest root wins), or
  `outside` for every other place; efrd reads the registry once per prompt and runs no
  git for it. So the conversation follows the scope when a prompt is sent, not on
  `cd`. `new_conversation` (`,new`) starts a new one in that slot; the other slots
  stay. The slot's conversation ends when the prompt comes from another shell while the
  slot's shell has exited (checked first: a new tab that reuses a closed tab's
  `/dev/pts` number starts fresh), or after `conversation.tty_idle_hours` (12 by
  default) without activity; each slot has its own clock. Any event of the
  conversation counts as activity, so a background job that still writes keeps its
  slot alive. Then, for a project key only, the prompt picks up the project's single
  newest conversation of those that a terminal held (a row with a `routed_seq`; one of
  `efr send` with no tty or of the phone never is) when it is idle, was active within
  those hours, and no open terminal holds it (a slot whose shell runs or is not known,
  or an open subscription from a connection with a tty; a phone never holds one). The pick-up needs the
  shell's or `efr`'s connection, never a phone's or a proxy's, whose process runs on
  the claimed terminal: efrd compares field 7 (`tty_nr`) of `/proc/<peer pid>/stat`
  with the device number of the tty (`proc_view.rs`; a test daemon injects a fake).
  The claim and its checks are one lock section, so two new splits never get the same
  conversation; the pick-up records `terminal_attached` in the prompt's batch, and a
  prompt that then fails gives the slot back. Else a new conversation starts. The
  outside key never picks one up. With `conversation.follow_project = false` a prompt
  goes to the terminal's current slot (the one of its last prompt, `,new` or pick-up),
  whatever its key, with no pick-up. A prompt with no terminal starts a conversation of
  its own and holds no slot. The result's `route` names the scope of the prompt and
  the switch (`back`, `picked_up`, or `new` with its reason); the receipt keeps it, so
  a retry answers the same. Each change of the slots follows one recorded event, and
  the store's projection applies the same rule to it, so a restart reads the same
  slots from the `route`, `shell_pid` and `routed_seq` columns. `jobs.stop`,
  `conversation.diff`, `turn.undo` and the presence of a terminal's input take the
  terminal's current slot. `conversations.list` with `tty` lists that terminal's
  conversations, its current one first.
  A prompt from a terminal whose key is `outside` and whose `pwd` is in a git work tree
  that is not a project gets that work tree in `route.hint`, for the line before the
  reply (`repo_hint.rs`, DV53): one git discovery through `efr_scope::derive`, as the
  turn's resolver runs it, so `$HOME` and the directories above it never count, and a
  slow or failed run gives no hint. A work tree gets the hint at most once in a rolling
  day of efrd's clock; efrd records it only after the prompt was sent, and keeps the
  times in `<state root>/repo-hints.json` (0600, by a rename, without the entries older
  than a day), which it reads at start. `conversation.repo_hint = false` turns it off,
  and then efrd runs no git for it.
  A prompt's `settings` (mode, model, effort) are checked by the conversation
  against the latest settings (`settings.rs` gives it the defaults and the model list
  of `catalog.rs`, both from memory): a model outside the list, or an effort the model does not take,
  is `invalid` with the setting, the value and the choices as data, and nothing is
  recorded. The result carries the effective settings; the turn resolves them again
  when it starts, records them on `turn_started` and sends the effort as the request's
  `effort`. A turn from a remote origin runs with at most `cautious`.
- `conversations.recent` (`methods/conversations_recent.rs`) is the list of `,resume`
  and `efr resume`: the conversations whose scope key is the key of `pwd` first, then
  all others, each part newest first by its last activity, 10 by default. Each row has
  its key, its project's name (none outside projects or for a removed project), its
  prompt turns (a job notice turn and a withdrawn prompt do not count), whether the
  asking terminal holds it (`here`) and which other open terminal holds it
  (`open_in`, the rule of a pick-up). The phone sends no `tty`, so it sees every
  holder. `conversation.resume` (`methods/conversation_resume.rs`) moves a
  conversation to the terminal that asks. It needs the shell's or `efr`'s connection
  whose process runs on that terminal, as a pick-up does; a phone or a made-up tty is
  `forbidden`. When another open terminal holds the conversation, it moves nothing and
  answers `held`, unless the params say `take`. The conversation goes into the
  terminal's slot of its own key, not of the user's directory, so a project
  conversation keeps its project; the result says whether a prompt from the user's
  directory continues it. efrd claims the slot (`Conversations::take` for `take`),
  chooses the directory of the hidden shell (the one of the shell that runs, else its
  last directory: the newest shell's directory in the shells projection, else the
  last prompt's, else the user's; the first that exists), records `terminal_attached`
  (`how: resumed`) with the receipt in one batch, completes the claim, and only then
  starts the shell when none runs, so a resume that fails starts nothing. The slot is
  then the terminal's current one, and every other slot that held the conversation
  lets it go; that terminal's next prompt says where it went. A pick-up starts the
  hidden shell in the same directory, but before its prompt is recorded, so the turn
  finds the shell and moves it to the user's directory by its own rule; when the
  prompt fails, the shell ends. A job keeps running, and its notices go
  to the new terminal, because they follow the projection's `tty`. A `held` answer
  keeps no receipt, so the client asks and sends a new command with `take`.
- `conversation.delete` (`methods/conversation_delete.rs`, scope `terminal`) deletes a
  conversation and everything that efr keeps of it. When another open terminal holds
  it (the rule of `held`, as for a resume; the terminal of the connection's hello
  never counts), it deletes nothing and answers `held`, unless the params say
  `force`; that answer keeps no receipt. A running turn, a queued prompt or a
  compaction is `busy` (data `{"busy": "turn"}`) unless the params say `stop`, and an
  undo that runs is `busy` (`{"busy": "undo"}`) also with `stop`: an undo that stops
  half way leaves the project half restored. Then efrd stops the actor, every running
  job of the conversation (`JobTable::stop_conversation`, the user's stop, waiting for
  each end) and the hidden shell with its screen, and records `conversation_deleted`
  with the receipt. After that commit the slots of every terminal let the
  conversation go, the presence table forgets it, and whatever raced the commit (an
  actor, a job, a shell) stops again; its events fail in the store. `deletes.rs`
  removes the rest: the recording writers of its PTYs stop for good
  (`StoreRecording::forget`, which waits for the writer that closes a segment), its
  rows go in writer steps, its snapshot refs and the stores that only it used go
  (`Snapshots::forget_conversation`), then its files: the recordings of its PTYs, each
  `$SCRATCH` dir whose marker names it (`efr_conversation::scratch_dirs`), its sandbox
  state dir, its runtime dir of the hidden shell, its legacy file backups and the undo
  trash of each of its undos, removed without following a link, a dir made writable
  first. A path that stays is logged and named in `not_removed`, and the delete stays
  open: each start of efrd finishes the open deletes (`deletes::finish_pending`) before
  its socket opens. A subscription of the conversation ends after the tombstone.
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
  one request that runs no model, sent once: `efr_providers::openai::check_key` (the
  API's `/models` with the organization and project headers) or
  `efr_providers::anthropic::check_key` (with the base URL and the workspace of
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
  answers whether one was stored and whether the provider is the one of
  `[model] provider` (`active`), whose turns then fail until a new login. A logout
  that deleted a credential records `logout_completed`, with the provider and nothing
  of the credential. After a logout of the subscription, the running provider forgets
  its cached token.
- `admin.status` lists every provider (`openai-subscription`, `openai-api`,
  `anthropic-api`) with its login (`subscription` or `api_key`), the expiry of a
  subscription token, the hint of a key, which one is active and, for a stored key that
  its provider refused, when (`key_refused_at`). When `[model] provider` uses a key
  (`openai-api`, `anthropic-api`), its model calls go through `Watched`
  (`providers/refusals.rs`), and they and the fetches of its model list report each
  answer to `KeyRefusals`, each source into a state of its own (`KeySource`), because
  a key can work on one endpoint and not on the other: a 401 marks the key as refused
  at that time, and a request of the same source that works clears that mark (a
  stream counts as working from its first event). The model calls decide: their mark
  shows, and once a model call worked, the mark of the model list does not. A login
  or a logout of the provider clears both, and an answer to the old key that comes
  after it changes nothing. Each answer of either source, refused or not, also marks
  the key as checked at that time (`key_checked_at`), and so does a login that
  checked the key. The marks live in memory only: after a restart, a stored key has
  neither until its first answer, and `efr status` says `not checked since efrd
  started` (DV32).
- `prompt.send` refuses a prompt that names no model of its own while `[model] name`
  is a model of another company than `[model] provider` (`efr_config::ForeignModel`):
  `invalid`, with the cause and the fix (set `[model] name` to a model of the provider,
  or remove it) and the setting, the value and the provider as data. efrd also warns
  about such a name at start, and after a reload that changed what `foreign_model()`
  gives against the running settings: a reload of the same name warns no more (C3.1).
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
  `settings.rs` hands `[compaction]` to the conversations as `CompactionConfig`.
- `conversation.context` (`methods/conversation_context.rs`, scope `read`) waits for a
  model list as a prompt does, starts the conversation's actor when none runs (a
  conversation that does not exist is `not_found`), and answers with what the actor
  measured: the parts of the next request, the largest items, the newest count, the
  compactions and the cache use. It records nothing and answers while a turn runs. The
  contract is in the README of efr-conversation, section "What fills the context". For
  the fresh context block after a compaction, the toolbox names the children of the
  hidden shell (`Toolbox::shell_children`, `tools/shell_children.rs`): the command
  lines of the shell's child processes from `/proc`, at most 10, each cut at 200
  characters.
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
  with links resolved. After the write, both write the registered projects again to
  `<runtime root>/projects.tsv` for the zsh plugin's scope marker (`projects_file.rs`:
  one `<root>\t<name>` line per project, the deepest root first, 0600, by a rename,
  without a root that holds a tab or a line break). efrd also writes it at start, and
  at a prompt whose registry read differs from it, after a hand edit of
  `projects.toml`. Both reload after the write, so the engine trusts the new set of
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
  the call's command no longer runs, is `not_found`. A command that its call left
  running in the hidden shell at the timeout still takes an answer with that call's id,
  with the same checks (see "A command left running" below); and a command that does
  not wait for that
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
  question) never stops a command. The same count keeps a call running past the
  model's `timeout_seconds` up to `shell.interactive_timeout_minutes` of the latest
  settings (60 by default): a call outside the sandbox whose line may ask at the
  terminal, whoever approved it (`CallContext::may_wait`, which `tools.rs` turns into
  `ToolContext::interactive_limit`), and any call whose command waits for input at its
  timeout (`ToolContext::input_limit`, set for every call). The shell asks once per
  quiet period past the timeout whether such a client still follows. Without one, or
  once the limit passes, the call answers as at its timeout (`Interactive` or
  `StillRunning`, with `left_running` on its `tool_call_completed`), and its command
  goes on in the hidden shell (see "A command left running" below). A visible wait
  whose prompt looks like a password prompt behind a relay carries `looks_secret` on
  its `tool_call_input_changed` event.
- A command left running. A call whose command still runs when the call answers the
  model leaves it in the hidden shell, which takes no other call until it ends.
  `tools.rs` hands each call `input_limit` and the approver of the call
  (`CallContext::approved_by`), and the shell keeps both with the command.
  `shells/observer.rs` records the shell's notices of it as `shell_command_detached`,
  `shell_command_input_changed` (each wait change, with the row of the screen that
  asks) and `shell_command_ended` (`ended`, `stopped`, `limit`, `closed` or
  `shell_exited`), events of the conversation that name the call. The shell's watch
  asks `Connections::answerers` (`Followers` in `shells.rs`) whether a person can
  answer a wait of it: a wait for a password stays while one follows, or, while nobody
  does, only for a line that may ask at the terminal and that the user or the user's
  own rule approved; in the sandbox it never stays. Any wait ends at the input limit
  after the command's start. `input.respond` with the call's id answers it, and the
  shell refuses a hidden answer, an answer to a wait that looks secret and every manual
  answer for a command in the sandbox (`conflict`). `shell.status` (scope `read`,
  `methods/shell_status.rs`) answers the command as a `BusyShell` from the shell's
  live state, never from the log, so the event of an efrd that stopped since never
  shows as busy. `shell.interrupt` (scope `terminal`, so never the phone or a
  model-side peer, `methods/shell_interrupt.rs`) sends `SIGINT` to the command's job
  only while that job holds the terminal and is not the shell itself, or with `close`
  closes the hidden shell; it is `not_found` once the command no longer runs and
  `conflict` when the job does not hold the terminal, and a final refusal gets a
  receipt. A call that finds the shell held fails at once while the command waits for
  input, and before a sandboxed call after its timeout otherwise, and the model reads
  the command's line from `not_ready_text` (`tools.rs` reads it from
  `ShellSessions::busy`). `Toolbox::busy_shell` gives the turn's preamble the same
  command. The notices (`notices.rs`) tell the terminal at its next prompt, when no
  client showed the event: the command left running, a wait that efr keeps for the
  user with the row that asks, the approver and `type the password only after ,shell;
  never at this prompt`, and a stop by efr at the limit or of a password wait that
  nobody could answer.
- `shell.sudo_cache` decides what happens to sudo's credential cache on the hidden
  shell's terminal, read from the latest settings at each call (`tools.rs` sets
  `ToolContext::forget_credentials`). `keep`, the default, leaves it to sudo (about
  five minutes), so a second `sudo` soon after an answered one may not ask again.
  `per_call` makes the hidden shell forget the credentials (`sudo -k`, `doas -L`) after
  each call, before anything else runs there, so the next `sudo` asks for the password
  again. It needs the zsh integration: a hidden shell without it keeps the cache, and
  a call inside a nested shell forgets only when that shell exits. Every command with `sudo` still needs the user's approval with either value.
  A call of full mode forgets after the call whatever the setting says, and before
  its line too (`ToolContext::forget_before`): a cache from an earlier `keep` call or
  from the user's own terminal (`timestamp_type=global` in sudoers) is gone before the
  line runs. A hidden shell without the integration does not run such a call.

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
  The surface guard scans the write roots, scratch, and the real dir of each
  `Grant::Write` outside them, or the dir that holds `.git` for a grant in a git dir
  (DE41), so an approved write to a repository that is no project cannot plant its git
  hooks unseen. The engine gets `$SSH_AUTH_SOCK` and the socket of `$GPG_AGENT_INFO`
  from the hidden shells' environment as credential sockets
  (`HostFacts::agent_sockets`), so only the user approves one.
- The desktop socket of a `desktop` grant (`sandbox/desktop.rs`): `prompt.send` keeps
  the six desktop variables of the prompt's terminal (`ShellContext.desktop`) for its
  conversation; a call takes them, or efrd's own start environment when the prompt
  sent none (`HostFacts::desktop`). efrd computes the one control socket from the
  variable (`efr_sandbox::desktop_socket_path`), never scans `$XDG_RUNTIME_DIR`, and
  checks it with `lstat` at each call: a socket of the user (an X server's may be
  root's), each dir on the way a real dir of the user. No such socket, or an X server
  that needs a cookie (`$XAUTHORITY` in the prompt's terminal, which only
  `DesktopEnv::has_xauthority` says, or in efrd's start environment, or
  `~/.Xauthority`), refuses the call with
  `SandboxError::NoDesktop`, and no probe runs again.
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
- The mount view (`sandbox/mounts.rs`): for a contained call or job whose line runs
  `df` or `findmnt` by bare name where the shell itself finds the program
  (`efr_permissions::exits::mount_view_line`), `prepare` also writes
  `mounts` (efrd's own `/proc/self/mountinfo`, verbatim) and `mounts.json` (the sizes
  of each mount) into the call dir. efrd measures the sizes outside the sandbox with
  one `efr-sbx statvfs` process from the launcher's copy, beside the plan lock and the
  plan, never in its own threads: a FUSE or NFS mount that hangs holds a thread that
  only SIGKILL stops. The deadline is 1 s; then efrd kills the helper, hands it to a
  reaper task and does not wait for its exit. At most one helper lives at a time; a
  mount that missed the deadline gets no sizes (`timeout`) and is not measured again
  for 60 s; a snapshot serves every call for 2 s. No sizes for the dummy types, for
  another user's runtime dir and FUSE mounts, and for a mount at or below a mask of
  the call. A snapshot that fails writes nothing and adds a note. The view needs
  efrd in the user's mount namespace: at start efrd compares its own with its
  parent's (`ns/mnt`, else the mount ids of `mountinfo`, which every process may read;
  the systemd user manager's link does not read), and with another namespace
  `mount_view()` is false and `df` stays an `outside` exit.
- Before the engine decides a shell call of an `auto` or a `full` turn
  (`CallContext::predicts`), the toolbox collects its facts (`sandbox/facts.rs`). A
  full call collects them also when the probe says that the sandbox is not available:
  efrd always builds the sandbox service, and the facts need no sandbox. The facts
  are: what each target is, the tracked files below each `rm -r` directory through
  the hardened `git ls-files`, and where each program word leads and whether it
  changed in the turn. A word that the shell runs itself
  (`FactRequest::builtins`) leads to `builtin`, never to a file of its name on the
  `PATH`. For the always-allow rules (phase E), each path of `FactRequest::real_paths`
  gets its real path after every link (`CallFacts::real_paths`): for a path that does
  not exist, the real path of its nearest existing parent with the rest joined. A path
  with a link that leads nowhere on its way gets no fact, so no rule covers it. Each
  socket of `FactRequest::sockets` that an ssh process listens on goes to
  `CallFacts::ssh_sockets` (`sandbox/ssh.rs`, v0.0.5 OQS17): efrd reads the socket
  inodes of each process named `ssh` and the listening sockets of its
  `/proc/<pid>/net/unix`, and never connects to a socket; when it cannot read `/proc`,
  every socket of the request counts. At start, `sandbox/ssh.rs` also reads the
  `ControlPath` patterns of `~/.ssh/config`, `/etc/ssh/ssh_config` and their `Include`
  files into `HostFacts::ssh_control_paths`, which the engine gets through
  `Locations::with_ssh_control_path`.
- After a call, the quarantine question's "keep" moves the changes back
  (`sandbox/quarantine.rs`), and at the end of the turn the report lists the files
  that run code later outside the sandbox (`sandbox/report.rs`, from the hardened
  `git status` before the turn's first launcher call and at its end).
- An hourly task deletes cache layers idle for `sandbox.cache_days` and the oldest
  above `sandbox.cache_max_gib` (`sandbox/gc.rs`), never those of a running call.
- Socket peers (`sandbox/peers.rs`): a process that descends from a hidden zsh, or
  shares a hidden zsh's session, and a process that is gone, get the `read` scope
  only (`methods.rs`, `granted(surface, peer)`). Under the systemd user unit, a peer
  whose cgroup is efrd's unit cgroup or below it and that is not efrd gets `read` too
  (DV49; below it since the job cgroups of ADR 0021, `<unit>/daemon` and
  `<unit>/jobs/...`): an orphan of
  an exit child job that called `setsid` and lost its launcher leaves the job's session
  and tree, but not the unit's cgroup. `Deps::from_process` reads efrd's cgroup once
  (`peers::unit_cgroup`, the `0::` line of `/proc/self/cgroup` when it ends in
  `/efrd.service`); a hand-started efrd and a test daemon have none, and the rule is
  off. The rule only reduces a peer that the walk cleared.
- The network proxy (phase D2, `sandbox/proxy.rs`, the tokio tasks of `efr-proxy`).
  The engine's egress is the proxy only with `network.mode = "proxy"` and a probe
  whose proxy self-test passed (`Probed::proxy`); else a network need stays a review
  that opens the whole network, never a proxy with no route. Then efrd logs one
  warning, `admin.status` lists the reason in the sandbox warnings, and the first
  contained call after each start gets a note. The plan (`sandbox/plan.rs`) gives a
  contained call `NetworkPlan::Proxy` with its own socket `$CALL/proxy.sock`, except
  with `Grant::OpenNetwork` (the host's network) or a network view line with no host
  grant (D1). `prepare` binds the socket after it wrote the call dir, through
  `/proc/self/fd/<dirfd>/proxy.sock` with an `O_PATH` descriptor of the dir (the path
  can be longer than a socket address holds), mode 0600, and serves it with the
  call's frozen policy: its `Grant::Host` entries, `network.max_upload_bytes`, and the
  hosts that a file of the call's projects holds a credential for (`proxy/tokens.rs`:
  the registry files of each project root and the call's dir, and one hardened
  `git config` run per repository; the call's output says why). A credential helper
  denies the host of its scope, or every remote when it has none (v0.0.4). gh's helper
  denies only when the call can reach gh's token (`HelperFacts::gh_open`): when the
  call's masks do not hide the `hosts.yml` of gh's config dir (from the hidden shell's
  `env` file, else the newest `env` file of any hidden shell, else unknown, which
  denies), when `sandbox.env_keep` keeps a GitHub token variable, or when the call has
  the whole session bus. The proxy refuses a denied host with the reason `denied`,
  which `refused_hosts` leaves out. A job's start outcome carries a `SandboxSummary`
  with `proxied`, so the turn's trace names its network. A foreground call
  closes its socket after its launcher wrote the result (a grace of 200 ms for open
  tunnels) and merges the refusals into `sandbox.blocked` and the model's output; a
  call past its timeout keeps its socket until `result.json` appears or its dir goes
  (`gc::sweep_proxies`, at the hourly gc pass and the next proxy call's prepare); a job
  keeps its socket in its entry of the job table and closes it at its end, before its
  dirs go, with the refusals in `job_ended` and its notice. Dropping a socket's handle
  aborts its tasks, on every path. The pacman mirror hosts of `@pacman-mirrors` are
  read at start, at each reload, and at a proxy call's prepare when a file changed, at
  most once a minute (`proxy/mirrors.rs`). `admin.status` says `network_mode: proxy`
  when the proxy runs, and warns about each `sandbox.env_keep` entry that looks like a
  secret while `network.mode = "proxy"`. The facts of a line carry the git remotes of
  each directory of a remote git command (`sandbox/facts/remotes.rs`: one hardened
  `git config` run per repository, one more over a `.gitmodules`; a remote of a pinned
  project git dir is the project's own while its config files have not changed since
  the conversation's first fact request and its URLs equal the first read) and the
  hosts that the proxy refused in the conversation's last call and its jobs. For a
  line with a `FactRequest::git_code` ask, the facts carry its git code (v0.0.4, B4):
  `tools/git_code.rs` first runs the surface guard of every running contained job,
  then `sandbox/facts/git_code.rs` asks git in hardened runs (no ceiling, with the git
  variables of the hidden shell's `env` file, else efrd's start environment) for the
  git dir, the common dir, the work tree and the config with its include files (an
  include file that git did not read, such as an `includeIf` whose condition it did not
  take, gets a run of its own), lists the hooks, the code keys (also the program keys
  of `sendemail`) and the programs that
  they start (resolved with the hidden shell's `PATH`), and walks the module git dirs
  for a line that can reach the submodules. A part is writable below a `$SCRATCH`, a
  write rule, a config write root, a running contained job's or a timed-out call's
  write root, or in a registered project outside the git floors that its plans pin. A
  run that fails or passes 200 ms gives `checked = false`. The toolbox keeps the check
  of each call; right before an exit child starts, under the plan lock, it checks
  again, and a change of the writable parts stops the call with `GIT_CODE_CHANGED`.
  Before every other foreground upload in the exit child it runs every job's guard. In a
  build with the test seams, `Seams::proxy_deps` gives the proxy a table of test names
  (`Deps::with_proxy_hosts`, `EFR_TEST_PROXY_HOSTS` for the smoke's `efrd`), or refuses
  every name and address with no table, and the pacman files are `/dev/null`.
- The filtered bus (phase E, `sandbox/bus.rs`). A call whose launch holds
  `Grant::BusFilter { bus }` gets one `xdg-dbus-proxy` for that bus, started by
  `BusService::start` after the call dir and before the launcher: the trusted program
  of `trusted_programs.rs` (its inode checked again at each start), run through the
  launcher's copy as `efr-sbx dbus-proxy --dir $CALL/bus --upstream <bus> -- <program>
  --fd=3 unix:path=<bus> <bus>.sock --filter --log --call=...`, with a clean
  environment, the sync pipe as stdout and a pipe as stderr. `$CALL/bus/` is 0700, a
  stale socket goes first, and the socket name is relative. One byte `x` within 2 s
  (on the injected clock) means ready; else the call fails with a setup error and
  never runs with the real bus. The rules are the defaults of `efr_permissions::bus`
  plus `[[dbus.allow]]`, which reaches the next call after a reload
  (`bus::follow_settings`). The upstream is a bus that `BusUpstream::check` accepts:
  resolved, a socket, never in efr's dirs or a write root. Before the spec is written,
  efrd drops a view of a bus that the machine lacks and adds the note `[efr: no
  session bus on this machine; the bus filter did not run.]` (or the system one). The
  proxies live in `Prepared::bus`: a foreground call finishes them after its result,
  a call past its timeout leaves them to `BusService::sweep` (the gc pass and the next
  bus call's prepare), and a job keeps them in its table entry until it ends. A proxy
  ends at once when its sync pipe closes; one that ended before (a crash) is reported
  as stopped. Each proxy logs `phase=bus_proxy call=<id> bus=<bus> ready_ms=N` at its
  end and keeps at most 256 KiB of its stderr for the refusals. `BusCall::finish`
  reads the refused calls from that `--log` text (`sandbox/bus/log.rs`): a `*DENIED*`
  or `*HIDDEN*` line pairs only with the call line just before it (the `C<n>` of a
  line is the client's own serial, and each client starts at `C1`), signal lines and
  their verdicts are skipped, a bus driver refusal (`Filtering message due to arg0`)
  counts as a refusal of that name with no member, and a name, member or path that
  is not D-Bus grammar becomes `<invalid>`. At most 16 entries, one per bus, name,
  member and path with a count, go to `SandboxSummary.bus_refused` of a call or
  `JobEnded.bus_refused` of a job, and the rest to `bus_refused_more`. The model gets
  `efr_tools::bus_note` in the tool result or the job's notice: the refused calls as
  data and how to ask for the whole bus (`needs.bus`), and `the bus filter stopped
  during the command` when a proxy died. In a build with the
  test seams both buses come only from `Deps::with_system_bus` and
  `Deps::with_session_bus` (`EFR_TEST_SYSTEM_BUS` and `EFR_TEST_SESSION_BUS` for the
  smoke's `efrd`), for the filter and for a whole bus grant alike: with no seam the
  system bus is a path that never exists and there is no session bus.
  `Deps::with_bus_proxy_program` names a test's fake program. While `dbus.proxy` is
  true, each probe also runs the filter's self-test (`BusService::self_test`, in a
  fresh dir below `$R/sbx/bus-test`): `xdg-dbus-proxy --version` must say 0.1.9 or
  later (older versions cannot serve sd-bus clients or let messages past the filter), and one proxy with the default system rules and an upstream that no client
  uses must become ready and end cleanly when its sync pipe closes. No self-test runs
  while `dbus.proxy` is false, and a seam's probe sets the result instead.
  `SandboxStatus.bus_proxy` carries the result into `Probed::bus_proxy`, and the engine
  gets `AutoSupport::bus_proxy` only with `dbus.proxy` true too. `bus::follow` runs the
  self-test when a reload turns `dbus.proxy` on, and asks the reload task for a new
  engine when the filter's state differs from the engine's. `admin.status` says
  `bus_proxy_off` (`config`, `no_program`, `self_test`, `old_version`) and warns when
  `dbus.proxy` is true but the filter is off. While the check after such a reload
  runs, the status still holds `config`; `SandboxService::reported` then gives the
  state of the last check that ran with `dbus.proxy` on (`bus_tested`), or `off` with
  no reason when none ran yet (DV40), so `efr status` never says `dbus.proxy = false`
  for a filter that is being checked. The built-in session rules also hold the calls
  of `notify-send` (v0.0.4, DV18).

### Background jobs

A `shell` call with `background: true` starts a job (efr's auto spec, section 3.18).
`jobs.rs` holds the job table; `tools/job_tools.rs` starts a job and answers the tools
`jobs` and `job_stop`; `methods/jobs_list.rs` and `methods/jobs_stop.rs` answer
`jobs.list` and `jobs.stop`.

- Modes: a job starts in every mode. In `auto` its line goes through the exits like any
  line and runs contained, or in the exit child for an approved exit. Outside `auto`
  `DaemonToolbox::requirements` hands the engine the call with
  `Requirements::background`, so the engine asks the user before each job, and an
  approved job runs in the exit child. `jobs.enabled = false` refuses every new job.
- The start (`DaemonToolbox::start_job`): the limits (`jobs.max_running` per
  conversation, `jobs.max_running_daemon` in all; a job that starts holds its place from
  the check on, through `jobs::Slot`; the refusal lists the running jobs), the time
  limit (the default `jobs.default_minutes`, cut to `jobs.max_minutes`), the watch
  pattern (a regular expression of at most 200 characters, with a size limit), the
  hidden shell's environment (the `env` file that the zsh integration writes at each
  prompt; with no shell, efrd opens one and waits for its first prompt; a shell with no
  file gets one line that makes its next prompt write it), the job's number (one more
  than the highest that the store or the table knows, so an id never repeats), and
  `SandboxService::prepare_job`: the call dir `$R/sbx/<conversation>/jobs/<call>` with
  `line` and `env`, the job plan in the spec, and the cache layers in
  `$SBX/jobs/<call>`, so a job never holds the layer lock of a foreground call. From
  there on the start runs in a task of its own, so an interrupt that drops the call
  cannot cut it in half. Under the plan lock: the guard check of the running contained
  jobs whose roots meet the new job's, the snapshot before, the check of the covered
  asks, the spawn, the start byte and the wait for `started`. Then the snapshot after
  with the roots marked not complete, a second snapshot of the job's roots that the job
  keeps, and `job_started`, recorded before the call answers. The home copies of the
  call get no "after": the end of the turn names them `(a job wrote it)`, and a root of
  the turn `(job j3 runs)` while the job runs.
- `SandboxService::prepare` makes `$SCRATCH/jobs` (0700, with the marker
  `.efr-jobs`, below the descriptor of `$SCRATCH`) before every contained call, so no
  command can make it first; an entry of that name without the marker goes aside to
  `jobs.moved-<n>`.
- The env file of a job in the exit child gets efrd's own root variables that are set
  (`jobs/launch.rs`, `root_vars` and `with_roots`: `EFR_HOME`, `EFR_RUNTIME_DIR`,
  `EFR_DATA_DIR`, `EFR_CONFIG_DIR`, `EFR_STATE_DIR`, from `Deps::shell_env`), in place of
  any that the hidden shell held, so an `efr` in the job reaches this efrd (DV47). A
  contained job's env file stays as it is. The launcher sets `SCRATCH` to the spec's
  scratch dir in every job, whatever the env file holds.
- The spawn (`jobs/launch.rs`): `efr-sbx run --job` with efrd's own `PATH`, `HOME` and
  `XDG_RUNTIME_DIR`, the job's start dir as its working directory, the lifeline pipe as
  stdin (its write end only in efrd, close-on-exec), the output pipe as stdout and
  `$CALL/launcher.log` as stderr; no process group, so the launcher's `setsid` works.
  The launcher's pid joins the model-side list before efrd writes the start byte.
- The output (`jobs/output.rs`): a reader thread per job writes
  `$SCRATCH/jobs/<id>.out` below the descriptor of `$SCRATCH/jobs` with
  `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`: the first 1 MiB, then the rest; past 16 MiB
  more, a new file with the head, `[N bytes omitted]` and the last 8 MiB replaces it by
  a rename. It keeps the last 8 KiB for the end, and matches lines of at most 4 KiB with
  the watch pattern. A write that fails drops the file, never the pipe, and the end says
  `output_truncated`.
- Job cgroups (`jobs/cgroup.rs`, ADR 0021): with `Delegate=yes` in its unit, efrd
  checks the `user.delegate` mark of its own cgroup at start (`Cgroups::setup`, only
  when `Deps::from_process` built the deps), moves itself into `<unit>/daemon`, gives
  `<unit>` and `<unit>/jobs` the `memory`, `cpu` and `pids` controllers, and removes
  what the last efrd left in `<unit>/jobs` (`JobTable::recover`). `JobTable::spawn`
  makes `<unit>/jobs/<conversation>-j<n>` and moves the launcher in before the start
  byte; `JobTable::adopt` moves a moved call's shell and every process below it.
  `jobs.list` reads `memory.current` and `cpu.stat` of each running job. Without a
  delegated subtree, or when a step fails, the job has `JobCgroup::off` with the
  reason and ends by its session only.
- The task of each job runs under the table's own cancel token, never in efrd's
  `background` tasks. It waits for the launcher, a stop (`job_stop` for the model,
  `jobs.stop` for the user) or the time limit on efrd's clock. A stop sends SIGCONT and
  SIGTERM through the launcher's pidfd and SIGKILL after 6 s; every end sweeps the job's
  session (`jobs/sweep.rs`: SIGKILL through a pidfd to each process whose session is the
  launcher's pid, never efrd's own; the pidfd's process must still have the session and
  start time that the scan found), and kills the job's cgroup (`jobs/cgroup.rs`,
  ADR 0021: `cgroup.kill`, else SIGKILL to each pid of `cgroup.procs`; it keeps the
  most memory and the CPU time in `JobInfo::cgroup` and removes the dir). Then it reads the rest of the output and
  `result.json`, records `job_ended` (with the processes that may still run and whether
  the launcher was lost), hands the conversation's actor the notice, and removes the
  job's dirs. A job that the model stopped, or that efrd stopped because full mode went
  off (`full_off`), gets a notice that wakes no turn; every other end, also a stop by
  the user, wakes the conversation.
- `JobTable::stop_full` asks every running job that a full call started to stop with
  `full_off`, and only those, with no wait. The reload calls it after `install`
  (`reload::follow_full_gate`), when the installed settings refuse full mode, and marks
  full mode off in `FullTurns` first: a full job that enters the table after the look
  sees the mark in `JobTable::begin` and stops at once.
  A session that a process still has stays model-side; the table looks again every 10 s.
- Socket peers: `methods::model_side_roots` adds each running launcher and each such
  session to the hidden shells, so a process of a job holds `read` only, also after its
  launcher is gone.
- At efrd's stop, after the background tasks and before the actors, the table stops
  every job with `daemon_stop` and records each end with no notice. At start, before the
  socket opens, each job that the last efrd left running gets `job_ended` with
  `daemon_restart`: efrd kills its launcher only when the pid's start time matches the
  `job_started` event. It sweeps the session only while it is still the job's: the
  launcher runs with that start time, or it is gone and the boot time with its start
  says the same boot (`sweep::sweep_left`); then only processes that started at or after
  the launcher count. A session that a process outlived stays model-side. Then efrd
  reads a `result.json` that the launcher left, and removes every job dir. `recover`
  returns each `job_ended` that it recorded, and `run.rs` writes its notice line to the
  terminal of its conversation (`jobs::restart_notices`, as `reconcile` writes the line
  of a prompt that did not run): the notices task starts later and never sees those
  events. Then each actor gets the notices of the last day that no turn took, which
  never start a turn of their own.
- The job's files (`jobs.rs`, `efr-snapshot`'s `Snapshots::after_job`): at the job's
  end, before the job leaves the table, the snapshot of its roots right after its
  start is compared with the roots now, and what the job changed joins the turn that
  started it, without the paths that another turn or an undo changed since the job's
  start. Snapshots of `$SCRATCH` leave `jobs` out (`tools/snapshot.rs`), and the hourly
  snapshot collector keeps the turn of each running job.
- Undo (`jobs/undo.rs`, `Toolbox::job_blocks_undo`): `turn.undo` is `busy` while a job
  of the conversation runs (`job j3 runs; stop it first (efr jobs stop j3)`), or a job
  of another conversation can write in a root of a turn that the undo may act on (the
  named turn and a half-done undo; every turn without a name or for a redo): a contained
  job whose write roots hold a root of the turn or lie in one, and every exit child job
  (`a job of another conversation writes here; stop it first (efr jobs list --all)`).
- The guard while a job runs (`jobs/guard.rs`): `efr-sbx guard-check` for each running
  contained job every 5 minutes on efrd's clock, and before each contained call whose
  guard roots overlap the job's, before its launcher records the surface (it costs a few
  milliseconds). A break moves the entries to the job's quarantine, stops the job with
  `guard`, and its changes go into `job_ended`, also when the launcher's own end guard
  no longer sees them. At start, a contained job of the last efrd with no
  `result.json` gets the check before its dirs go.
- Programs that a job wrote (`jobs/since.rs`, `sandbox/facts.rs`): a program counts as
  changed in a turn from the earliest of the turn's start and the start of each job of
  the conversation that ran since the previous turn started, by its modification time
  (mtime) or its change time (ctime). The previous turn's start comes from one query of
  the store (`conversations::previous_turn_start`). A turn that the store does not know
  has no previous turn, so every job of the conversation counts.
- The daemon-wide limit of wake turns (`jobs/wakes.rs`): `Toolbox::wake_allowed` counts
  the wake turns of every conversation in the last hour against
  `jobs.max_wakes_per_hour_daemon`, seeded at start from the store's wake turns.
- Retention (`jobs/retention.rs`): the hourly snapshot collector removes a job's output
  7 days after the job's end, and keeps at most 20 outputs of ended jobs per
  conversation; only a `j<n>.out` in a `jobs` dir, and a link as a link.
- Port forwards (`jobs/forward.rs`, `jobs/forward/serve.rs`, v0.0.4 live items DL1-DL16):
  a job that runs contained in `auto` with a network namespace of its own
  (`network.mode = "none"` or the proxy, no grant of the whole network, no network
  view) gets a `ForwardPlan` in its job plan while `jobs.forward_ports` is true when it
  starts (`forward::plan_of`), and efrd binds `$CALL/forward.sock` (0600, through
  `/proc/self/fd` as the proxy socket, `sandbox/proxy/bind.rs`) before the launcher
  starts; the start result gets one sentence for the model. The job's entry keeps a
  `JobForward` until the job ends. Its first connection must send `Hello`; when none
  came in 5 s on efrd's clock, efrd logs one warning and waits on until the job's
  forwards end, and a first frame that is no `Hello` leaves the job with no forwards
  (`late_hello_still_forwards`); only that connection reports
  `Listen` and `Gone`, every later one is a data connection whose first frame must be
  the `Data` of a waiting `Open`. efrd decodes each frame with
  `efr_sandbox::Frame::decode`; a bad frame or another kind closes the forwarder, and
  the job runs on with no forwards. efrd checks each `Listen` again, as it trusts no
  report from the job: below 1024, the bridge's port of a proxy job and the service
  ports are `refused`; a job forwards at most 8 ports and efrd 32 (`forward::Forwards`,
  `limit`); a port that another job holds (`in_use`, with `by_job` in the same
  conversation), a `LISTEN` row on `::1` or `::` of the host, or a failed bind is
  `in_use`. Each outcome is one `job_port_forwarded` in the conversation; a refusal of
  a port comes once for each job, at most 32 in all. A forwarded port listens on
  `127.0.0.1:<port>` through `sandbox::seams::HostPorts` (no `SO_REUSEPORT`). Each
  host connection passes the peer check first: the uid of the peer's row in the host's
  `/proc/net/tcp` must be efrd's own (`efr_sandbox::peer_uid`); else it closes, with one
  warning for each job and port. Then efrd sends `Open`, waits 5 s for the `Data`, and
  copies bytes with `copy_bidirectional`, at most 64 connections for each job. A `Gone`
  keeps the port for the job, and a connection then closes at once. The listeners and
  connections close when the job ends (its `JobInfo` ends with no forwards), at efrd's
  stop, and at a reload that turns forwards off. `jobs.list` shows the forwarded ports
  of a running job from the table.
- What the model learns of each forward (v0.0.4 L2): before efrd records a
  `job_port_forwarded`, `Forwards::tell` keeps the outcome of the port for the job and
  queues a note for the model of the job's conversation, unless the port's last
  outcome was the same, so the model reads each outcome once (a port that is gone and
  back makes no new note). `DaemonToolbox::take_notes` gives the turn the queued notes
  before each model call, also in the middle of a turn, and the turn sends them in one
  message after `efr_conversation::NOT_USER_NOTE`. A note never starts a turn. The
  texts (`forward::note`): `efr: job j1 listens on 127.0.0.1:8765; forwarded: the user
  can open http://localhost:8765, and your contained calls reach it at
  127.0.0.1:8765.`, `efr: job j1 listens on 127.0.0.1:8767; not forwarded: <why>. The
  user cannot open it in a browser, and your calls do not reach it; choose another
  port.` (for the limit: `a job keeps its forwarded ports until it ends; stop a job that
  you do not need.`), and `efr: job j1: port 8765 is no longer forwarded (<reason>).
  The user cannot open it now, and your contained calls do not reach it.` At most 64
  notes of a conversation wait; a new one drops the oldest. The outcomes and the
  waiting notes of a job go when the job ends or its forwarder fails. A reload that
  turns forwards off keeps them (`Forwards::release_ports`), and the control task does
  not close a second time, so the model reads the `stopped` notes
  (`reload_off_note_reaches_the_model`). The `jobs` tool shows the outcome of each port
  of a running job from the same table (`JobTable::line`: `ports 8765 ->
  http://localhost:8765; 8767 not forwarded: 127.0.0.1:8767 is in use on this
  machine`), and the start result says that a note follows.
- The reach of a contained call (`jobs/forward/reach.rs`, v0.0.4 shell items
  DS34-DS48): a contained foreground call with a network namespace of its own reaches
  the forwarded ports of its own conversation's running jobs at `127.0.0.1:<port>` and
  `[::1]:<port>` in its namespace, with no question. `forward::Forwards` knows which
  job serves each forwarded port (after the host bind), since when each job forwards,
  and a `watch` that changes at each forward and each release. While
  `jobs.forward_ports` is true and a job of the conversation forwards, the prepare
  (`SandboxService::prepare`, `PrepareInput::reach`) puts a `ReachPlan` with the
  conversation's forwarded ports in the spec (`reach::plan_of`) and binds
  `$CALL/reach.sock` (0600) before the launch; a conversation with no such job makes no
  socket. The reach helper's first connection must send `Hello` with `REACH_VERSION`;
  when none came in 5 s on efrd's clock, efrd logs one warning and the call runs with no
  reach. On that control connection efrd sends `Reach` for each new forwarded port and
  `Unreach` for each port that no job forwards any more (the job ended, a release, a
  reload that turns forwards off), each on both addresses, and only logs the helper's
  `Bound` frames at `debug`; any other frame closes it. Every other connection must
  begin with `Connect`: efrd finds the job by the call's conversation and the port on
  every connection (`Forwards::route`), takes one place of the job's 64, sends `Open`
  on the job's control connection and copies the bytes (`Ctx::reach`); it never opens a
  TCP connection on the host for a call. A port of another conversation, a port that no
  job forwards, a gone server and a server that does not answer close the connection
  with no byte. A call has at most 32 connections at one time, so the host keeps room
  in the job's limit. The `ReachCall` lives beside the call's proxy socket: a call that
  ended drops it, and a call past its timeout keeps it until its `result.json`
  (`SandboxService::keep_reach`; the gc pass and the next prepare of a call with a reach
  end it). The fact `CallFacts::reach_ports` of a shell line holds the conversation's
  forwarded ports (`reach::fact`); when the line names a loopback port that is not
  forwarded yet and a job of the conversation began to forward less than 3 s ago, efrd
  first waits for that port's outcome (a forward, or an outcome that `Forwards::tell`
  kept, which changes the `watch` too) up to the rest of the 3 s on its clock. A named
  port that a job of the conversation listens on and efrd does not forward (`in_use`,
  `refused`, `limit`, or `stopped` until the job ends) gives `LineReach::refusal`, from
  `Forwards::unreached` and `forward::refusal` (the newest such job of the same
  conversation; a forward of the port by another job of the conversation wins). In an
  `auto` turn `DaemonToolbox::facts` returns it as the error of `requirements`: the call
  does not run, the engine never sees it, so no exit and no question, and the model
  reads `efr: port 8767 of job j1 is not forwarded: ... efr did not run this call. ...`.
  Full mode reads the same fact and keeps its behaviour, and the other modes collect no
  facts.
- A call moved into a job (`call.background`, Ctrl+B in `efr`; v0.0.4 shell items
  DS15-DS33 and DS53-DS56, with the user's change of OQS2). The tool path registers
  each foreground `shell` call while it runs (`jobs/moves.rs`, `Moves::register`, from
  `tools/moves.rs`), with the requirements and facts that the turn judged
  (`DaemonToolbox::requirements` keeps them per call until the call answers). The
  handler (`methods/call_background.rs`, scope `terminal`, so never the phone or a
  model-side peer) refuses with `invalid` and the reason alone as the message: the call
  does not run, `jobs.enabled` is false, no free place, the hard `interactive` flag, a
  nested shell, a contained call whose plan holds the cache layer lock (an existing
  cache with a private layer: the `overlay` mode, or `tmp` with `keep_downloads`), or a
  job decision of the line (`moves::check`: the engine of the key press with
  `Requirements::background` and the default time limit) that denies it (`this cannot
  run as a job` for `NotAsJob`). A contained call moves at once. A call outside the
  sandbox (the exit child, a line typed into the hidden shell, full mode) first gets a
  `preview` with a token, the line, the limit and one `ExitInfo` with the kinds and the
  reasons of the job decision; only a confirm with that token within 60 s and the same
  exits moves it. The move: efrd reserves the place, puts it with the receiver of the
  adopted run in the call's entry and calls `ShellSessions::request_move`; a refusal of
  efr-shell (the command did not start, a sentinel run, a shell or REPL, an input wait,
  a full-screen program) frees the place. The run answers its call with
  `Completion::Moved`; the tool path, which still holds the call's proxy, bus, reach
  and snapshot, takes the adopted run. It makes `$CALL/moved` (`O_CREAT | O_EXCL`,
  0600) for a call through the launcher only then, so a launcher that ended before the
  hand-over keeps the shell state of a call that did not move. Then the reach helper
  gets `Unreach` for every port and the reach ends (`ReachCall::unreach_all`), the git
  roots of a contained call stay counted until its `result.json`, and
  `JobTable::adopt` records `job_started`
  (with `moved_from`, the shell's pid and start time as the launcher's) before the
  call answers with the output so far and `job_moved_text`, with no exit code. The
  handler waits up to 10 s on efrd's clock for the job and answers `moved`. The old PTY
  is marked adopted at the hand-over, whatever the handler does: efr-shell's
  `ShellNotice::Adopted` reaches the shells' observer, which calls `Ptys::adopt`
  (`pty.write` and `pty.resize` with it get `not_found`, the idle collector skips it).
- A command left running moves too (v0.0.5, OQS16, DS30). A call that answers with
  `left_running` and no move keeps its registration as a left call
  (`Registration::finish` with `LeftParts`, the guard and write roots of its job), at
  most one for each conversation. `call.background` with that call's id, when no
  running call has the id, checks the live state first (`ShellSessions::busy` must name
  the call, else `the command ended before it moved`, and the left call goes; a wait
  is `the command waits for input; answer it first`), then the same `moves::check`,
  place, preview and token (`Moves::preview_left`, `answered_left`). The handler takes
  the left call, asks `ShellSessions::request_left_move`, puts the left call back when
  efr-shell refuses, and makes the job itself with `moves::into_job`: `$CALL/moved`
  first, the call's proxy socket and bus filters from the lists of calls past their
  timeout (`ProxyService::take_timed_out`, `BusService::take_timed_out`), and the end
  of its reach (`SandboxService::take_reach`, `unreach_all`). The model reads nothing
  at the move; the job's notice tells it at the end. The command's writes after the
  move do not join the turn of its call, as for any command left running.
- The job of a moved call (`JobProcess::Adopted`): its time limit is
  `jobs.default_minutes` cut to `jobs.max_minutes`, from the move; its proxy socket and
  bus filters live in its entry; it has no state dir, no lifeline, no forwards and no
  guard while it runs (only the launcher's end guard). Its output file starts with the
  output so far, then the rest as plain text from the shell's tee. A stop sends
  `SIGINT` to the shell's foreground job and closes the shell when the command did not
  end within 6 s. A wait for a secret that efr-shell interrupted (`AdoptedRun::looks`)
  gets the same 6 s before the close. Before each close of the shell, and at the end,
  efrd kills every process below the shell (`sweep::kill_descendants`, by the parent of
  each in `/proc`, through a pidfd; the shell is a child subreaper, so a `setsid` child
  stays below it only while the shell lives). Then the end closes the shell and sweeps
  its session. A call through the launcher takes its `result.json` from the
  call's dir and removes only that dir; a line typed into the shell takes the exit
  status of its `D` mark and is never `launcher_lost`. A wait for a password that
  efr-shell stopped in the job adds a note to the notice. A task follows
  `AdoptedRun::looks` and keeps `JobInfo::waits_input` true while the command waits
  for plain input (DS27), which `jobs.list` shows; a stop while it waits adds a note to
  the notice. The shell's end closes its
  recording and records no `shell_exited`. At start, a moved job of the last efrd reads
  its result in the call's dir and gets no guard check of a job.

### The reviewer, presence and the home store

Phase A of efr's auto spec. The turn in `efr-conversation` decides who answers an exit
and applies the verdict; efrd gives it the parts below (`run.rs` wires them into
`ConversationDeps`):

- The judge (`judge.rs`): `ClassifierJudge` implements `efr_conversation::ExitJudge`.
  One judge serves every conversation, over the provider of the conversations. Each
  step of the chain is one request: the prompt of `judge/prompt.md` as the system
  prompt (`judge/prompt-proxy.md` when the engine's egress is efr's proxy, phase D2:
  the same text with the network lines of the proxy and the rules about
  `on_allow_list`, `project_remote` and `refused_by_proxy`; `JudgeInput::egress` picks
  it), the record as the one user message, no tools, no output limit, the verdict
  schema as the output form, `one_shot`, and no cache key. The chain is the classifier
  model (`classifier.model`, else `Vendor::classifier_model` in `catalog/list.rs`, the
  small model of the provider's company), then the turn's model, then the user. A step
  that is missing, refuses, times out on the daemon's clock or answers outside the
  schema goes to the next step; refused credentials or a used-up quota go to the user
  at once. A step that refuses the output form goes once more without it. On the
  wire, `properties` keeps the order of the names (serde_json has no
  `preserve_order`): `category`, `rationale`, `risk`, `user_authorization`, `verdict`.
  The output form makes the model write the members in that order, so the prompt, its
  example and the schema's `required` list name them in the same order, and the model
  writes its reason before its risk and its verdict. A prompt that asked for another
  order got rationales that were empty, `x` or `placeholder`
  (`the_prompt_asks_for_the_members_in_the_order_of_the_wire_schema`). A rationale that
  is shorter than 12 characters or a placeholder (`x`, `n/a`, `tbd`) makes the answer
  unreadable: the classifier model gets the same request once more
  (`JudgeOutcome::reasked`, which the turn records as `exit_judged.reasked`), and a
  second such answer goes to the next step. The judge returns the model's judgement as
  it is, with the SHA-256 of the record that the model read. What it keeps between exits
  (`judge/memory.rs`): the allow and deny verdicts of each conversation's current turn,
  the models that a conversation skips, and the models that refused the output form, for
  at most 256 conversations.
- Presence (`presence.rs`): `PresenceTable` implements
  `efr_conversation::PresenceReader`. `methods.rs` marks each input of a person (a
  prompt, a steer, an interrupt, an answer, keys for the hidden shell).
  `presence.set` (`methods/presence_set.rs`, the method of `efr away`) sets a
  conversation away, or each one that is present. No timer runs: the idle state is
  found when a waiting question reads the presence (when it parks, then every 15
  seconds) or the next input comes, and only then is `presence_changed` recorded. The push to a phone does nothing yet.
  `PresenceReader::followed` is true while a client that can type answers subscribes to
  the conversation (`Connections::answerers`); a turn that a job notice started counts
  the user away unless one does.
- The home store (`home.rs`, spec 10.8, phase B): before efrd runs a call, the top of
  `DaemonToolbox::invoke` (`tools.rs`) copies what each approved exit names in
  `ExitNeed::copies` into the home store of `efr-snapshot`,
  `$XDG_DATA_HOME/efr/snapshots/home.git` (`docs/storage.md`). So `write_file`,
  `edit`, `apply_patch` and a line in the exit child share one hook. A file
  (`CoverAsk::Home`) is copied with each link on its way from the path that the user
  knows: the link as a link, the real file as an entry of its own. A directory
  (`CoverAsk::HomeDir`, also the parent of a `WriteBind::Parent` bind) is walked with
  no link followed, within `snapshot.home_max_files` files and `snapshot.home_max_mib`
  MiB; over the limits only the target is copied, and the directory is "not covered".
  efrd reads each path through `DaemonFs` and hands the entries to the store one at a
  time, at most one file of 16 MiB in memory. After the call it reads the same paths
  again and lists each walked directory again, so a file that the call made there was
  missing before and undo moves it to the undo trash. A missing path keeps its
  parents that are missing too (`HomeEntry::missing_parents`), and so does a file
  that the call made in a walked directory that did not exist: after undo removes
  such a path, it removes each of these dirs that is empty, the deepest first, such as
  the `default.target.wants` dir that `systemctl --user enable` made. A `.git`, and
  any other name that git refuses in a tree (`efr_snapshot::git_refuses`), stays out
  of a copy unread and is "not covered"; below a directory target it does not keep
  the directory from being covered, as the snapshot of a root leaves a nested
  repository out. The failure rule (plan PB14): a read error stops the call; a floor
  or persistence file that efrd cannot copy (masked, larger than 16 MiB, in a git dir)
  stops it too; any other path over a limit, and a masked path,
  runs with no copy and its `FileBackup::reason` says why. The refused list is the
  plan's masks: the engine's secrets, the sandbox-only masks, a `$XAUTHORITY` or
  `$HISTFILE` outside its usual place and `sandbox.mask`. When the exit names the real
  file behind a dotfile link (`ExitNeed::link`), `home_file_changed` keeps the path
  through the link and `real` names the real file. With `snapshot.enabled = false`
  efrd keeps no store, and a floor edit does not run. Nothing new goes to
  `$XDG_DATA_HOME/efr/file-backups/` of phase A: `backups.rs` keeps only its collector,
  which deletes the old copies by `snapshot.keep_turns` and `snapshot.max_age_days`.
- Named persistence that is no file edit (plan PB19, PB19a, PB20, PB34). A named unit
  change (`CoverAsk::UnitLinks`, such as `systemctl --user enable agent.service`)
  copies the whole unit config dir `~/.config/systemd/user` within the dir limits,
  and each link that `sandbox/units.rs` predicts from the `[Install]` section of the
  unit file (`WantedBy=`, `RequiredBy=`, `UpheldBy=`, `Alias=`, one level of `Also=`)
  as missing; after the call the dir is listed again, so a link that the prediction
  missed is caught too. A name outside the unit name grammar, a template or an
  instance, a specifier and a config dir that is a link get no prediction, so the
  engine keeps the change with the user. A named change of the crontab or of linger
  (`CoverAsk::CronTable`, `CoverAsk::Linger`) copies the state that the program reads
  (`home/state.rs`): `crontab -l` (the text, missing, or an error, which is not
  covered) and `loginctl show-user <uid> -p Linger --value`, as the entries `@crontab`
  and `@linger`, before the call and after it. Undo compares the state with the one
  after the turn and sets the one before with `crontab -`, `crontab -r`,
  `loginctl enable-linger <uid>` or `disable-linger <uid>`; a crontab that it replaces
  goes to the undo trash as `state/crontab`, and a removed unit link adds the note
  "run systemctl --user daemon-reload". `STATE_COPIES` turns the state copies off. efrd
  runs only the `crontab` and `loginctl` of `trusted_programs.rs`: found once at start
  on efrd's own `PATH` (never the hidden shell's), in a dir that root owns and only root
  can write, outside the home dir, efr's roots, `/tmp`, `/var/tmp` and `/dev/shm`, as a
  root-owned file that only root can write. Before each run efrd checks that the file
  has the device and inode that it found, and it runs it with fixed arguments and a
  clean environment. With no such program the state copies are off and the forms stay
  with the user. For the facts of such a form, `sandbox/facts.rs` reads each file that
  it installs (`FactRequest::sources`): the real file behind every link, whether its
  mtime or ctime is in the turn, and its text when it is UTF-8 and at most
  8 KiB, never a mask or a secret. A test daemon finds no program: only
  `Deps::with_trusted_program_dir` of the feature `test-sandbox-fake` names a dir of
  fakes.
- Deletes in the real tool caches (`tools/cache_deletes.rs`): the launcher lists the
  whiteouts of a contained call's private cache layers in `SandboxSummary`, and the
  turn asks the engine, then the reviewer or the user. After an allow,
  `DaemonToolbox::delete_cache_paths` checks each path again against the caches, pins
  and masks of the latest settings, and one path that fails refuses the whole list.
  Each delete goes through `DaemonFs::remove_beneath` (`sandbox/fs.rs`): the cache
  opens with no link on its path, and every path below it with `RESOLVE_BENEATH` and
  `RESOLVE_NO_SYMLINKS`, so a link never leads the delete out of the cache. Before the
  deletes, efrd copies what they remove into the home store within the same limits;
  over them the deletes run with no copy, and the end of the turn names them.

### Always-allow rules

Phase E, efr's auto spec 11.2. `engine.rs` builds the engine with the rules of
`sandbox.allow` last (`Engine::with_allow`), after the locations and the host list that
their floor checks read. A rule that fails a floor check stays in the list, disabled,
and never covers a call; only a rule of a bad shape, which the config check refuses,
stops efrd or a reload. A rule can turn bad with no edit, such as when a new project
lies below it, so each reload checks every rule again. Each disabled rule gives one
`WARN` in the log at the start and when a reload sends an engine that disables it
(`always::warn_disabled`), and the warning `sandbox.allow[N] is disabled: <reason>` in
`SandboxStatus::warnings` of `admin.status` and `admin.sandbox_check`
(`always::disabled_warnings`). The answer of `admin.config_reload` carries the warning
of each rule that its reload disabled in `warnings` (DV44), so `efr config reload`
shows it once; `efr sandbox check` and `efr sandbox allow` show every disabled rule.

`always.rs` (`RuleWriter`) is the `AllowWriter` of the conversations: it saves the rule
of an `a` answer. It runs `Engine::check_allow` on the running engine first, so it
never writes a rule that the next load would disable (`NotSaved { load_check }`). Then
it opens `config.toml` through a link, as `efr config set` does, adds the rule to
`sandbox.allow` in the form that the file uses (`Edit::add_table_entry`, which keeps
every comment) and writes the file. When the file changed under it, it plans the
change again, at most 3 times (`changed`). A rule that the list already holds keeps its
old index. A file without the owner's write bit is `read_only`, and so is a write that
the system refuses; the atomic write would replace a read-only file, but the user made
it so. Then it asks the reload task for a reload and waits for it, at most 5 seconds on
the injected clock (`reload_timeout`), so the next call reads the engine with the rule.
The writer never fails with an error: the turn runs the call as a "yes" whatever the
outcome, and records `always_rule_saved`. The writer and `efr sandbox allow remove` are
the only paths of efr that write `sandbox.allow`.

`approval.respond` refuses `allow_always` from a phone with `forbidden` before the
conversation sees it, and the conversation checks it again (`RemoteAlwaysAnswer`). An
`allow_always` to a question that offers no rule is `invalid` (`NoAlwaysOffer`). No
receipt keeps either refusal, so the question stays open for the user's own terminal.
A model-side peer holds no `approve` scope at all.

### What a call changed

`tools/snapshot.rs` (`CallSnapshots`) uses `efr-snapshot`, efr's own snapshot store in
`$XDG_DATA_HOME/efr/snapshots/`, which the sandbox masks:

- A `shell` call: a snapshot of each root before and after it, and the changes go to
  `tool_call_completed`. The roots are the turn's registered project and `$SCRATCH`
  in every mode, and in `auto` the registered projects that the line names (the
  plan's project roots; that snapshot is taken under the plan lock). A `full` call
  runs with no plan; it takes the plan lock and the snapshot of each registered
  project that holds a target which its facts said a snapshot covers, and asks the
  covered asks again under that lock. Nothing outside these roots is snapshotted, git
  repository or not. The first snapshot of a root in a turn records the turn's mode
  for `auto` and `full`, so a later efrd knows that undo covers what the turn wrote
  outside the roots.
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
- A root's git dirs come from efrd (plan PB6): the worktree record while the `.git`
  file still names it (`sandbox/projects.rs`), else `<root>/.git` when it is a
  directory. Only such a root gets the mirror of its refs; `snapshot.mirror_max_mib`
  is the store's budget (0 turns the mirror off). A file tool's writes join the turn's
  `touched` paths, which undo takes back.
- The coverage facts (plan PB15): in `auto` and `full`, `DaemonToolbox::facts`
  answers each `CoverAsk` of the engine's `FactRequest::covers` in `CallFacts::covered`. A snapshot
  ask (a path, the tracked or untracked files, the refs or the stash of a root) is
  answered by `Snapshots::covers` from a snapshot taken now, under the plan lock of
  its root; a home ask outside the roots from the sizes, the walk and the masks
  (`home::coverage`). The answers stay with the call: under the call's own plan lock,
  after its snapshot, efrd asks each covered snapshot ask again, and the call does not
  start when one is no longer covered or the snapshot failed; a home copy that the
  facts said covers a path and no longer does stops the call too. The lock is never
  held across the decision, so a question or a review blocks no other call.
- At the end of a turn (`Toolbox::turn_changes`), `Snapshots::turn_end`: `post` of
  each root, the turn's changes for `turn_completed`, the `pre` and `post` of each
  root and one record per home copy for `snapshot_taken`, and `TurnUndoInfo`: the
  changed files of each root by the name that a person sees, the home files that a
  copy covers, and what no copy covers (a left-out file, a failed snapshot, a store
  that changed, refs without a mirror, a home path over the limits or masked, a call
  that still runs). `undoable` is true only for an `auto` or a `full` turn with a
  kept snapshot while `undo.enabled` holds and no launcher of the conversation runs
  (`SandboxService::launcher_runs`, plan PB39).
- Undo (`tools/undo.rs`, `Toolbox::undo`): `efr-snapshot` plans; efrd writes. It
  refuses with `undo.enabled = false`, from a phone, while a newer turn is not undone
  (unless `force`), and lists the turns. An undo that stopped half way resumes first.
  With no turn named, it passes over the newest wake turns (`UndoCandidate::wake`)
  whose plan with `force` acts on nothing (`empty_wake_turns`): their job's writes
  join the job's own turn, so that turn is the one to take back, and those wake turns
  do not block it as newer turns.
  The apply holds the plan lock of each root of the plan for all of it: the "before"
  snapshot and the journal ref, then for each path a fresh read and a compare with
  what the plan expects (git's blob id), a copy of what is there into
  `$XDG_DATA_HOME/efr/undo-trash/<undo-id>/` (the bytes of the plan's trash blob,
  written as a new file, so another file system needs nothing special; directories
  0700, files their own mode; the bytes that the compare read when their blob id is
  the trash blob, else the store's blob), and the
  write: a new file next to the path, renamed over it, or a link made the same way.
  Every path opens below its root (or `/` for a home path) with `RESOLVE_BENEATH |
  RESOLVE_NO_SYMLINKS` (`sandbox/fs.rs`, `Beneath`), so a path through a link is
  left alone. A file that undo makes again gets its recorded mode without set-id
  bits; one that exists keeps its mode. A root whose `(dev, ino)` changed is written
  only with `force`. Then the project's refs by compare and swap, the home paths, each
  checked again against the masks and efr's own roots, then the dirs that a removed
  home path made (`HomeStep::remove_dirs`), the deepest first, each with `rmdir`
  below `/` with no link on its way (`Beneath::remove_empty_dir`): a dir that holds
  anything stays, and so do the dirs above it. Then the end marker. A write
  error stops the undo; the result lists what it did, and the next `efr undo`
  resumes it.
- `conversation.diff` (`methods/conversation_diff.rs`) reads the refs back. Without a
  `turn_id` it takes the newest finished turn of the conversation; without a
  `conversation_id` it takes the named turn's conversation, else the terminal's. A
  turn without snapshots changed no files, so its list is empty.
- An hourly task keeps the refs of the newest `snapshot.keep_turns` turns of each
  conversation whole across every store, the home store too, keeps the mirrors of a
  store within four times `snapshot.mirror_max_mib`, and deletes a store without a
  snapshot for `snapshot.max_age_days`. It deletes an undo of the undo trash and the
  phase A backups older than `snapshot.max_age_days` too.
  `snapshot.enabled = false` takes no snapshot; a file tool still shows its diff.
- At start, efrd reads the user's own `core.excludesFile` once, because the store's
  git reads no global config.

### Notices

When a turn finishes or fails, an approval waits, a background job ends (`efr: job j3
ended (exit 0): <title>`, with the processes that may still run and a lost launcher),
or a job's notice waits for the next prompt (`...; no wake turn (wake limit reached)`),
or efr forwards a port of a job or says why not (`efr: <title>: job j1 listens on
127.0.0.1:8765 -> http://localhost:8765`; the title comes first, so a cut never takes
the URL), or a command still runs in the hidden shell after its call answered (`efr:
<title>: the hidden shell still runs rsync -a ...`, a wait of it that efr keeps for you
with the row that asks and who approved it, and a stop by efr; at most 300 characters,
and a cut takes the title or the command, never the row, the approver or where to
type), and no subscription from the
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

The end of a turn that job notices started has no prompt to name. Its line names the
jobs and adds the first line of the turn's last answer, cut to the line's limit, then
`; efr history shows all`: `efr: job notice turn finished (job j3): <title>: The tests
pass.; efr history shows all`, or `efr: job notice turn failed (job j3): <title>:
<error>` (`notices::Wake`, read from the turn's row and its events through the end).
Control and format characters go, as in every notice.

### The search index

`search_index.rs` keeps the search index of the store (migration 9, `efr_store::search`)
current with the event log, on one background task (`search_index::follow`, spawned
with the other background tasks). The turn's end path does no work for it: the batch
that ends a turn writes no index row, and the task writes only through writer jobs of
its own (`search_apply`, `search_reset`, `search_merge`).

- **What goes in.** The prompts, the steers, the model's answers and the command lines
  of `shell` calls. Never the output of a command, a diff, the contents of a file or the
  body of a heredoc. The text rules (`search_index/text.rs`) run in this order: the
  store cuts each text to 33,792 characters in SQL; a command loses its heredoc bodies;
  every control and format character goes (the rule of the text that efr shows); each
  secret that the index redaction recognises becomes `[***]`
  (`efr_conversation::redact_for_index`); the text is cut to its cap at a line end (a
  prompt or a steer 32 KiB, an answer 16 KiB, a command 2 KiB); an empty text gives no
  doc. The strip comes before the redaction, so a key with an escape sequence in it is
  one word again when the redaction reads it.
- **When it runs.** At the start, at once (the cursor is in the database, so a crash or
  a stop loses nothing). Then after each committed batch that ends a turn or takes text
  back (`prompt_withdrawn`, `steering_withdrawn`), and after the broadcast lagged: it
  waits 500 ms on the clock first, so the turn ends of that time join one round. A round
  reads pages after the cursor on the indexer's own read connection (never one of the
  turns' readers), applies the text rules on the blocking pool, and writes each page in
  one writer job. A caught-up task holds no timer.
- **The duty cycle.** After a page that leaves more to read, and after each merge, the
  task waits 4 times the time of that writer job, at least 10 ms, so a backfill takes at
  most a fifth of the writer. While a turn runs (the task follows `turn_started` and the
  turn ends on the broadcast, and reads the running turns again after a lag), it writes
  at most one page or merge per second. While it waits, it already reads and cleans the
  next page. After a round of more than one page, it merges the segments of the index
  (64 leaves at a time) until a merge does no work.
- **Failures.** A text whose rules panic gives no doc; the task logs its sequence
  number at `warn`, never its text, and goes on. A panic message can quote its text,
  so the rules, the reads of the index and each search run inside
  `efr_stdx::panic::quiet`, and the hook that efrd installs at its start
  (`install_quiet_hook`) writes only the thread and the place of such a panic. A store error waits 1 s, then twice as
  long each time up to 60 s, with one `warn` line each time; a success ends the backoff.
  efrd's stop ends the task at the next row of a page, and a page cut short is not
  written.
- **A new redaction.** When the index holds another `REDACTION_VERSION` than this build
  (also 0 in a new store), the task clears it (`search_reset`) and builds it again from
  the start of the log.

After each change the task publishes a `SearchIndexStatus` on a `watch` in `State`
(`Daemon::search_index`): the cursor, when it sleeps until, the pages and merges with
their writer time, and whether it is idle. The tests wait on it
(`TestDaemon::wait_indexed`), never on real time.

### The heap trim

glibc keeps the heap that efrd frees in its arenas, so the resident set of efrd stayed
near its peak long after the conversations went idle. Two parts give that memory back
(`docs/adr/0020-malloc-trim.md`):

- `malloc.rs`, the one module of this crate that allows `unsafe_code`: `init`
  (`efr_daemon::init_malloc`), which `main.rs` calls first, before the runtime starts a
  thread, sets glibc's trim threshold to a fixed 128 KiB with `mallopt`, which also
  stops glibc's dynamic threshold; and `trim`, one `malloc_trim(0)`. Both are FFI
  through `libc`, only for Linux with glibc; elsewhere they do nothing. The setting is
  not an environment variable, so the hidden shells and their programs do not get it.
- `heap_trim.rs`, one background task in safe Rust (`heap_trim::follow`). Each actor
  adds one to `ConversationDeps::idle_trims` when it drops its turns after 10 idle
  minutes. The task then calls `malloc_trim(0)` once, on a blocking thread, after 30 s
  (`QUIET`) on the clock with no turn running, no turn end and no other actor trim,
  and at most once in 5 minutes (`MIN_GAP`). It follows the turns from the committed
  batches (`turn_started` and the four turn ends), and reads the running turns from
  the store after a lagged broadcast. Without an actor trim it holds no timer. A heap
  trim takes about 8 ms. It logs one debug event (`released`).

In a test of 20 conversations of 50 turns, the resident set after the idle period and
the heap trim is about one third of the peak of the load; before, it stayed at about
90% of the peak.

### The search methods

`conversations.search` (`methods/conversations_search.rs`) gives one row per
conversation, best first, for the picker of `,resume`, `efr search` and the phone.
`conversation.search` (`methods/conversation_search.rs`) gives the hits in one
conversation, newest first, for the preview. The store parses the query, searches the
index and ranks the rows (`efr_store::search`); efrd adds which terminal holds each row
and the name of its project (`methods/recent_rows.rs`, which `conversations.recent`
uses too).

- **Who may search.** Both methods need `read`, but only a user-side peer may call
  them: efrd walks `/proc` for each call, and a model-side peer or one whose process is
  gone gets `forbidden` (`user_side_only` in `methods.rs`). Search would give a process
  of the model's commands the text of every project and the places of each secret that
  the index replaced. A phone whose process is the user's may search.
- **Lanes.** A search runs on one of the store's 3 search lanes, never on a reader of
  the turns, so a burst of keys cannot slow a prompt or a turn. A fourth search waits
  for a lane.
- **Time.** The time of a search counts from the start of its handler, on the clock
  (`methods/search.rs`). At 150 ms the row loop of the store ends at the next row, and
  the search ranks what it saw (`partial: time_cap`). At 200 ms efrd answers: with no
  rows and `time_cap` when the search has a lane, else `busy`. Then it interrupts the
  statement of the lane. One step inside FTS5 reads no interrupt, so the lane can stay
  busy until that step ends. When the client cancels or closes, the transport drops the
  handler, and the drop stops the search the same way.
- **The empty query.** The rows of `conversations.recent`, in its order, 50 by default
  and 200 at most. Only the empty query reads the place of `pwd` (the registry, and the
  plugin's copy of it); a query reads only the registry file, and only when a row is in
  a project.
- **Titles.** Every row shows a title with its secrets replaced, which `title_ranges`
  point into: the search title (from the indexed text of the first prompt), else the
  projection's title after the index redaction. When efr cut the projection's title at
  80 characters, the row loses its last word too, so a key that the cut made too short
  to be recognised never shows. `conversations.recent` keeps the projection's title.
- **Hits.** A hit has its kind, its event, its turn, its time and a `history_cursor`
  whose `conversation.history` page ends at the hit. `index.percent` shows while the
  indexer is more than 5,000 events behind the log.
- **Errors and logs.** A query of more than 256 characters or 16 words is `invalid`
  before the search takes a lane. Every other error of the store on this path is
  `internal` with one fixed sentence, "the search failed in the store", so neither the
  answer nor the log shows the query. The debug line of a search holds counts and
  times, never the query or a text.

### Full mode

Full mode runs the model's commands as the user, with no sandbox and no questions,
while efr still reads each line for a small set of floors. efrd's part:

- `full.rs`: `full::status` says whether a turn may run in full mode:
  `permissions.allow_full`, and the first refusal that holds, in this order: efrd runs
  as root (`FullRefusal::Root`), `snapshot.enabled` is false, `undo.enabled` is false,
  `shell.program` is no zsh (`efr_shell::gets_integration`, the test that gives a
  hidden shell the integration; a zsh named by its path counts). The conversations
  read it in `ConversationConfig::full_mode` at each resolve and before each call
  (`settings.rs`, from the latest settings, so a reload that turns undo off stops full
  mode from the next call on), and `admin.status` reports the same value as
  `full_mode`. efrd reads its effective uid once at the start (`run.rs`); a build with
  `test-sandbox-fake` takes it from `Deps::with_runs_as_root` instead.
- `permissions.mode = "full"` is a valid default only next to
  `permissions.allow_full = true` (`efr-config` refuses it otherwise, at the start and
  at a reload). At the start, and at a reload that makes full the default, efrd writes
  one WARN line to its log; when a refusal holds, the line names it, and a turn that
  names no mode runs as `auto` with the reason in its fallback, while a prompt that
  names full mode fails.
- A full call collects the facts of its line and copies home files first as an `auto`
  call does (`CallContext::predicts`), with no sandbox, and its snapshot roots record
  the mode `full`, so undo covers what the turn wrote outside its roots. It clears
  sudo's cache before and after its line (`shell.sudo_cache` above).
- While a full turn or a job that a full call started runs, a reload that loosens
  permissions is held (the config hold, under Live reload).
- A reload whose installed settings refuse full mode (the gate off, undo or the
  snapshots off) stops every job that a full call started (`JobStopBy::FullOff`, under
  Background jobs). Such a stop starts no wake turn; its notice rides in the next
  prompt.

### Features

- `local-pty` (default): hidden shells on PTYs opened in this process through
  `efr_pty::LocalPtyHolder`. Without it, and unless a holder is injected, every shell
  start fails with a clear error. The PTY holder milestone deletes the feature.
- `screen-ghostty`: the libghostty-vt screen backend (needs Zig 0.16.0). With it, the
  screens are ghostty unless `EFR_SCREEN=vt100`; without it, vt100.

- `test-sandbox-fake`: the test seams of the `auto` sandbox in `sandbox/seams.rs`:
  `Deps::with_sandbox_launcher` names the `efr-sbx` to copy,
  `Deps::with_probe_override` replaces the probe's result, `Deps::with_job_env` gives
  every background job a fixed environment, `Deps::with_peer_probe` lets a test ask
  for the side of a socket peer by pid, `Deps::with_statvfs_helper` names the
  program that measures the mounts of the mount view, and `Deps::with_test_host_ports`
  binds a free port of `127.0.0.1` for each job port forward (`TestHostPorts`; a test
  build without it gets a map of its own, and only `Deps::with_system_host_ports` and the
  smoke's `efrd` bind the real ports). Only `efr-test-daemon`
  turns it on; `just install` refuses an `efrd` whose hidden `--test-seams` says `on`.

`cfg(feature = ...)` appears only in `screens.rs`, `shells.rs` and `sandbox/seams.rs`
(a tidy rule).

## Tier

Tier 4: a binary, the composition root.

## Allowed dependencies

Every library crate except the test crates: `efr-stdx`,
`efr-protocol`, `efr-store`, `efr-credentials`, `efr-permissions`, `efr-scope`,
`efr-holder`, `efr-http`, `efr-screen` (with its vt100 backend), `efr-provider`,
`efr-screen-ghostty` (optional), `efr-pty` (optional), `efr-shell`, `efr-tools`,
`efr-providers` (the OpenAI providers, and the provider of `anthropic-api` with its
catalog and the key check of a login), `efr-oauth-openai`,
`efr-config`,
`efr-conversation`,
`efr-transport`, `efr-sandbox` (the spec of a sandboxed call, the worktree record,
the probe's report and the plan that `sandbox.explain` reads) and `efr-snapshot` (the
snapshots before and after each call that can write, the turn's changes and
`conversation.diff`), `efr-proxy` (the network proxy of the sandbox, as tokio tasks
of efrd: one listener per contained call). `xtask/src/deps.rs` holds the allowlist; `efr-test-daemon` is its only dev-dependent,
and only from `tests/`.

Third-party crates: `tokio`, `tokio-util` (`CancellationToken`), `async-trait`, `bytes`,
`futures` (the stream of a model call, whose answers say whether a stored key works),
`serde`, `serde_json`, `jiff`, `nix` (`flock`), `base64` (the hello
challenge), `clap` (the flags), `sd-notify` 0.5.0 (`READY=1`, `STOPPING=1`), `rustix`
(inotify, for the config file watcher), `toml_edit` (the values that the settings tool
sets and the one-line text of a rule), `tracing`, `tracing-subscriber` (with its reload
layer for the log filter), `tracing-journald`, `thiserror`, `sha2` (the check of the
sandbox launcher's copy and the hash of the record that the reviewer read), `libc`
(glibc's `mallopt` and `malloc_trim`, only in `src/malloc.rs`, ADR 0020), and `anyhow`
in `main.rs` only.

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
- The last command and the recent commands of a prompt reach the turn in memory only;
  they never enter an event, a receipt or a log field.
- An API key reaches the credential file only; it never enters an event, a receipt,
  a log field or the text of an error.
- Shutdown releases the lock last, after the database is closed.
- A turn, a prompt and `models.list` never wait for a fetch of the model catalog,
  with one exception: while the Anthropic catalog has no list at all, a prompt and a
  manual compaction wait for one fetch.
- A refused key is a time in memory; nothing of the key goes to the refusals, the
  status, the `logout_completed` event or a log line.
- efrd names itself to the backend as efr, with efr's own version.
- A daemon with an injected provider never fetches the model catalog.
- The judge never allows by failing: a chain of models that gives no verdict ends at
  the user.
- efrd never runs an approved floor file edit whose copy it could not take, and it
  never copies a secret or a mask into the home store.
- Undo never writes through a link, never writes a path that changed since the plan
  read it, and copies every file that it overwrites or removes into the undo trash
  first.
- efrd runs `crontab`, `loginctl` and `xdg-dbus-proxy` only from a root-owned dir
  outside every write root, as the file that it found at start, with fixed arguments
  and never a word of the model.
- A background job starts only after its launcher's pid is model-side, and a process
  of a job holds `read` only on the socket, also after its launcher is gone, while its
  session lives. efrd writes a job's output itself, and never through a link.
- A contained call reaches the network through the proxy only with
  `network.mode = "proxy"` and a passed proxy self-test; its socket lives no longer
  than its call or its job, and a test build of efrd never resolves a name or connects
  to an address that a test did not map.
- A call reaches a bus through the filter only by its own proxy, which lives no longer
  than its call or its job; a proxy that cannot start fails the call, and never gives
  it the real bus. A test build of efrd never reaches the machine's buses.

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
providers that efrd builds for each id with their settings, the tool adapter and its answer to who can answer hidden input,
the judge (its request with no output limit and no cache key, each step of the chain
and why it goes on, the check of the answer, the second request after a placeholder
rationale, the order of the schema on the wire, the resend without the output form, and
what it keeps: the verdicts of a turn and the skipped models of a conversation),
presence, the home store (`home/tests.rs` and `tools/tests.rs`: links, masks, a
missing file, the dir limits, a failed copy, an unchanged file, a cache delete, the
coverage facts and the check under the lock, the counts at the end of a turn), the
undo applier (`tools/undo/tests.rs`: modes, the undo trash, links, the plan lock, a
replaced root, a write error and its resume, home files and the masks, the empty dirs
that a turn made, a home dir with a git repository), the named
persistence (`sandbox/units/tests.rs`: the links that a unit change makes;
`home/state/tests.rs`: the crontab and linger copies and their undo;
`trusted_programs/tests.rs`: the programs that efrd runs itself), the collectors
of the phase A backups and the undo trash, and the check of cache deletes. The tests
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
`input_respond`, `sandbox`, `drafts`, `turn_input`, `catalog`, `anthropic`, `presence`,
`classifier_eval`, `apply_patch`, `backtrace`, `cache_prefix`, `compact`, `home`,
`snapshots`, `undo_method`, `undo`, `jobs`, `full`) run the daemon through `efr-test-daemon`'s `TestDaemon` and replay
its fourteen NDJSON scenarios, each with the assertions of its case: the fake PTY
holder plays the hidden shell, the replay provider or a local Responses server plays
the model. The `shell_` tests run a real zsh and skip with a message unless
`EFR_TEST_ZSH=1`: a command runs and is recorded, a password typed through
`input.respond` reaches only the program (not the model's next request, the event log,
any file of the daemon's tree or any log line at any level), and a password prompt
that no client can answer is stopped within seconds. `input_respond::left` leaves a
command running past a short timeout: it prints a marker and then reads a FIFO that the
test writes after the call answered, so it waits for input only once it is left. The
events and `left_running`, a follower that answers its password wait, the notices
without a follower, the reviewer's and the user's approval of a wait, the refused
password for a contained command (the fake launcher), `shell.interrupt` with and
without `close` (and from the phone), the next call that fails at once, the preamble
and the limit run there.

The `jobs` module starts background jobs through `efr-test-daemon`'s `FakeLauncher`
(a `/bin/sh` script in place of `efr-sbx`) with a fixed job environment, so it needs no
zsh: the start, the time limit on the test clock, the stops by the model, the user and
efrd's stop, a launcher that stopped itself, the limits, the output tool, watch
matches, a job in `manual` after the user's yes, and the ids after a restart. A job
that must outlive its turn blocks on a FIFO that the test writes after the turn's end.
Its two `shell_` tests take the environment from a real hidden zsh. The unit tests of
`jobs/` cover the output file and its cut, a planted link, the start byte after the
peer list, the lifeline, the sweep by session and the restart records, and with a test
launcher whose `guard-check` finds a planted hook, the guard schedule, a guard break
and the check of a lost job at a restart; the undo rule, the retention, the wake window
and the time from which a program counts as changed. The `jobs_undo` module runs undo
while a job of the same or another conversation runs, and after a job's files joined
its turn. `sandbox::jobs_guard::shell_job_planted_hook_quarantined_before_next_turn`
runs a job on the real `efr-sbx` (`just test-sandbox`, a real zsh) that plants a hook
in a new repository, and checks that the next contained call's guard check moves it to
the quarantine and stops the job.

The `jobs_forward` module tests efrd's side of the job port forwards with the
`FakeLauncher`, which starts no forwarder, and `efr-test-daemon`'s `FakeForwarder`,
which the test plays on the job's forward socket with an echo server behind each port.
efrd binds each forwarded port through `TestHostPorts`, which takes a free port of
`127.0.0.1` in its place and reads the machine's own tcp tables. It covers the record,
`jobs.list` and the start note, bytes both ways with a half close, a gone port that
keeps the port, a port in use (a failed bind, a listener on `::1`, another job), the
refusals that efrd checks again (service ports, the bridge's port of a proxy job), the
limits of a job and of efrd, a peer of another uid, a bad frame and a port below 1024,
the close at the job's end, at efrd's stop and at a reload that turns forwards off, no
reopen after a reload that turns them on, and no forwards for a job outside `auto`,
for a network view job, and with `jobs.forward_ports = false`. `real_host_bind` binds a
free port below 32768 of the machine through the system's host ports, and tries another
when a program took it. The unit tests of `jobs/forward/` cover the plan of each launch,
the socket in a deep call dir, the table of held ports, the lines, and the two checks
of the plan: a listener of std and tokio has no `SO_REUSEPORT`, and the peer's row of
an accepted connection is in `/proc/net/tcp` at once.

The `reach` module tests the reach of a contained call on efrd's side with the
`FakeLauncher`, which starts no reach helper, a `FakeForwarder` with an echo server
behind each port, and `efr-test-daemon`'s `FakeReach`, which the test plays on the
call's reach socket with no TCP port. A foreground call runs through a real hidden zsh,
so most of its tests need `EFR_TEST_ZSH=1`, and a line that names a port runs a fake
`curl`. It covers the route to the conversation's own job (64 KiB both ways), the
refusal of another conversation's port, the close after a gone server and the
`Unreach` at the job's end, a forward that comes during the call, a reload that turns
forwards off, a call past its timeout that keeps its reach until its result, no reach
without a forwarding job, for a job and for the exit child, the limit of 32
connections that leaves room for the host, and the fact of a line: a forwarded port
needs no exit, the wait for the forward of a new job, the end of the wait at 3 s, and
no wait for an older job. `reach/unforwarded.rs` covers the live test of a port that a
program of the user holds: the line of the job's port does not run, with the refusal
for the model and no question, also when the outcome comes during the settle wait; a
port of no job still asks; and a forwarded port of the same job still reaches it. The
unit tests of `jobs/forward/reach/` cover the ports and routes of `Forwards`, which call
gets a plan, the ports that a line names, the wait of the fact on the test clock, and
the refusal of each outcome that is no forward, never for another conversation.

The `login` module also runs `admin.login_api_key` and `admin.logout` against the
local server's `/v1/models`: a key that is checked with its organization and project
headers and stored, a refused key with the server's message, a check without an
answer and a store without a check, keys and providers refused before a check, an
Anthropic key whose check fails, a logout and the status of every provider, a new
key for the running provider that brings the model list of that key, a logout of the
active provider that says so and records `logout_completed`, and a refused OpenAI key
whose time shows in the status until a logout. With every log
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
login and calls no model; a refused key at the fetch that fails the prompt with the
server's message and shows in the status until a fetch works; another failed fetch
with the server's answer; a model with its output limit in the config that runs
without a list; a refused model call in the status until a call works or a new login;
a `[model] name` of OpenAI that fails a prompt with the cause and the fix, and a
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

The `proxy` module runs the network proxy (phase D2) through efrd with the shared fake
launcher, background jobs and the test seams, which resolve no name unless a test maps
it: a host grant that reaches one job's socket only, a socket that lives as long as its
job, the refusals in `job_ended` and its notice, a refused host as a fact of the next
exit, `network.mode = "none"` and a failed self-test that keep the phase 1 behavior, a
reload whose list reaches a running job and a mode change that does not cut one,
`sandbox.explain` with the proxy on, and a test name that reaches its server. With a
real zsh, a foreground call's refusals reach the tool result. Unit tests
(`sandbox/proxy/tests.rs`, `sandbox/facts/remotes/tests.rs`,
`sandbox/proxy/tokens/tests.rs`) cover the bind in a deep call dir, the sweep of a call
past its timeout, the live list, the mirror read, the remote facts and the project
snapshot, and the credential deny.

The bus filter's supervisor (phase E) has unit tests below the turn
(`sandbox/bus/tests.rs`, with the fake program and launcher of `sandbox/bus/fake.rs`
and two stand-in bus sockets): the arguments that reach the program, the ready wait,
a start failure that fails the call, the end with the call, the sweep of a call past
its timeout, a relative socket in a deep call dir, a stale socket, the upstream check,
a crash reported as stopped, a reload's rules for the next call, the dropped views,
a test build without a seam, the self-test (its version floor, a failure, no program,
none while `dbus.proxy` is off, and a reload that turns it on), and the status with
its reason. `jobs/tests.rs` checks that a job's filter lives with the job.
`sandbox/bus/log/tests.rs` reads the refusals from logs of `xdg-dbus-proxy` 0.1.9: the
pairing with the open call, signal lines, driver refusals, invalid names and the cap
of 16. At turn level, `tests/it/bus.rs` runs a fake proxy through the shared fake
launcher with two stand-in bus sockets: a read form gets the view with no review, the
refusals reach `JobEnded` and the job's notice, a job's filter lives with the job, a
filter that died gives the note, and with a real zsh a foreground call's refusals
reach the tool result and a whole bus grant starts no filter. No line runs a real
`systemctl`: a job's line waits on a FIFO that no one writes, and a foreground call
finds a fake `systemctl` first on its `PATH`.
`sandbox_real_wrapper_runs_the_real_proxy` runs the real launcher and the real
`xdg-dbus-proxy` under `just test-sandbox` when the program is installed. The eval
checks that its daemon keeps the filter off (`eval_daemon_keeps_bus_proxy_off`).

The `sandbox` module runs the `auto` mode end to end with a scripted model: the
probe's failure and the fallback with its reason, a launcher in a write root, a project
at the home directory, the one-command rule, `sandbox.explain`, and, with a real zsh and
a fake launcher (a script that runs the child shell directly), a routine command that
runs contained without a question, an exit that asks and runs with its grant, and a
failed start that runs the probe again; and a double-forked process of an approved
command, which python3 plays, that reaches the socket and may only read. With
`EFR_TEST_SBX_BIN` (`just test-sandbox`) one call runs in the real sandbox. A finished
call's dir is removed. The same module runs the reviewer with efrd's judge: a named
floor file edit that runs after the reviewer's allow with its copy in the home store
and `home_file_changed`, one that the reviewer refuses, and an unnamed one that asks the
user. Its child module `sandbox::review` follows a floor file edit through
`apply_patch`, `write_file`, `edit` and a dotfiles link, with the record that the
reviewer reads (the target's facts from efrd's own look, the payload of the edit), and,
with a real zsh, deletes in a private cache layer that reach the real cache after the
reviewer allows them and stay private after it refuses; under `just test-sandbox` one
such delete runs in the real sandbox. Its child modules `sandbox::netview` and
`sandbox::mount_view` run the views of phase D1 on the real `efr-sbx` with a real zsh
and efrd's own probe: `ss -Htln` runs with `Grant::NetView` and no review and lists a
listener that the test bound; a function named `ss` from the sandbox state of an
earlier call runs in a view call, and its TCP socket fails with `EAFNOSUPPORT` while
the test's listener accepts nothing; a background job that watches the network starts
with the view and no review, sees the test's connection and stops by `job_stop`; and
`df` runs with no review and lists the host's mounts, not the sandbox's binds. Its
child module `sandbox::proxy` runs the network proxy of phase D2 the same way, with
`network.mode = "proxy"`, the launcher's bridge and a local HTTPS server on a loopback
address of its own (`sandbox/proxy/registry.rs`: a rustls server with a leaf of a test
CA that `openssl` makes at test time, and a log of each request), which the test seams
map to a `.test` name: `cargo fetch` from a sparse registry runs with no review and
gets the crate (and the same line with an empty allow list gets a refusal of the
registry); a script that pushes to a listed git host sends no credential from the
masked credential files, while the same push outside the sandbox sends one; a token in
the project's git config or `.npmrc` makes efrd deny that host for the call; a script
that reaches a host that is not listed gets the proxy's note in its output; and a
background dev server with the `localhost` route answers the test on the host's
loopback. Its child module `sandbox::forward` runs the job port forwards on the real
launcher with a real zsh: a job's `self-test --echo` server on 127.0.0.1 or on `::1`
only is reachable from the host through the test seam, in `proxy` and in `none` mode;
the stop of the job and a restart of efrd leave no listener; and a foreground call
gets no forwarder. The unit tests of `sandbox/` cover the plan, the probe with a fake launcher, the lock, the worktree record, the facts, the report, the
quarantine and the peer check on real process trees.

The `full` module runs full mode end to end: the gate key, the config default with
and without `permissions.allow_full`, the caps of a phone turn and of a wake turn, the
refusal of a root efrd, a full turn with no fallback when the sandbox is not available,
the questions that full mode
still asks (a guarded delete, a process that outlives the turn, a settings change) and
who can answer them, and `efr undo` of a full turn. A guard test uses the fake PTY
holder, so its line never runs, and its delete is a dead branch. Its `shell_` tests
(`EFR_TEST_ZSH=1`) write only below the stand-in home, find a fake `sudo` first on the
hidden shell's `PATH`, and check the snapshot before a call, the copy and undo of a
home file, the config hold against a script, and a model side peer that cannot answer.
Its sandbox twin `sandbox::full` (`just test-sandbox`) checks that an `auto` turn after
a full turn is contained again and that an `auto` job keeps its proxy while a full turn
runs.

The `presence` module checks that a conversation goes away after the idle minutes and
comes back with the next input, and `presence.set` for one conversation and for each
one that is present.

The `classifier_eval` module is the recorded-fixture eval of phase A. Each fixture of
`tests/it/fixtures/classifier/` is one line of the command corpus (8 to 11, 44 to 55,
66, 99, 106 and 107) with the user's prompt and the reviewer's recorded answer. Each runs
as a turn in `auto` through the engine, the facts, the judge and the turn, with fake
shells that run nothing. The eval checks which lines the reviewer judges and which only
the user decides, the verdict after the allow rule and the naming rule, one review per
judged line, and that no record holds the text of a tool result. A second eval replays
20 recorded answers to prompts for system work and checks that each request holds the
auto rule and that no answer says that the model lacks permission before its first
call. The job eval of phase C replays five prompts that start a background job, and
the view eval of phase D1 (`view_prompts.json`) six prompts: `ss`, `ip -s link`, a
network watch job and `df -hT` run with no review (the first three with
`Grant::NetView`), a read form of `nmcli` is a `bus` exit for the reviewer, and
`nmcli radio wifi off` is a `bus` exit that only the user may allow. The proxy eval of
phase D2 (`proxy_prompts.json`) runs four prompts with `network.mode = "proxy"`: "update
the dependencies" runs contained with no review; a download from a host that is not
listed is a `host` exit with a grant of that one host, and the record names the host;
"push this branch" stays an `upload` exit outside the sandbox; and a dev server on
`localhost` is a `host` exit with the whole network. The answers are
written for the verdicts of the spec, not taken from a live model, so the evals prove
the replay and efr's rules; the real models are a manual check.

The heap trim has unit tests in `heap_trim/tests.rs` (an in-memory store, a
`TestClock` and a trimmer that only counts its calls, so they need no glibc): one heap
trim after the quiet period of many actor trims and none after it without a new actor
trim, none while a turn runs, a turn that starts in the quiet period, the gap
between two heap trims, and an actor trim before the task first runs.

The search indexer has unit tests in `search_index/tests.rs` (an in-memory store, a
`TestClock` and a store seam that fails a read or takes a set time for a job: the
backfill, the debounce, the duty cycle, the slower backfill while a turn runs, the
merges, a lagged broadcast, a new redaction, withdrawn prompts and steers, a text whose
rules panic, the backoff, and the stop in the middle of a page) and of its text rules in
`search_index/text/tests.rs` (the strip, the cut of each kind, heredocs). The module
`tests/it/search_index.rs` runs it in a `TestDaemon` (a turn is in the index 500 ms after
its end; a restart catches up; the stop), and a proptest checks that no 8 characters of
a secret that the index redaction recognises reach a doc or a title. Its `bench_search_*`
tests are benches (`#[ignore]`), which `just bench-search` runs in release: the text
rules of a turn and of the worst docs, the backfill of corpora S and L, the wait that
the backfill adds to a turn's commit on a store file, and the memory of the indexer.

The search methods have unit tests in `methods/search/tests.rs` (the caps on a
`TestClock`, the stop of a dropped handler, the titles, the errors) and in
`methods/tests.rs` (who may search, and the bench `bench_search_peer_walk` of the walk of
`/proc` for each search). `tests/it/search.rs` runs them in a `TestDaemon` on a store
that the test writes before the start: what a query finds and never finds (command
output, a heredoc body, the redaction marker), the titles, the order of the empty query,
the index percent, the answer size, and a proptest that no 8 characters of a secret
are in an answer. The daemon's hook in the row loop (`TestDaemonBuilder::search_hook`)
holds a search, so the tests of the caps, of a cancel and a closed client, and of a
prompt while every lane is busy wait on events, never on real time.
`tests/it/sandbox.rs` checks that a process of the model's command may not search.

No test uses the network, a real model, real time, the user's home, config or runtime
directory, or the git configuration of the machine.

## Benches

The `bench_perf_*` benches are ignored tests. `just bench-perf` runs them in release.

The benches of `tests/it/perf.rs` run efrd end to end. They report times, counts and
sizes, and do not fail on a time. Each one runs a `TestDaemon` with a file store on a
tree under cargo's target temp dir. A task moves the daemon's `TestClock` with real
time, so these benches use real time. The model is a fake model of the test, or the
real providers against the fake servers. Two benches run again in a child process of
the test binary, with a git first on its `PATH` that counts its runs.

- `bench_perf_ttft`: the time from `prompt.send` to the first text of the answer, on
  the HTTP Responses path and the Messages path, at 20, 200 and up to 2,000 turns.
  It also gives the time to the model request.
- `bench_perf_prompt_send`: the time from `prompt.send` to its ack, with 1, 5 and 20
  open conversations, in a project and outside, with the repository hint on and off.
  It counts the git runs.
- `bench_perf_turn_end`: the time from the model's last event to the turn's end, in a
  git project of 100, 2,000, 19,000 and 20,000 files where the turn writes one file.
  It counts the git runs.
- `bench_perf_shell_call_auto`: one `shell` call in `auto` with the fake launcher, for
  `true`, 10 `cd`s and `rm` lines of 100 B to 100 KB, with 0, 1, 4 and 16 jobs. It
  needs `EFR_TEST_ZSH=1`. Budget: the decision of the `rm` line of 100 KB takes less
  than 4 s (v0.0.4 took about 9 s), and the decision of the line of 10 `cd`s less
  than 75 ms at each count of jobs (v0.0.4 took about 95 ms, of which about 68 ms
  went to the real paths of its 2,048 paths).
- `bench_perf_subscribe_snapshot`: the start of `conversation.subscribe` on events of
  1 KB, 32 KB and 500 KB, and with a newest event of 1.5 MiB.
- `bench_perf_daemon_memory`: the resident memory of this process after the start,
  after 20 conversations of 50 turns, and after an idle period of 3 hours on the
  clock. It counts the turns that the idle trim of the conversation actors drops
  (from the actor's debug event), and fails when the trim drops less than 90% of the
  turns of the load. It also counts the saved messages and the event JSON of the page
  that each turn's start decodes (from the history's debug event), and fails when
  either mean passes 64 KiB. Like `main`, it calls `efr_daemon::init_malloc` before
  the daemon starts. After the idle period it moves the clock past the quiet period
  of the heap trim, which the daemon's own task runs, and reports the resident memory
  after it. It fails when there is no heap trim, or when the resident set after it is
  not below 50% of the peak of the load (about 35%; without the heap trim, about 90%).
- `bench_perf_daemon_growth`: the resident memory after 50, 150 and 300 conversations
  of 5 turns, and its growth for each conversation. After each batch the bench moves
  the clock past the idle trim and waits for the trims, so the growth is what a
  conversation keeps after its trim. It calls `efr_daemon::init_malloc` too, and the
  daemon's heap trim may run between the batches. It fails when that growth passes
  46 KiB.
- `bench_perf_shell_call_sandbox` (`tests/it/sandbox/perf.rs`): 10 calls of `true` in
  `auto` on the real sandbox, in a git project of 100, 2,000 and 20,000 files, with 0,
  1 and 4 contained jobs in the same project. Each `phase` line of efrd is a measure.
  It needs `EFR_TEST_ZSH=1` and `EFR_TEST_SBX_BIN`. Its budget: in the step with no
  job, each call writes one `snapshot_before` and one `snapshot_after` line, and the
  snapshot of each of its two roots after it a `snapshot_changes` line, so no time
  counts twice.
- `bench_perf_home_copy` (`src/home/tests.rs`): `home::prepare`, the copy into the home
  store, `home::after`, the deltas and `home::changed` for a directory of 200, 2,000
  and 20,000 files, with a raised limit. Its budget: `prepare` and `after`, and the
  deltas, grow with a slope below 1.3, and `prepare` and `after` of 20,000 files take
  less than 1 s.
- `bench_perf_home_tally` (`src/tools/tests.rs`): a cache delete of a directory of 200,
  2,000 and 20,000 files with its home copies, whose copy after the call adds each
  file to the tally of the turn, and the end of the turn. Then the tally alone with as
  many files that no copy covers and as many targets of a job call that left its copy.
  Its budget: both grow with a slope below 1.3, the tally of 20,000 files takes less
  than 1 s, and the delete and the turn end of 20,000 files less than 3.5 s (with a
  linear search of each list they took about 7 s).
- `bench_perf_undo_apply` (`tests/it/undo.rs`): the apply of an undo of a turn whose
  shell call changed 50, 500 and 2,000 files. A test process cannot change its own
  `PATH`, so the bench also starts a copy of itself with a counting `git` first on the
  `PATH`, and the copy reports the git runs of efrd. It needs `EFR_TEST_ZSH=1`. Its
  budget: at most one git run per changed file and 50 more.
- `bench_perf_daemon_start` (`tests/it/perf_start.rs`): spawns the built `efrd` on a
  temp `EFR_HOME` and measures the time to `READY=1` and to the socket file, for the
  first start and for five restarts, with the phases from its log lines. The cases are
  the sandbox off and on, `dbus.proxy` off and on, and an empty store or search corpus
  S or L.
