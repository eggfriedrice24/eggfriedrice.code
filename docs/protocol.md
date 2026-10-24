# Protocol

This page describes the protocol between the efr clients and efrd. It holds a short
summary and the changelog of the protocol. The types and the frozen fixtures are in
`crates/efr-protocol`.

The command `cargo xtask protocol-docs` does not write this page yet, so you edit the
page by hand. Add a line to the changelog below for each change of the protocol.

## Summary

- Transport: a Unix socket at `$XDG_RUNTIME_DIR/efr/daemon.sock`. Each frame is a
  4-byte big-endian length followed by one JSON object, at most 16 MiB. The later
  WebSocket transport sends one JSON frame per text message.
- Client frames: `{id, method, params}` and `{cancel: id}`. Server frames:
  `{id, item}`, `{id, end}`, `{id, error}`, and `{ack: id}` on streams that ask for it.
- `hello` comes first; both sides send `PROTOCOL_VERSION`, and a mismatch fails with
  `ProtocolMismatch` before any method runs. Additive changes do not bump the version.
- The frozen fixtures in `crates/efr-protocol/fixtures/v1/` are compared byte for byte.
  Changing one needs `just bless` and a line in the changelog below.

## Changelog

No protocol version has shipped yet.

- Before version 1 ships: `assistant_message_updated` carries `offset` and `delta`, the
  text added since the previous update, instead of `text`, the whole message so far,
  which made the event log grow with the square of an answer's length.
- Before version 1 ships: answers to a running command that waits for input. New event
  kind `tool_call_input_changed` with `input` (`none`, `visible` or `hidden`), new
  method `input.respond` (scope `terminal`, no command id) whose `text` is a plain
  string, and a new optional `answers_input` flag on `conversation.subscribe`, false
  when absent. The `conversation_subscribe_params.json` fixture now sets it.
- Before version 1 ships: turn settings. New wire enum `mode` (`manual`, `cautious`,
  `auto`). `prompt.send` and the `prompt_queued` event carry an optional `settings`
  object with an optional `mode`, `model` and `effort`, where the effort is a plain
  string; absent means none. The `turn_started` event and the `prompt.send` result carry
  an optional `settings` object with the effective `mode`, `model` and optional
  `effort`, and an `overridden` object whose `mode`, `model` and `effort` flags say which
  values came from the prompt; absent means not recorded, as in turns from before. The
  fixtures `prompt_send_params.json`, `prompt_send_result.json`,
  `events/prompt_queued.json` and `events/turn_started.json` now set them, and the new
  `modes.json` freezes every mode.
- Before version 1 ships: new method `models.list` (scope `read`, no params) whose
  result lists `models`, each with its `id`, its `efforts` (plain strings), an optional
  `default_effort`, a `default` flag for the model that a turn uses when the prompt
  names none, and its `source` (`builtin` or `config`). New fixtures
  `models_list_params.json` and `models_list_result.json`.
- Before version 1 ships: new method `admin.config_reload` (scope `admin`, no params)
  whose result says whether the config was `applied`, and carries an optional `error`
  (`message`, optional `line`, `column` and dotted `key`) and the `restart_needed` keys,
  absent when empty. New fixtures `admin_config_reload_params.json` and
  `admin_config_reload_result.json`.
- Before version 1 ships: the `admin.status` result gains an optional `roots` object
  (`config`, `data`, `state` and `runtime`, each a `path` and a `source`:
  `dir_variable`, `efr_home`, `xdg` or `run_user`) and an optional `config` object (the
  `path` of `config.toml`, whether it `exists`, an optional `symlink_target`, the
  optional `reload_error` of the last reload in the form of the `admin.config_reload`
  error, and the `restart_needed` keys, absent when empty). Absent means not reported,
  as from an earlier daemon. The `admin_status_result.json` fixture now sets both.
- Before version 1 ships: the daemon applies turn settings and answers `models.list`.
  The `turn_started` event and the `prompt.send` result now always carry `settings`; a
  turn from a remote origin records a `mode` of at most `cautious`. A `prompt.send`
  whose `settings.model` is not in `models.list`, or whose effort (its own or the
  config's) is not one of the model's `efforts`, fails with `invalid` and `data`
  `{setting, value, choices}`, plus `model` for an effort; a turn whose settings no
  longer fit when it starts ends with `turn_failed` and the same error body. No wire
  type changed.
- Before version 1 ships: the daemon answers `admin.config_reload` (it reloads at once and
  reports the outcome), and the `admin.status` result now always carries `roots` and
  `config`. No wire type changed.
- Before version 1 ships: a restart no longer holds queued prompts. At the next start
  the daemon records each prompt that waited, and each one an earlier daemon held, as
  not run with `turn_cancelled` (no `turn_started` before it), and writes a notice to
  its terminal that names it and says to send it again; nothing runs it later.
  `prompt_held` stays in the protocol for older logs, and no daemon writes it any more.
  No wire type changed.
- Before version 1 ships: answers at the edges. The `tool_call_input_changed` event
  gains an optional `looks_secret` flag, false when absent: a `visible` wait whose
  prompt line looks like a password prompt while the terminal is not in line mode, as
  behind a relay such as `sudo`'s own pty, `ssh` or `docker exec`; a client hides what
  the user types and still sends a visible answer. `input.respond` gains an optional
  `manual` flag, false when absent: an answer that the user chose to type for a
  command that reported no wait, which the daemon takes while that call's command runs
  and a job of it holds the terminal, without the check against a reported wait and
  its kind. An answer after the call's `tool_call_completed` gets `not_found`. The
  fixtures `events/tool_call_input_changed.json` (now a `visible` wait that looks
  secret) and `input_respond_params.json` (now a manual visible answer) set the new
  flags.
- Before version 1 ships: the project registry. New wire type `project_info` (`id`,
  `root` and an optional `name`). New method `projects.list` (scope `read`, no params)
  whose result carries the registry `file` and its `projects`. New methods
  `admin.project_add` (scope `admin`; params `path`, an optional `name` and a
  `git_root` flag, false when absent) and `admin.project_remove` (scope `admin`; params
  `path`), whose results carry the `project`, the `file` that was written and the
  `reload` that followed, in the form of the `admin.config_reload` result. A broken
  file or project is `invalid`, a second registration or a file that changed during the
  write is `conflict`, and a root that no project has is `not_found`. New fixtures
  `projects_list_params.json`, `projects_list_result.json`,
  `admin_project_add_params.json`, `admin_project_add_result.json`,
  `admin_project_remove_params.json` and `admin_project_remove_result.json`.
- Before version 1 ships: the `tool_call_started` event gains an optional
  `manual_input` flag, false when absent: the call takes an input that the user chooses
  to type while it reports no wait (`input.respond` with `manual`). A client offers such
  an input only for a call with the flag. A `shell` call has it unless its command types
  into a shell that reads command lines (`nested_shell`, or a command that starts a
  shell or a REPL), and the daemon now refuses a manual answer for such a call with
  `conflict`, and for a job in the foreground that its last look at the command did not
  see. The `events/tool_call_started.json` fixture now sets the flag.
- Before version 1 ships: the `approval_requested` event gains an optional
  `interactive` flag, false when absent: the call may wait for input at the terminal,
  such as a `sudo` password. A client whose user approves such a call with a key may
  keep the keys typed while it runs, for the answer it asks for. The
  `events/approval_requested.json` fixture now sets the flag.
- Before version 1 ships: the wire types of the `auto` sandbox (phase 1). New wire
  types `launch` (`direct`, `contained` with optional `grants`, `unsandboxed`),
  `grant` (`write`, `host`, `open_network`, `socket`, `bus`, `device`, `unmask`),
  `exit_kind` (17 kinds, frozen in `exit_kinds.json`), `exit_source`, `needs`,
  `exit_info`, `mode_fallback`, `sandbox_status`, `sandbox_check`, `sandbox_paths`,
  `sandbox_summary`, `blocked`, `surface_change`, `reported_file`, `question_id`, and
  the classifier's `exit_record` with its facts and `judgement`. `turn_started`
  settings gain an optional `fallback` (`asked` and `reason`). `tool_call_started`
  gains an optional `launch`, `tool_call_completed` an optional `sandbox` summary
  (names only), and `approval_requested` an optional `exit`. Absent means not
  reported, as before. New event kinds `exit_requested`, `exit_judged`,
  `sandbox_surface_changed`, `surface_question_requested`,
  `surface_question_answered`, `turn_surface_report` and `sandbox_unavailable`. New
  methods `sandbox.explain` (scope `read`; params `path` and an optional `cwd`),
  `sandbox.surface_respond` (scope `approve`, a write with a `command_id`; params
  `conversation_id`, `question_id` and `keep`) and `admin.sandbox_check` (scope
  `admin`, no params). The `admin.status` result gains an optional `sandbox` status
  and optional `sandbox_paths`. The daemon answers the three new methods from phase 1;
  `sandbox.explain` needs only the `read` scope.
  The fixtures `admin_status_result.json`, `events/turn_started.json`,
  `events/tool_call_started.json`, `events/tool_call_completed.json` and
  `events/approval_requested.json` now set the new members; `exit_kinds.json`,
  `grants.json` and `launches.json` freeze the new closed sets.
- Before version 1 ships: `sandbox_summary` gains an optional `survivors` list, empty
  when absent: the programs of an approved exit's descendants that efr could not end,
  such as a process that `sudo` left running as root. The daemon then closes the
  conversation's hidden shell, and the next call starts a new one. The
  `events/tool_call_completed.json` fixture now sets the list.
- Before version 1 ships: `sandbox_summary` gains an optional `setup_error`: why the
  sandbox could not start, such as bubblewrap's setup message. The command did not run,
  and the daemon checks the sandbox again. A client shows it on its own line. The
  `events/tool_call_completed.json` fixture now sets it.
- Before version 1 ships: the `tool_call_completed` event gains an optional `refusal`:
  why efr refused the call before it ran, in a few words, such as `efr's config
  (floor)`. It is absent for a call that ran and for one that the user denied. A client
  shows it in place of a plain "failed". The `events/tool_call_completed.json` fixture
  now sets it.
- Version 1, additive: drafts. `conversation.subscribe` takes an optional `drafts`
  flag, false when absent. With it, the stream also carries items of the new kind
  `draft` (`turn_id`, `after_seq`, and a `draft` object): what a running turn got from
  the model before the event log has it. The `draft` object is `text` (`index`,
  `offset`, `delta`, as in `assistant_message_updated`), `reasoning` (`offset` in the
  reasoning of the turn, `delta`, an optional `title`) or `tool_input` (`call`, the
  position of the call in the answer, `tool`, and `bytes` of input so far). `after_seq`
  is the last event that the turn recorded before the draft. A draft has no sequence
  number of its own, the daemon never stores it, drops it when the queue of the
  subscriber is full, and never closes a subscription for it. A client without the
  flag gets no drafts. The `conversation_subscribe_params.json` fixture now sets the
  flag; the new `conversation_subscribe_item_draft.json` and `draft_parts.json` freeze
  the item and every kind of part.
- Version 1, additive: what the agent changed in files. New types `file_change`
  (`path` as a client shows it: relative to the turn's project root, under `$SCRATCH/`,
  under `~/` or absolute; `kind`, one of `added`, `modified`, `deleted` and `renamed`;
  an optional `from` for a rename; `added` and `removed` line counts; `binary`, false
  when absent) and `file_changes` (`files`, at most 50 and sorted by path, `more`, the
  files left out, and the `added` and `removed` totals). The `tool_call_completed`
  event gains an optional `changes` (a file tool's write, or what changed while a
  `shell` call ran in the turn's project, `$SCRATCH` and, in `auto`, the projects that
  the line names, also a file that another conversation or the user changed then)
  and an optional `diff` (the unified diff of a file tool's write, at most 2000 lines
  and then `... N more lines`). The `turn_completed` event gains an optional `changes`:
  the turn's first snapshot against its last, with the same caveat. New method `conversation.diff` (scope
  `read`, no command id) with an optional `conversation_id` (absent: the conversation
  of the named turn, else the active conversation of the hello's terminal), an
  optional `turn_id` (absent: the newest turn of the conversation that started and
  ended, not a cancelled one) and a `stat`
  flag, false when absent; its result has the `turn_id`,
  the `changes` and, without `stat`, the unified `diff`, at most 20000 lines, where a
  changed ignored file such as `.env` has only a line `<path>: ignored file, content
  not shown`. The
  fixtures `events/tool_call_completed.json` and `events/turn_completed.json` now set
  the new members; `conversation_diff_params.json` and `conversation_diff_result.json`
  are new.
- Version 1, additive: steer, queue and withdraw from the input row of a turn.
  `turn.steer` takes an optional `if_late` object, absent by default: `{kind: "queue"}`
  with an optional `context`, `last_command` and `settings`, as in `prompt.send`. A
  steer is late when the turn makes no more model calls: the model answered without a
  tool call, an interrupt was requested, `turn_id` names a turn that does not run, or
  no turn runs. Without `if_late` a late steer gets `conflict`, as before. With it, the
  daemon records `prompt_queued` (with the steer's `command_id`) instead of
  `turn_steered`, and the result has `queued: true`, the new `turn_id` and the `seq` of
  `prompt_queued`. A late steer is never recorded as `turn_steered`. The `turn.steer`
  result gains `queued`, false when absent. New event kind `steering_delivered`
  (`turn_id` and the `steers`, the seqs of the `turn_steered` events that a model call
  read). New method `prompt.withdraw` (scope `operate`, a write with a `command_id`;
  params `conversation_id` and a `target`: `{kind: "turn", turn_id}` or
  `{kind: "newest_from_tty", tty}`) whose result has the `withdrawn` prompt (`turn_id`,
  `seq` of the event, `text`); a prompt that started gets `conflict`, and a target
  without a waiting prompt gets `not_found`. New event kind `prompt_withdrawn`
  (`turn_id`, `origin`), the last event of a turn that never started. `turn.interrupt`
  takes optional `resend_steers` (seqs of unread `turn_steered` events to send again
  as one prompt that runs next), `resend_as` (`{kind: "queue"}` with the `context`,
  `last_command` and `settings` of that prompt, as in `if_late`; absent: those of the
  interrupted turn), `withdraw_steers` (seqs of unread `turn_steered` events to take
  back) and `withdraw` (turns of queued prompts to withdraw), all done in the step
  that records `turn_interrupt_requested`; its result gains an optional `resent`
  (`turn_id`, `seq`, `steers`), `withdrawn` and `withdrawn_steers` (`seq`, `text`),
  absent when empty. New event kind `steering_withdrawn` (`turn_id`, `steers`,
  `origin`): steers taken back that no model call read.
  The `prompt_queued` event gains `steers`, absent when empty: the steers whose text it
  carries. The fixtures `turn_steer_params.json`, `turn_steer_result.json`,
  `turn_interrupt_params.json`, `turn_interrupt_result.json` and
  `events/prompt_queued.json` now set the new members; `prompt_withdraw_params.json`,
  `prompt_withdraw_result.json`, `events/steering_delivered.json`,
  `events/steering_withdrawn.json`, `events/prompt_withdrawn.json` and
  `withdraw_targets.json` are new.
- Version 1, additive: freeform tools, whose input is plain text, such as the patch
  of `apply_patch`. The `tool_call_started` event gains an optional `freeform` flag,
  false when absent. With the flag, `input` is a JSON string that holds the text as
  the model wrote it. Without it, `input` is the JSON that the model wrote, as before
  (or a JSON string with arguments that were not JSON). A client shows a freeform
  input as text. The `tool_input` draft part counts the bytes of that text in
  `bytes`. The new fixture `events/tool_call_started_freeform.json` sets the flag;
  `events/tool_call_started.json` does not change.
- Version 1, additive: a file tool's call can change several files. The `changes` of
  `tool_call_completed` list every file of an `apply_patch` call, with `deleted` for
  a deleted file and `renamed` (and `from`) for a moved one. Its `diff` holds the diff
  of each file in the order the call changed them, one after the other, each with its
  two header lines (`+++ /dev/null` for a deleted file), together at most 2000 lines.
  No member and no fixture changes.
- Version 1, additive: the context window and its compaction. `usage` gains optional
  `cached_input_tokens` and `reasoning_tokens` (sums over the turn's model calls,
  parts of `input_tokens` and `output_tokens`) and `context_tokens` (the last model
  call's input plus its output), each absent when zero. `turn_failed` and
  `turn_interrupted` gain an optional `usage`. `turn_completed`, `turn_failed` and
  `turn_interrupted` gain an optional `context` (`tokens`, `limit`, `window`): how
  full the model's context was at the end, where `limit` is the point that a client
  shows as 100%. A model of `models.list` gains an optional `context_window`. New draft
  parts `context` (the same three members, before and after each model call) and
  `compacting` (`trigger`). New event kind `conversation_compacted` (`compaction_id`,
  optional `turn_id`, `trigger`: `auto`, `manual` or `overflow`, optional `focus`,
  `model`, `window`, `limit`, `tokens_before`, `tokens_after`, `through_turn`,
  optional `through_message`, `kept_turns`, optional `pruned_outputs`,
  `pruned_tokens`, `omitted_turns` and `omitted_messages` (what the summary never
  saw, absent when zero), `summary` and `usage`). New method `conversation.compact` (scope
  `operate`, a command id, `conversation_id`, optional `focus`) whose result has the
  `seq` of the event and the `compaction`. The fixtures `draft_parts.json`,
  `models_list_result.json`, `events/turn_completed.json`, `events/turn_failed.json`
  and `events/turn_interrupted.json` now set the new members;
  `conversation_compact_params.json`, `conversation_compact_result.json` and
  `events/conversation_compacted.json` are new.
- Version 1, additive: the model catalog comes from the provider's backend. A model of
  `models.list` gains an optional `max_context_window` (the largest window that
  `[openai] models` can set) and `prefer_websockets` (false when absent), and its
  `context_window` is the window that efrd uses. The source `builtin` now means the
  provider's catalog. The result of `models.list` and of `admin.status` gains an
  optional `catalog` (`origin`: `backend`, `cache` or `builtin`, and an optional
  `fetched_at`). The fixtures `models_list_result.json` and `admin_status_result.json`
  now set the new members.
- Version 1, additive: cache writes. `usage` gains optional `cache_write_tokens` (the
  part of `input_tokens` that the provider wrote to its prompt cache) and
  `cache_write_1h_tokens` (the part of `cache_write_tokens` written with a time to live
  of one hour), both sums over the turn's model calls and absent when zero. For every
  provider, `cached_input_tokens` and the two write counts are parts of `input_tokens`.
  The fixtures `events/turn_completed.json`, `events/conversation_compacted.json` and
  `conversation_compact_result.json` now set them.
- Version 1, additive: the stored fresh context block. `Compaction` (in
  `conversation_compacted` and in the result of `conversation.compact`) gains an
  optional `fresh`: the fresh context block that every request after a compaction with
  a summary sends before the summary, as efrd read it when the compaction ended. It is
  absent for a compaction without a summary and for one from before the field. The
  fixtures `events/conversation_compacted.json` and `conversation_compact_result.json`
  now set it.
- Version 1, additive: logins with an API key. New method `admin.login_api_key` (scope
  `admin`, unary, no command id) with `provider` (`openai-api` or `anthropic-api`),
  `key` (a plain string that the daemon never logs) and `check` (true when absent),
  whose result has the `provider`, the `key_hint` (the known prefix and the last four
  characters of the key, such as `sk-ant-...a1b2`), `checked` (false when the check was
  skipped) and `active` (true when the provider is the one of new conversations). New
  method `admin.logout` (scope `admin`, unary, no command id) with `provider`, whose
  result has the `provider` and `logged_out` (false when nothing was stored). A
  provider of the `admin.status` result gains `active` (false when absent), an optional
  `login` (`subscription` or `api_key`) and an optional `key_hint`, and the daemon now
  lists `anthropic-api` too. New fixtures `admin_login_api_key_params.json`,
  `admin_login_api_key_result.json`, `admin_logout_params.json` and
  `admin_logout_result.json`; `admin_status_result.json` now sets the new members.
- Version 1, additive: the catalog of each provider. The `catalog` of the `models.list`
  and `admin.status` results gains an optional `provider`, the provider whose catalog
  it is, absent from an earlier daemon. `origin` gains `missing`: no list yet, because
  no fetch worked, no cache is on disk and the provider has no list built into efr
  (`anthropic-api`). The fixtures `models_list_result.json` and
  `admin_status_result.json` now set `provider`.
- Version 1, additive: refused keys and logouts. A provider of the `admin.status`
  result gains an optional `key_refused_at`: when the provider last refused its stored
  API key (a 401 on a model call or on the fetch of the model list). It is absent while
  the key works, after a new login and from an earlier daemon. The result of
  `admin.logout` gains `active` (false when absent): true when the provider is the one
  of new conversations, whose turns fail until a new login. New event kind
  `logout_completed` with the `provider` only; it belongs to no conversation, like
  `login_completed`. New fixture `events/logout_completed.json`;
  `admin_status_result.json` and `admin_logout_result.json` now set the new members.
- Version 1, additive: the classifier, presence, cache deletes and floor file backups of
  the auto mode (phase A). New event kinds `exit_review_started` (`turn_id`, `call_id`,
  `kinds`), `classifier_paused` (`turn_id`, `reason`: `in_a_row` or `per_turn`,
  `denials`), `presence_changed` (`conversation_id`, `presence`: `present` or `away`,
  `source`: `input`, `user` or `timeout`; it belongs to no turn),
  `cache_delete_applied` (`turn_id`, `call_id`, `paths`, `freed_bytes`) and
  `home_file_changed` (`turn_id`, `call_id`, `path`, `floor`, optional `covered` and
  `backup`). `approval_expired` gains `away` (false when absent). `exit_judged` gains an
  optional `chain_step` (`classifier_model`, `turn_model` or `user`) and `skipped` (the
  models that the chain skipped, absent when none), and its `judge` can be
  `presence_expired`. In the exit record, `action` gains an optional `edit` (the payload
  of a file tool edit of a floor file, at most 8 KiB), a target gains
  `backup_before_write` and `masked` (a path that the sandbox hides, such as a
  credential store; both false when absent), `facts` gains `denials_in_a_row` and
  `denials_this_turn` (0 when absent), and a target's `class` can be `cache`,
  `build_output` or `trash`. An exit's `source` can be `cache_delete`: a delete in a
  real cache that a contained call made. The `sandbox` of `tool_call_completed` gains
  `cache_deletes` and `cache_deletes_more`, absent when none. New fixtures
  `events/exit_review_started.json`, `events/classifier_paused.json`,
  `events/presence_changed.json`, `events/cache_delete_applied.json`,
  `events/home_file_changed.json` and `events/exit_requested_floor_edit.json`;
  `events/approval_expired.json`, `events/exit_judged.json`,
  `events/exit_requested.json` and `events/tool_call_completed.json` now set the new
  members.
- Version 1, additive: presence. New method `presence.set` (scope `operate`, unary,
  with a command id) with an optional `conversation_id` (absent: each conversation that
  had input since the daemon started) and `presence` (`present` or `away`), whose result
  has the `presence`, the `changed` conversations (absent when none changed) and the
  `seq` of the last `presence_changed` (absent when none changed). New fixtures
  `presence_set_params.json` and `presence_set_result.json`.
- Version 1, additive: desktop grants. A `grant` can be `desktop` with a `desktop`
  (`wayland`, `x11`, `hyprland`, `sway`, `i3` or `niri`): a bind of the one control
  socket of the desktop of the prompt's terminal. A `shell_context` gains an optional
  `desktop` with the optional members `wayland_display`, `display`,
  `hyprland_instance_signature`, `swaysock`, `i3sock` and `niri_socket`, the values of
  these variables in the user's shell, and `has_xauthority` (true when the shell has
  `$XAUTHORITY`, never its path; false when absent); absent when the shell has none of
  the six variables. `grants.json`
  now holds a desktop grant of each desktop, and `prompt_send_params.json`,
  `turn_steer_params.json`, `turn_interrupt_params.json` and
  `events/prompt_queued.json` now set `desktop`.
- Version 1, additive: why a turn stopped. `turn_completed` gains an optional `stop`
  with a `reason` (`end_turn`, `max_tokens`, `refusal`, `content_filter` or
  `pause_turn`) and `empty` (false when absent): the last model call gave no text and
  no tool call. It is absent for a normal answer and in turns recorded before it.
  `events/turn_completed.json` now sets it.
- Version 1, additive: labels of exits and changed files. In the exit record, `facts`
  gains `outside_commands` (the commands of an `outside` exit that run outside the
  sandbox, absent when none) and `desktop_read` (a desktop command that only reads
  the desktop's state or reloads its config; false when absent). `home_file_changed`
  gains an optional `real`: the real file when `path` reaches it through a link.
  `events/exit_requested.json` and `events/home_file_changed.json` now set the new
  members.
- Version 1, additive: undo of a turn (phase B). New method `turn.undo` (scope
  `operate`, unary, with a command id) with an optional `conversation_id` and
  `turn_id` and the flags `dry_run`, `force`, `redo` and `list` (false when absent).
  Its result has the `conversation_id`, the optional `turn_id`, `title`, `undo_id` and
  `trash`, the flags `applied`, `stopped` (a write failed after the undo changed
  files) and `resumed`, and the lists `entries` (each with an
  `action`: `restore`, `trash`, `delete`, `skip`, `not_covered` or `command`, a `path`
  and an optional `reason`), `entries_more`, `refs` (each with `root`, `name` and the
  optional `from`, `to`, `applied` and `reason`), `notes` and `turns` (with `list`: each
  with `turn_id` and the optional `title`, `auto`, `undone`, `stopped` and `ended`),
  each absent when empty. A refusal is a `conflict` error whose `data` holds the
  refusal as `refused` (`{"kind": "off"}` until undo comes); `undo_refusals.json`
  freezes its forms. New event kinds `snapshot_taken` (`turn_id`, `root`, `phase`: `pre` or
  `post`, `commit`, `files`, `skipped`, `ms`) and `undo_applied` (`conversation_id`,
  `undone_turn_id`, `undo_id`, `redo`, `origin`, `restored`, `trashed`, `deleted`,
  `conflicts`, `not_covered`, optional `refs` and `trash`, and `stopped`, false when
  absent; it belongs to no turn).
  `turn_completed`, `turn_failed` and `turn_interrupted` gain an optional `undo` (the
  optional `undoable`, `roots` with `shown`, `files` and `refs` (0 when absent),
  `home_files`, `not_covered`
  and `not_covered_more`), and `turn_failed` and `turn_interrupted` gain an optional
  `changes`. From phase B, `home_file_changed` has `covered` and no `backup`. New
  fixtures `turn_undo_params.json`, `turn_undo_result.json`,
  `events/snapshot_taken.json`, `events/undo_applied.json`, `undo_actions.json`,
  `undo_reasons.json` and `undo_refusals.json`; `events/turn_completed.json`,
  `events/turn_failed.json` and `events/turn_interrupted.json` now set the new members.
- Version 1, additive: background jobs (phase C). New wire types: `job_id` (a string
  `j1`, `j2`, ... per conversation; any other string is refused), `job_state`
  (`running`, `exited`, `stopped`, `timed_out`, `failed`), `job_stop_by` (`model`,
  `user`, `time_limit`, `daemon_stop`, `daemon_restart`, `guard`), `notice_kind`
  (`exited`, `watch`) and `held_reason` (`wake_limit`, `wake_turn_job`,
  `daemon_restart`, `wakes_off`, and `not_auto`, which only older builds record). A reader takes a value of these enums that it does
  not know as `unknown`. A `job_info` has `id`, `conversation_id`, `turn_id`, `call_id`,
  `command`, `launch`, `state`, `started_at`, `ends_by`, the optional `ended_at`,
  `exit_code`, `stopped_by` and `watch` (`pattern`, `max_wakes`), `output`,
  `output_bytes` and `wakes` (0 when absent). New method `jobs.list` (scope `read`,
  unary, an optional `conversation_id`; the result's `jobs` are newest first, at most
  200, absent when none) and `jobs.stop` (scope `operate`, unary, with a command id, an
  optional `conversation_id` and a `job_id`; the result has the `job`). New event kinds
  `job_started` (`turn_id`, `call_id`, `job`, `launcher_pid`, `launcher_start`),
  `job_watch_matched` (`job_id`, `line`, `wakes_left`), `job_ended` (`job_id`, `state`,
  the optional `exit_code` and `stopped_by`, `output_bytes`, `ran_secs`, `tail`, and the
  optional `surface_changes`, `quarantined`, `survivors`, `launcher_lost` and
  `output_truncated`), `job_notice` (`conversation_id`, `turn_id`, `job_id`,
  `notice_kind`, `woke`, `source_seq`, `not_user_input`, always true, and `text`) and
  `job_notice_held` (`job_id`, `source_seq`, `reason`). `job_started` and `job_notice`
  belong to their turn; the other three belong to no turn. In the exit record, `action`
  gains `background`, `watch` and `wake_turn` (false when absent) and `max_minutes`
  (absent when the call starts no job). New fixtures `jobs_list_params.json`,
  `jobs_list_result.json`, `jobs_stop_params.json`, `jobs_stop_result.json`, the five
  `events/job_*.json`, `job_states.json`, `job_stop_causes.json`, `held_reasons.json`
  and `notice_kinds.json`; `events/exit_requested.json` now sets the new members.
- Version 1, additive: the network view (phase D1). New grant kind `net_view` (no other
  member): the call keeps the host's network namespace but can open only Unix and
  netlink sockets. A reader takes a grant kind that it does not know as `unknown`; efrd
  never sends one, and the launcher refuses a call that holds one. The sandbox status
  gains `netview` (false when absent): true when the probe's network view self-test
  passed. It also gains `netview_off` (false when absent, sent only when true): efrd
  sets it in `admin.status` and `admin.sandbox_check` when the user's
  `network.netview` is false. `grants.json` now holds `net_view` and `unknown`, `launches.json` a launch
  with `net_view`, and `admin_status_result.json` and `admin_sandbox_check_result.json`
  set `netview`.
- Version 1, additive: the proxy's refusals and status (phase D2). `block_reason` gains
  `ip_literal`, `not_tls`, `bad_request`, `limit` and `unknown`; a reader from this
  version on takes a reason that it does not know as `unknown`. An `efr` from before
  this version cannot read `ip_literal`, `not_tls`, `bad_request` or `limit`, and fails
  on the frame that holds one. A `blocked` entry gains `count` (1 when absent,
  sent only when it is not 1): how many connections the proxy refused for that host,
  port and reason. `sandbox_summary` gains `blocked_more` (0 when absent, sent only when
  it is not 0), the refusals that `blocked` leaves out (it lists at most 32), and
  `proxied` (false when absent, sent only when true): the call ran with a proxy plan.
  The `job_ended` event gains the same `blocked` list and `blocked_more`, absent when
  empty. A host fact of the exit record gains `project_remote` (false when absent): the
  host of a remote of a pinned project git dir. The sandbox status gains `proxy` (false
  when absent): true when the probe's proxy self-test passed. The new
  `block_reasons.json` freezes every reason;
  `events/tool_call_completed.json`, `events/job_ended.json`,
  `events/exit_requested.json`, `admin_status_result.json` and
  `admin_sandbox_check_result.json` now set the new members.
- Version 1, additive: the bus filter and always-allow (phase E). `approval_decision`
  gains `allow_always`: run the call and save the rule that the question offered. An
  `efr` or a daemon from before this version cannot read it; a turn that does not know
  it denies the call. The exit info of `approval_requested` gains `always` (absent when
  no rule may cover the call): the rule in words; a client offers `allow_always` only
  when it is set. New event kind `always_rule_saved` (`turn_id`, `call_id`, `outcome`),
  which belongs to its turn; `outcome` is an object tagged by `kind`: `saved` (`index`,
  `rule`), `not_saved` (`reason`: `changed`, `read_only`, `load_check`, `io`,
  `reload_timeout`) or `unknown`, and a reader takes a state or a reason that it does
  not know as `unknown`. `approval_resolved` does not change. New grant kind
  `bus_filter` (`bus`): the call's own filtered socket of that bus, a view like
  `net_view`; a reader that does not know it takes it as `unknown`, and a launcher that
  has no plan step for it refuses the call. The sandbox status gains `bus_proxy` (false
  when absent): true when efrd runs the filtered bus proxy, and `bus_proxy_off` (absent
  when the filter is on): `config`, `no_program`, `self_test`, `old_version` or
  `unknown`. `sandbox_summary` and the `job_ended` event gain `bus_refused` (absent
  when empty, at most 16 entries of `bus`, `name`, `member`, `path` and `count`, 1 when
  absent) and `bus_refused_more` (0 when absent). The new fixtures
  `approval_decisions.json`, `always_saved.json`, `not_saved_reasons.json`,
  `bus_proxy_off_reasons.json` and `events/always_rule_saved.json` freeze them;
  `grants.json`, `launches.json`, `events/approval_requested.json`,
  `events/tool_call_completed.json`, `events/job_ended.json`,
  `admin_status_result.json` and `admin_sandbox_check_result.json` now set the new
  members.
- Version 1, additive: full mode. The wire enum `mode` gains `full`, after `auto`: every
  call runs as the user with no sandbox and no question, and efr still reads each line
  for a small set of floors. An `efr` from before this version cannot read a
  `turn_started` or a `prompt.send` result with `mode: full`; `efr` and `efrd` ship
  together. New wire enum `full_refusal` (`root`, `snapshot_off`, `undo_off`,
  `no_zsh`): why the daemon refuses full mode although `permissions.allow_full` is
  true. The `judge` of `exit_judged` gains `full`: full mode let the exit run with no
  review and no question, or a question of full mode expired. `approval_requested`
  gains `expires_in_secs` (absent when the question has no fixed limit), such as 120
  for a delete that full mode guards; the target and the reason stay in `summary`. The
  `admin.status` result gains `full_mode` (absent when not reported), with `allowed`
  (`permissions.allow_full`) and `refused` (a `full_refusal`, absent when the daemon
  does not refuse it). Its `config` object gains `held` (absent when nothing is held):
  a change of `config.toml` that loosens permissions and came while full mode ran, with
  `loosens`, the config keys that loosen. A turn of a full conversation that a job
  notice starts records the fallback reason `a turn that a job notice starts runs with
  at most auto`. The new fixtures `judge_kinds.json`, `full_refusals.json` and
  `events/exit_judged_full.json` freeze them; `modes.json`,
  `events/approval_requested.json` and `admin_status_result.json` now set the new
  members.
- Before version 1 ships: each `turns` entry of a `turn.undo` result gains `full`
  (false when absent): the turn ran in full mode, and `auto` is true too then.
  `turn_undo_result.json` now sets it.
- Version 1, additive: the recent commands of a terminal. `prompt.send` and the
  `queue` object of `turn.steer` `if_late` and `turn.interrupt` `resend_as` gain
  `recent_commands` (absent when empty): the terminal's recent command lines, oldest
  first, each with `command` and the optional `status`, `cwd`, `ended_at` and
  `duration_ms`. Like `last_command`, they reach only the turn's preamble: never a
  `shell_context`, an event or the store. `prompt_send_params.json`,
  `turn_steer_params.json` and `turn_interrupt_params.json` now set them.
- Version 1, additive: `job_stop_by` gains `full_off`, before `unknown`: a call of full
  mode started the job, and a config reload turned full mode off, so efrd stopped it.
  Its notice starts no turn of its own. A reader from before reads it as `unknown`.
  `job_stop_causes.json` now lists it.
- Version 1, additive: the `facts` of an exit record gain `git_code` (absent when
  efr did not use it): for an `outside` or `upload` exit of a git or gh line, the git
  code that the line runs outside the sandbox, when it made the exit one that only the
  user may approve. It holds `checked` (false when efr could not check it) and `items`
  (absent when empty), each with `repo`, `item` (such as `hooks/pre-push` or
  `core.sshcommand`), `file` and `writable` (a contained call can write the file).
  The record keeps only the writable items, and an `item` never holds the user name
  or password of a URL in a config key.
  `events/exit_requested.json` now sets it.
- Version 1, additive: new method `conversation.context` (scope `read`, no command id)
  with `conversation_id` and an optional `model`: what fills the model's context of a
  conversation. Its result has the model and its limits (`window`, `window_known`,
  `trigger`, `hard_cap`, `auto`), the last real count `counted` (absent when no turn
  has one since the newest compaction), the `estimate` of the next request and its
  `parts` (one per kind of the new wire enum `context_part_kind`), the `largest` items,
  the `compactions` and the `cache` use of the conversation's model calls. It holds
  only sizes, counts and labels, never the text of a message. New fixtures
  `conversation_context_params.json`, `conversation_context_result.json` and
  `context_part_kinds.json`.
- Version 1, additive: `block_reason` gains `denied`, before `unknown`: the proxy
  refused a host that the deny list holds, or that efrd denies for the call because a
  file or a credential helper can give the call a token for it. Such hosts were
  `not_allowed` before. No host grant opens them. A reader from before reads it as
  `unknown`. `block_reasons.json` now lists it.
- Version 1, additive: routing by scope. `conversation_created` gains `route` (absent
  in conversations from before, and for a first prompt without a working directory):
  the scope key of its first prompt, the new wire enum `route_scope`
  (`{"kind": "project", "value": "<project id>"}` or `{"kind": "outside"}`; a kind
  that a reader does not know, with or without a value, reads as `unknown`). New event
  kind `terminal_attached` with `tty`, `route`, `shell_pid` (absent when unknown) and
  `how` (`picked_up`, `resumed`, or `unknown`): the conversation became the
  conversation of that terminal's slot for that key. A reader from before reads it as
  `unknown`. The result of `prompt.send` gains `route` (absent for a prompt with no
  terminal or with a conversation id): the `scope` of the prompt (`scope`, `name` of a
  project, `dir`: the project's root or the prompt's working directory), the
  `switch` (absent when the conversation did not change, at a terminal's first prompt
  and at `,new`; `back` and `picked_up` with `title` and `updated_at`, `new` with
  `from` and a `reason`: `scope`, `idle` with `hours`, or `moved` with `tty` and
  `title`) and `hint` (a git repository that is not a project; absent for now). The
  params of `conversations.list` gain `tty`: only that terminal's conversations, its
  current one first, in one page; a daemon from before ignores it. New fixtures
  `events/terminal_attached.json`, `route_scopes.json`, `route_switches.json` and
  `attach_hows.json`; `events/conversation_created.json`, `prompt_send_result.json`
  and `conversations_list_params.json` now set the new members.
- Version 1, additive: `exit_judged` gains `reasked` (false when absent): the
  classifier model wrote no real reason (empty, too short or a placeholder), and the
  judge asked it once more. `events/exit_judged.json` now sets it.
- Version 1, additive: the result of `admin.config_reload` gains `warnings` (absent
  when empty): what the reload changed that the user should know, one sentence each,
  such as a `[model] name` of another company, an always rule that a floor now
  disables, or a bus filter that is now off. Each comes once, from the reload that
  changed it; a held change stays in `error`. `admin_config_reload_result.json` now
  sets it.
- Version 1, additive: each `providers` entry of the `admin.status` result gains
  `key_checked_at` (absent when unknown): for an API key, when the provider last
  answered a request with the key since the daemon started, refused or not, or when a
  login checked it. `admin_status_result.json` now sets it.
- Version 1, additive: resuming a conversation (`,resume`, `efr resume`). New method
  `conversations.recent` (scope `read`, no command id) with an optional `tty`, `pwd`
  and `limit`: the recent conversations, the ones of the scope key of `pwd` first,
  each part newest first. Its result has the `scope` of `pwd` (absent without one),
  the terminal's `current` conversation (absent without a `tty` or when it holds
  none) and `conversations`, each with its `summary`, `route`, `project` (the name;
  absent outside projects and for a removed project), `turns` (prompt turns only),
  `open_in` (another open terminal that holds it; absent when none) and `here` (false
  when absent). New method `conversation.resume` (scope `operate`, with a command id)
  with `conversation_id`, the terminal's `context` and `take` (false when absent): the
  conversation becomes the conversation of the terminal's slot for its own scope key,
  and efrd records `terminal_attached` with `how` `resumed`. Its result is `resumed`
  (`conversation_id`, `scope`, `title`, `updated_at`, `shell_dir`, `here` and
  `continues_here`, each flag false when absent) or `held` (`tty` and `title`:
  another open terminal holds it and `take` was not set, so nothing moved); a kind
  that a reader does not know reads as `unknown`. A daemon from before refuses both
  as frames that are not valid. New fixtures `conversations_recent_params.json`,
  `conversations_recent_result.json`, `conversation_resume_params.json`,
  `conversation_resume_result.json` and `conversation_resume_result_held.json`.
- Version 1, additive: port forwards of contained jobs. New event kind
  `job_port_forwarded` (`job_id`, `listen`, the address that the job listens on as
  text such as `127.0.0.1` or `::1`, `port`, `outcome`, and `by_job` and `reason`,
  each absent when not set). It belongs to the conversation, not to a turn. The new
  wire enum `outcome` is `forwarded`, `in_use`, `refused`, `limit` or `stopped`; a
  value that a reader does not know reads as `unknown`. `by_job` names the job of the
  same conversation that holds the port of an `in_use` outcome; `reason` is a short
  text for `refused` and `limit`, such as `a service port`. `JobInfo` gains `forwards`
  (absent when empty), each with `listen` and `port`, so `jobs.list` and `jobs.stop`
  show the ports that efr forwards for a job. New fixtures
  `events/job_port_forwarded.json` and `forward_outcomes.json`;
  `jobs_list_result.json` and `jobs_stop_result.json` now set `forwards`.
- Version 1, additive: a command left running in the hidden shell. Three new event
  kinds of the conversation, not of a turn: `shell_command_detached` (`call_id`,
  `command`, the first 200 characters of the line, `since`, `contained`, `input`, and
  `looks_secret`, `prompt` and `approved_by`, each absent when not set): the command
  of a `shell` call still runs after its call answered the model, and the shell takes
  no other call until it ends; `shell_command_input_changed` (`call_id`, `input`,
  `contained`, and `looks_secret` and `prompt`, each absent when not set): that
  command began or stopped waiting for input; `shell_command_ended` (`call_id`, `how`,
  and `exit_code`, absent when not known). `prompt` is the row of the screen that
  holds the cursor, at most 200 characters. The new wire enum `how` is `ended`,
  `stopped`, `limit`, `closed`, `shell_exited` or `moved`, and the new wire enum
  `approved_by` is `you`, `your_rule`, `reviewer` or `full_mode`; a value that a
  reader does not know reads as `unknown`. `tool_call_completed` gains `left_running`
  (false when absent): true when the call answered while its command still runs. The
  detach event and `tool_call_completed` come in either order; a client links them by
  `call_id`. New fixtures `events/shell_command_detached.json`,
  `events/shell_command_input_changed.json`, `events/shell_command_ended.json`,
  `shell_command_ends.json` and `approved_bys.json`;
  `events/tool_call_completed.json` now sets `left_running`.
- Version 1, additive: the hidden shell's state and a stop of a command left running.
  New method `shell.interrupt` (scope `terminal`, with a command id) with
  `conversation_id`, `call_id` and `close` (false when absent): SIGINT to the group of
  the command that the call left running, or with `close` a close of the hidden shell.
  Its result is empty. New method `shell.status` (scope `read`, no command id) with
  `conversation_id`. Its result has `busy` (absent when the shell is free): `call_id`,
  `command`, `since`, `contained`, `input`, and `looks_secret`, `prompt` and
  `approved_by`, each absent when not set. A daemon from before refuses both as frames
  that are not valid. New fixtures `shell_interrupt_params.json`,
  `shell_interrupt_result.json`, `shell_status_params.json` and
  `shell_status_result.json`.
- Version 1, additive: the move of a running `shell` call to a background job (Ctrl+B
  in `efr`). New method `call.background` (scope `terminal`, with a command id) with
  `conversation_id`, `call_id` and `confirm` (absent when not set). Without `confirm`,
  a call in the sandbox moves at once, and a call outside the sandbox does not move:
  the result is a `preview` (`token`, `command`, `max_minutes`, and `exits`, absent
  when empty), and the client asks the user. A yes sends the method again with the
  token in `confirm`. The result is `moved` (`job_id`) or `preview`; a kind that a
  reader does not know reads as `unknown`. A refusal is `invalid` with a short reason.
  `JobInfo` gains `moved_from` (absent for a job that the model started):
  `call_id`, `turn_id`, `after_secs`, `exits` (absent when empty) and `launcher`
  (true when the call ran through efr's sandbox launcher; absent when false), and
  `waits_input` (true while the command of a running moved job waits for input that is
  not a secret; absent when false). A daemon from before refuses the method as a frame
  that is not valid. New fixtures
  `call_background_params.json`, `call_background_result.json`,
  `call_background_result_preview.json` and `events/job_started_moved.json`.
- Version 1, additive: the search of conversations. New method `conversations.search`
  (scope `read`, no command id) with `query` and, each absent when not set, `tty`,
  `pwd`, `limit` (50 when absent, 200 at most) and `stats` (false when absent). Its
  result has `rows`, best first: each row has `conversation` (a row of
  `conversations.recent` whose `summary.title` is the title with its secrets
  replaced), `short_id`, and `title_ranges` and `hit`, absent when not set. A hit has
  `kind` (`title`, `prompt`, `answer` or `command`; a kind that a reader does not know
  reads as `unknown`), `seq`, `turn_id` (absent outside a turn), `at`,
  `history_cursor` and `lines`, each a `text` with its `ranges`. A range is a half-open
  pair of Unicode code points, `start` and `end`. The result also has `scope`,
  `current`, `partial` (`row_cap` or `time_cap`; an unknown reason reads as
  `unknown`), `index` (`percent`) and `stats` (`rows_scanned`, `conversations_scored`,
  `titles_scored`, `elapsed_us`), each absent when not set. New method
  `conversation.search` (scope `read`, no command id) with `conversation_id`, `query`
  and `limit` (20 when absent, 50 at most). Its result has `hits`, newest first, and
  `title` and `partial`, absent when not set. efrd refuses both methods to a peer that
  is not the user's, and announces them with the new capability `search` in `hello`.
  A daemon from before refuses both as frames that are not valid. New fixtures
  `conversations_search_params.json`, `conversations_search_result.json`,
  `conversation_search_params.json`, `conversation_search_result.json`,
  `search_kinds.json` and `search_partials.json`; `hello_result.json` now sets
  `search`.
- Version 1, additive: the cgroup of a background job (ADR 0021). `JobInfo` gains
  `cgroup` (absent from an efrd before 0.0.5) with `off` (why the job has no cgroup of
  its own), `memory_bytes` (now while the job runs, the most at its end) and
  `cpu_usec`, each absent when not set. The fixtures `jobs_list_result.json`,
  `jobs_stop_result.json`, `events/job_started.json` and
  `events/job_started_moved.json` now set it.
- Version 1, additive: `call.background` also takes the id of a call whose command was
  left running in the hidden shell, and moves that command, with the same preview and
  the same refusals. The command's `shell_command_ended` then has `how: moved`, which
  the enum had already. A daemon from before refuses such a call as one that does not
  run. No fixture changes.
- Version 1, additive: the delete of a conversation. New method `conversation.delete`
  (scope `terminal`, with a command id) with `conversation_id`, and `force` and `stop`
  (each false when absent). Its result is `deleted` (`conversation_id`, and `title`,
  `jobs_stopped` and `not_removed`, each absent when not set) or `held` (`tty`, and
  `title` when set) when another open terminal holds the conversation and `force` is
  not set; a kind that a reader does not know reads as `unknown`. A running turn, a
  queued prompt or a compaction refuses it as `busy` unless `stop` is set, and an undo
  that runs refuses it as `busy` always; the error's `data` is `{"busy": "turn"}` or
  `{"busy": "undo"}`. A conversation that does not exist, or that is deleted, is
  `not_found`. New event kind `conversation_deleted` with `origin`: the last event of
  the conversation and the only one that stays in the log; a subscription of the
  conversation ends after it. A daemon from before refuses the method as a frame that
  is not valid. New fixtures `conversation_delete_params.json`,
  `conversation_delete_result.json`, `conversation_delete_result_held.json` and
  `events/conversation_deleted.json`.
