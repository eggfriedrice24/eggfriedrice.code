# efr-conversation

## Purpose

The conversation engine: one actor per conversation that queues prompts and runs
turns.

- `ConversationActor::spawn` starts the actor of one conversation and returns its
  `ConversationHandle`, the only way in. The actor reads its settings from a
  `ConfigSource` (the daemon's settings watch; a `watch::Receiver` of
  `ConversationConfig` is one too): a turn takes the latest settings when it starts
  and keeps them until it ends, so a change never reaches a running turn's model,
  prompt or limits; each tool call reads the approval timeout and the update interval
  when it starts; a prompt reads the queue limit when it arrives. `ConversationStart::New` records
  `conversation_created` together with the first prompt, so a conversation never
  exists without one; `ConversationStart::Existing` picks up a conversation from the
  log, as after a restart.
- `send_prompt` records `prompt_queued` with the command's receipt. A prompt runs at
  once when the conversation is idle; a second prompt queues behind the running turn
  (`ConversationConfig::max_queued`, 16 by default). The last command of `prompt.send`
  stays in memory and reaches only the turn's preamble; it never enters an event.
- Turn settings (`settings.rs`): a prompt may ask for a mode, a model and an effort.
  `send_prompt` resolves them over the config's defaults (`ConversationConfig::mode`,
  `model`, `effort`) and checks them against the model list
  (`ConversationConfig::models`; an empty list takes any model): a model outside the
  list, or an effort the model does not take, is `InvalidSetting` with the choices,
  and nothing is recorded. `prompt_queued` keeps what the prompt asked for, so a queued
  prompt keeps it; the result carries the effective settings. When the turn starts it
  resolves them again against the settings of that moment, fails with `invalid` and
  the choices when they no longer fit, and records them on `turn_started`. A turn
  from a remote origin runs with at most `cautious`. The turn sends its model, its
  effort as the request's `effort` (in the manual compaction's summary request too),
  and its mode in the permission
  engine's `DecisionInput` and as a line of the preamble.
- `steer` records `turn_steered`. Before its next model call the turn records
  `steering_delivered` with the seqs of the steers it takes, then sends their texts to
  the model. A turn that would end with steering waiting makes one more call. A steer
  is late when no model call of the turn would read it: the turn made its last model
  call and closed its steering, an interrupt was asked for, the steer names another
  turn, or no turn runs. A late steer is never recorded as `turn_steered`. Without
  `if_late` it gets `NoRunningTurn` (or `TurnMismatch`). With `if_late: queue` the
  actor records it as `prompt_queued` in the same step, with the steer's command id,
  the origin of the caller and the context and settings of `if_late`, and the result
  says `queued`. The turn waits for a steer that the actor is recording at that moment
  and reads it, so a steer is either read or late, never lost.
- `interrupt` is two-phase: the actor records `turn_interrupt_requested`, the turn
  drops the provider's stream (or the parked approval, or the running tool call, which
  the toolbox is asked to `cancel`), completes the text that streamed so far, and only
  then records `turn_interrupted`. Esc in the input row of `efr` lists the prompts that
  its view queued (`withdraw`) and its unread steers (`resend_steers`); Ctrl+C lists
  the prompts and the unread steers to take back (`withdraw_steers`). In the step and
  the append that record `turn_interrupt_requested`, the actor withdraws the listed
  prompts that still wait (`prompt_withdrawn` each, in queue order) and takes the
  listed steers that no model call took yet out of the turn's steering. It records
  the steers taken back as one `steering_withdrawn`, and the steers to send again as
  one `prompt_queued` (texts joined by newlines, `steers` set) that runs next, before
  the prompts that wait. That prompt has the context, settings and last command of
  `resend_as` (the terminal that pressed Esc), else those of the interrupted turn's
  prompt. The resend never makes the interrupt fail: its prompt does not count
  against `max_queued`, because the turn took its steers already, and settings of
  `resend_as` that cannot work now give way to the interrupted turn's. Prompts and steers that are not listed stay as they are, so those of other
  terminals stay queued. A steer that the turn took for a model call counts as read,
  also when the interrupt stops that call. The history and the exit record leave out a
  steer that an interrupt sent again or took back.
- `withdraw` takes one queued prompt back, by its turn or as the newest prompt of a
  terminal, and records `prompt_withdrawn`, the last event of that turn. The actor
  starts a queued turn only between requests, so a prompt either starts or is
  withdrawn. A prompt that started, ended or was withdrawn is `PromptNotWaiting`; a turn
  that the conversation never queued is `UnknownTurn`; a terminal with no queued
  prompt is `NoQueuedPrompt`.
- The receipt of a result with sequence numbers inside it (`turn.interrupt`,
  `prompt.withdraw`) is stored without them. The events of a batch have consecutive
  numbers, so `completed_result` puts them back from the receipt's seq.
- The rebuilt history and the user messages of an exit record leave out a steer that
  an interrupt sent again (it counts once, as its prompt) and a withdrawn prompt.
- The rebuilt history puts a steer where the `steering_delivered` that names it is,
  because the model read it at that model call. A steer that no
  `steering_delivered` names is left out: no model call read it (the turn failed, an
  old client interrupted it, or the user took it back). A completed turn without any
  `steering_delivered` comes from a daemon that did not record the event yet, and
  keeps its steers where the user typed them.
- An approval's summary names the tool and what needs approval, the command line
  first. When some simple commands of a line of several ask, a second line names them,
  such as `asks for: hostnamectl, systemctl --failed`: each by its program and at most
  three words after it, cut at a long word or at a word with a quote, a space or
  another character outside letters, digits and `._/:@%+,-`, and with the value of
  `--option=value` left out, so a token on the line is not repeated there. Every other
  line break of the summary is escaped, so a path cannot pass for that line.
- The `auto` mode (`exit.rs`, `questions.rs`, `judge.rs`). The check point runs a shell
  call that the engine contains (`Effect::Contain`) at once with `Launch::Contained`
  in `CallContext::launch`, and `tool_call_started` names the launch. An exit asks the
  user: `exit_requested` with its `ExitRecord` (user messages, the action and efr's own
  facts, never tool output or the model's reason) comes in the same batch before
  `approval_requested`, whose `exit` shows the kinds, the launch, what a "yes" opens,
  efr's facts, the model's reason and whether only the user may approve it. A "yes"
  runs the call with the narrowest launch (`exit::grant`): the plain sandbox for a
  question of a user's `ask` rule or a `destructive` exit, the sandbox plus the union of
  the exits' grants (a write bind, the open network, a socket, a bus, a device, an
  unmask), or `Launch::Unsandboxed` (the exit child) when an exit cannot run in the
  sandbox. `CallContext::exits` carries the approved exits, so the toolbox can make a
  write target first. The answer is recorded as `exit_judged` (judge `user`). Before it
  asks about a launch in the exit child, the check point applies the one-command rule
  (`efr_permissions::exits::unsandboxed_line_problem`): a line that carries more than
  one command gets that text as a tool error with no question, and it is no refusal. A
  floor (`secret`, `config`) refuses the exit before any question and records
  `exit_judged` (judge `floor`); three such refusals in a row without a person's answer
  end the turn with `turn_failed` (`forbidden`, "auto stopped this turn: 3 actions
  were refused in a row. Read the answers, then send a new prompt."). The count lives in the turn, so the model cannot reset it.
- After a call whose `ToolOutcome::sandbox` reports changes to git settings that run
  programs, the turn records them in `sandbox_surface_changed` with the call's
  `tool_call_completed`, and, for the changes that the launcher moved to quarantine,
  asks the user before any other call: `surface_question_requested` with its own
  `QuestionId`. `respond_surface` (`sandbox.surface_respond`) records
  `surface_question_answered` and hands the answer to the turn; only a "keep" from the
  user's own machine moves the changes back, through `Toolbox::restore_quarantine`. A
  phone gets `RemoteSurfaceAnswer`. The approval timeout and an interrupt record the
  answer `keep: false` with no origin and leave the changes in quarantine; the model
  reads what happened after the call's output.
- `auto` needs the sandbox. `settings::resolve` reads the probe's latest
  `SandboxStatus` (`ConversationDeps::sandbox`) and the root of the turn's registered
  project: without an available sandbox, or in a project at the home directory or above
  it, the turn runs as `cautious` and records `EffectiveSettings::fallback` with the
  reason, and the preamble says so. A prompt does not know its project yet, so only the
  turn applies the home rule.
- `ExitJudge` (`ConversationDeps::judge`) is the seam of the classifier of phase 3. In
  phase 1 it is `None` and the user answers every exit.
- `respond_approval` records `approval_resolved` and hands the decision to the parked
  turn. The store refuses an answer to a call that is not pending, so of two racing
  answers only one commits. A parked call that can no longer be answered (the turn was
  interrupted, the optional `approval_timeout` passed) is recorded as
  `approval_expired`.

A turn reads one snapshot of the conversation through the store's readers, derives the
scope from the user's working directory (`ScopeResolver`, `GitScopeResolver` over
`efr_scope::derive` with the registry file read every turn), records `turn_started`
(and `scope_changed` when the user moved), makes sure `$SCRATCH` exists, and builds the
request below. When the user's directory differs from the one of the newest earlier
`turn_started`, the turn first calls `Toolbox::move_shell`, so the hidden shell goes
where the user went; without such a move the shell stays where the model left it. The
request:

1. the system prompt (the static rules, `ConversationConfig::system_prompt`);
2. bounded history (`HistoryLimits`: 50 turns, 4096 events, 512 KiB of message JSON,
   scaled to the model's window, see "Context"), each earlier turn from the actor's
   cache or the saved turn messages, else rebuilt from its events;
3. the newest prompt, whose first block is the live-state preamble regenerated every
   turn: the shell's directory and previous directory, the last command and its exit
   status, the git work tree and branch, home, host, OS, `$SCRATCH`, the hidden
   shell's own directory when it differs, one line when the user moved since the
   previous prompt (where from, and whether the hidden shell moved too or where it
   stayed), the turn's permission mode, model and
   effort, the three modes (`manual`, `cautious`, `auto`) with one line each, which
   is the one place that says what a mode allows, as `docs/permissions.md` does, and
   that only the user changes a terminal's mode, model and effort (`,mode`, `,model`,
   `,effort`, or a bare `mode auto` line in sticky mode) while the settings tool
   changes only the defaults in `config.toml` after an approval, so the model never
   claims to have switched one. These rules ride in the preamble, not the system
   prompt, so a system prompt replaced in `config.toml` cannot drop them;
4. the tool definitions.

It streams the provider and records coalesced `assistant_message_updated` events (at
most one per `update_interval` on the injected clock, the text held back sent when the
interval ends), each with only the text added since the previous one and its byte
offset, then `assistant_message_completed` with the whole text. While text, reasoning and
tool input arrive, the turn also sends drafts (`ConversationDraft`, through
`ConversationDeps::drafts`, a broadcast that the daemon holds): at most one batch per
`ConversationConfig::draft_interval` (16 ms by default, the first at once), each with
the sequence number of the last event the turn recorded before it. A text draft
carries the same `index` and byte offsets as the updates; a reasoning draft carries
the new reasoning at its offset in the reasoning of the whole turn, with the title of
the newest section (the last all-bold line); a tool-input draft carries the tool and the input bytes so far of each
call of the answer. Drafts never reach the log, and a turn that nobody follows live
does no draft work and sets no timer. Earlier turns are
rebuilt from the newest events without `tool_call_output_updated`, so a long command's
progress cannot push them out of the page. Every tool call is recorded
with `tool_call_started`, judged at the check point, run when allowed or approved
(with coalesced `tool_call_output_updated` events, and a `tool_call_input_changed`
event for every change of whether the call's command waits for input, which
`OutputSink::input_changed` reports: never coalesced, and recorded after the output
that came before it, in the same batch when that output was still held back), and
answered with `tool_call_completed`, until the model answers without a tool call. A
change that the tool reports just before it returns is still recorded before the
completion. The turn ends with
`turn_completed` (with the summed usage), `turn_failed` (a provider error mapped to an
`ErrorBody`: 401, missing credentials and a token source that cannot produce a token
to `unauthorized`, rate limits to `busy` with
`retry_after_ms`, an overloaded provider to `busy` without it, an unknown model to `invalid` with the `model`, the rest to
`internal`) or
`turn_interrupted`. The turn builds that batch and hands it to the actor. The actor
clears the running turn first and records the batch after. It answers one request at
a time, so a client that sees the end and at once steers or interrupts gets
`NoRunningTurn`, and a prompt that it sends then starts at once.

`$SCRATCH` is `<scratch root>/<YYYY-MM-DD>-<slug>-<idtail>`: the day the conversation
began, up to five words of its title (lowercase ASCII, at most 48 bytes) and the last
8 hex digits of its id. It is claimed with `efr_stdx::fs::claim_dir` and marked with a
`.efr-conversation` file that holds the conversation id. The name is a function of the
conversation, so after a restart the same names are tried in order and the marked one
is found again; a name that something else holds falls back to 12 digits, the whole
id, then the whole id with `-2` to `-9`. A symbolic link is never taken as scratch.
The directory is made again, under the same name, when it goes missing.

### The tools reach this crate through `Toolbox`

The structure document gives this crate the edge `efr-conversation -> efr-tools`, and
the allowlist in `xtask/src/deps.rs` permits it. But `efr-tools` depends on
`efr-shell`, and the same file forbids `efr-conversation -> efr-shell` through any
chain, so `cargo xtask deps` refuses the edge
(`efr-conversation -> efr-tools -> efr-shell`). The crate therefore defines `Toolbox`,
the registry's shape in its own terms, and the daemon implements it over its
`efr_tools::ToolRegistry` in `efr-daemon/src/tools.rs`. The adapter is a copy, field
by field:

- `definitions`: each `ToolSpec` (`name`, `description`, `input_schema`) as an
  `efr_provider::ToolDefinition`;
- `requirements`: build an `efr_tools::ToolContext` from the `CallContext` (plus the
  home, the clock and the write journal the daemon holds), call
  `ToolRegistry::requirements`, add what each path reaches through a symbolic link
  (`ToolRequirements::with_real_paths`, on the blocking pool), and copy
  `ToolRequirements` into
  `efr_permissions::Requirements` (`paths` with `AccessMode::Read`, `ReadTree` or
  `Write` to `with_read`, `with_read_tree` or `with_write`, then `command`, `network`,
  `interactive`); a `ToolError` becomes its `Display` text for the model. The turn
  asks the toolbox's `shell_cwd` before each call and puts it in
  `CallContext::shell_cwd`, so a command's relative paths are declared from where the
  hidden shell is, which an earlier call may have moved. Once the user approved a call
  whose requirements are `interactive`, the check point sets
  `CallContext::approved_interactive`, from which the daemon lets the call run past the
  model's timeout while someone who can answer follows it;
- `invoke`: `ToolRegistry::invoke` with the `OutputSink` passed through as the
  `ToolOutputSink` (output and input waits; the daemon itself answers whether a person
  can answer hidden input), and `ToolResult` copied into `ToolOutcome`; a `ToolError`
  becomes an error outcome. A call whose `CallContext::launch` uses the launcher runs
  only through it, never typed into the hidden shell; its `result.json` summary goes to
  `ToolOutcome::sandbox`;
- `restore_quarantine`: moves the quarantined changes of a call back when the user
  keeps them; the default keeps no quarantine and moves nothing;
- `turn_report`: the files that the turn changed through the launcher and that run
  code later outside the sandbox; the turn records them as `turn_surface_report` right
  before its terminal event; the default reports none;
- `turn_changes`: the files that the turn changed, from the toolbox's snapshots; the
  turn asks once before its terminal event, whatever the ending, and records the
  answer in `turn_completed`; the default takes no snapshot. A call's own changes and
  a file tool's diff come back in `ToolOutcome::changes` and `ToolOutcome::diff`, which
  the turn records in `tool_call_completed`;
- `cancel`: `ShellSessions::interrupt(conversation_id)` for the shell tool, so an
  interrupted command does not keep running in the hidden shell;
- `preview`: the diff of a `write_file` call, once the tools offer one;
- `takes_manual_input`: `ToolRegistry::takes_manual_input`, which the turn records as
  `manual_input` in the call's `tool_call_started`, so a client offers a manual answer
  (`Ctrl+\` in `efr`) only for a call that takes one.

If the forbidden-edge check learns to accept the path through `efr-tools`, this crate
can depend on `efr-tools` and implement `Toolbox` for `ToolRegistry` itself; nothing
else changes.

### What the daemon does around it

- It completes stored results: a receipt's result is stored without `seq`, and the
  store records the number with the receipt, so a retry is answered with the stored
  result plus `receipt.seq`. A `ConversationError::DuplicateCommand` carries the
  receipt. Rejected commands (an error before anything was recorded) get their
  rejected receipt from the daemon, which owns the mapping to wire errors.
- It reconciles after a restart (cancels in-flight turns, expires pending approvals,
  records queued prompts as not run); the actor starts with an empty queue.
- It routes `prompt.send` to the tty's active conversation and spawns or reuses its
  actor.

### Provider items survive a restart

`provider_raw` must go back unchanged to the model that made it, but no event in
`efr-protocol` carries it. The actor keeps the exact messages of the turns it ran (as
many as the history may carry), each with the provider and the model that answered
it, and uses them while both are the same; with another provider or another model
(a prompt that names one, or a new default in the config) they lose `provider_raw`:
the other model's encrypted reasoning and item ids go, the text, the tool calls and
their results stay, as opencode does. A turn also saves its exact messages in
`efr_store::turn_messages`, in the batch that records its end, and the store keeps the
newest `history.max_turns` turns of each conversation. After a restart the snapshot
reads them back in place of the empty cache, so the next request is the same as
without the restart. Only a turn with no saved messages (one from before the table,
or one whose saved messages cannot be read back) is rebuilt from the events, with the
daemon's call ids and without provider items.

A call of a freeform tool (its input is text, such as a patch) keeps that kind on the
way: `ContentBlock::ToolCall` has `freeform` set in the saved messages, and the turn
records it as the `freeform` flag of `tool_call_started`, which a rebuild from the
events reads back. So the next request sends the call and its result in the form the
model wrote them (for the Responses API, `custom_tool_call` and
`custom_tool_call_output`).

### The history only grows

A provider's prompt cache, and Claude's thinking blocks, need one rule: each request
starts with the messages of the request before it, plus its answer. Only these events
may edit earlier messages: a compaction (a pruning or a summary), another model or
provider, and a change of the system prompt, the tools or the effort.

The efr-daemon test `cache_prefix` measures this on the OpenAI path: the real
`openai-api` provider against a local Responses server, 55 turns and a restart. Before
this design, every request edited the prompt of the turn before it, because the
preamble left that prompt. From the 52nd request on, each request also edited the
oldest turn: the actor and the store kept the exact messages of 50 turns only, so the
oldest turn came back from its events, without its provider items.

The design:

1. The saved prompt is the prompt as the model read it: the `<live_state>` preamble,
   then the user's text, in one user message. Every later request sends it again
   word for word, so an old preamble stays as a record of that moment. The system
   prompt says that only the newest live-state block is current. The preamble shows
   the user's last command with its secrets redacted when it is rendered (the value of
   an assignment to a name that `efr_sandbox::secret_like` matches, and key forms such
   as `sk-ant-`, `sk-proj-`, `sk-` and `ghp_`), so the bytes that the store keeps are
   the bytes that went to the model. The events keep only the prompt's text; a turn
   rebuilt from its events has no preamble.
2. The list of earlier turns comes from the conversation's turns
   (`efr_store::conversations::turns`) and the saved messages, never from the
   4096-event page. A turn counts when it started and finished and the newest summary
   does not cover it. Its messages come from the actor's cache, else from
   `turn_messages`, else from its events in the page. Only a turn with no saved
   messages (one from an efr before the table) needs the page.
3. There is no turn limit. `turn_messages` keeps every turn until a compaction with a
   summary covers it, and the actor keeps in memory the turns that the newest request
   carried.
4. The byte limit (`HistoryLimits::max_bytes`, at least twice the window) stays as
   the safety net. When the window would leave out a turn (past the bytes, or a turn
   without saved messages whose start fell out of the page), the guard compacts before
   the model call, and that compaction always writes a summary. Only a turn that
   cannot compact (auto off, or the breaker open) sends the history without that turn,
   with the note `N earlier turns are omitted.`
5. The fresh context block of a compaction is part of its record
   (`Compaction::fresh`). The turn and the manual compaction send that stored text, so
   a restart sends the same bytes. Only a compaction from an efr before this field
   reads the block from disk again, once, and the actor keeps it in memory.

## Context

This section is the contract of context management: the accounting, the guards, the
compaction and its display. The code that builds each part follows it. A change of the
contract changes this text first. The numbers live in `src/context.rs`
(`CompactionConfig`, `ContextLimits` and the constants); the wire types live in
`efr-protocol` (`Usage`, `ContextUse`, `Compaction`, `CompactionTrigger`, the draft
parts `context` and `compacting`, `conversation.compact`).

### Words

- The window: the context window of the turn's model, in tokens. It is the
  `context_window` of the model in `ConversationConfig::models` (the provider's model
  catalog, from the backend, its cache or efr's built-in list, or the model's entry in
  `[openai] models`, which the daemon cuts down to the model's largest window). When
  it is not known, efr counts with `DEFAULT_CONTEXT_WINDOW` (128000).
- The context: the tokens of one request plus its answer.
- The trigger: `auto_at` percent of the window, rounded down (`[compaction] auto_at`,
  default 76; 206720 tokens on a window of 272000). An auto compaction runs at the
  trigger.
- The hard cap: 95% of the window, rounded down (`HARD_CAP_PERCENT`). efr never sends a
  request that it estimates above the hard cap.
- The limit: the trigger when `[compaction] auto` is true, else the hard cap. A client
  shows the context as a percent of the limit, so 100% means "a compaction runs now"
  (or, with auto off, "the next request does not go out").
- The cut: a place in the model's history. The messages before it are compacted. The
  messages after it (the verbatim tail) stay word for word. A cut never falls between a
  tool call and its result: the tail never starts with a message that holds tool
  results.

### Accounting

1. After each model call, the turn keeps the usage of that call next to the sum of the
   turn. `context_tokens` is the input plus the output of the last call. The input
   counts cached tokens too.
2. `turn_completed`, `turn_failed` and `turn_interrupted` carry `usage` (the sums, plus
   `context_tokens` of the last call) and `context` (`ContextUse`: `tokens`, `limit`,
   `window`). `context.tokens` is the last real count plus the estimate of the
   messages after it, such as the results of a call that an interrupt stopped. After
   a completed turn nothing comes after it, so it is the real count. When a
   compaction came after the last call, it is the estimate of the compacted request.
   A turn that ended before it had its settings has no `context`.
3. The turn sends a `context` draft (`DraftPart::Context`) before each model call, with
   the estimate, and after each call, with the real count.
4. Each request carries the conversation id as `prompt_cache_key` unless
   `ConversationConfig::provider_options` names one. OpenAI caches on its own; the key
   sends the requests of one conversation to the same cache. On the subscription
   backend, `efr-provider-openai` also sends the key as the `session-id` header, as
   Codex does.

### The estimate

Before each model call, the turn estimates the context of the request (`Meter` in
`src/context.rs`):

- with a base: the base plus `estimate_tokens` (4 bytes a token, rounded up) of the
  JSON of each canonical message added after it. Inside a turn the base is the input
  plus the output of the last call. At the start of a turn it is the `context.tokens`
  of the newest ended turn in the log, and the new prompt is added to it. The log
  holds it, so it survives a restart;
- without one (the first turn, the first call after a compaction, a newest turn that
  was cancelled or has no end, as after a daemon restart, or a newest ended turn
  without `context`, without a real count in its `usage`, or with a
  `conversation_compacted` after it):
  `estimate_tokens` of the JSON of the whole request (system prompt, tool definitions
  and messages).

Each real count replaces the estimate as the new base. A base that counts turns that
the history no longer sends is too high, never too low, so the turn compacts early
rather than late.

### The guards, before each model call

1. When `auto` is on and the estimate is at or above the trigger (or above the hard
   cap, for an `auto_at` above 95), the turn compacts (trigger `auto`) and then makes
   the call. The turn goes on after the compaction.
2. When the estimate is above the hard cap and the turn cannot compact (auto is off, or
   the breaker stopped it), the turn does not send the request. It fails, see "Failures".
3. When the provider answers `ProviderError::ContextOverflow` (OpenAI
   `context_length_exceeded`, HTTP 413, later Anthropic's `prompt is too long`), the
   turn never retries it as a transient error. With auto on, it compacts once
   (trigger `overflow`, `tokens_before` is the estimate of the refused request) and
   sends the call again once. A second overflow of the same call fails the turn.
   With auto off, the first overflow fails the turn.
4. The breaker: a compaction whose `tokens_after` is at or above the trigger counts as
   a miss, and so does a compaction that frees nothing and records nothing (nothing
   lies before the tail, or the summary request fails). After `BREAKER_TRIES` (2)
   misses in a row, the turn makes no more auto or overflow compactions. It goes on
   while its requests fit under the hard cap. The count of misses carries from turn
   to turn (the actor keeps it in memory), so a context that stays above the trigger
   does not pay for two summaries at the start of each turn. It goes back to 0 when a
   compaction brings the context below the trigger, when a guard finds the estimate
   below the trigger, and after a manual compaction. The client shows the breaker
   line (see "Display").
5. The safety net of `HistoryLimits` stays, scaled to the model's window
   (`HistoryLimits::for_window`): the byte limit is at least twice the window at 4
   bytes a token, and the turn limit at least the event limit. So the net never
   leaves out turns that the window can hold, and the compaction, not the net, makes
   room. When it still leaves out earlier turns that ran and that no summary covers
   (past the most turns or the most bytes, or a turn whose start fell out of the event
   page), the history gets a user message `N earlier turns are omitted.` (`1 earlier
   turn is omitted.` for one) after the summary, or first when there is no summary,
   and the daemon logs one `warn` line with the count. It never leaves out turns
   without that note. A turn that never started, such as one whose settings failed, is
   no omitted turn. A summary written while turns were omitted records their count
   (`omitted_turns`), see "Storage".

The turn sends the `compacting` draft before each compaction, records
`conversation_compacted` itself, drops the base of its estimate, counts the misses of
the breaker, and sends the next `context` draft with the estimate of the compacted
request. An interrupt during a compaction ends the turn at once.

### Compaction

One compaction has these steps. A turn runs them between two model calls, and a
manual compaction runs them between turns.

1. Prune. Take the model's history as the next request would send it. Walk its tool
   results from the oldest. Keep every result in the newest `PRUNE_KEEP_TOKENS` (40000)
   of history as it is, and every result of the shortest tail (step 3) however large,
   so the model always reads the newest result. Give every other result whose output
   is longer than `PRUNED_OUTPUT_STUB` that stub as its output. When this frees fewer than
   `PRUNE_MIN_TOKENS` (20000), do not prune: a new cache prefix costs more than it
   saves. When it frees enough and the estimate is then below the trigger, the
   compaction ends here, with no summary (an auto compaction only). After an overflow
   the estimate has just counted too low, so it cannot show that pruning is enough: a
   summary always follows. The cut is the place after the newest pruned result.
2. Summarize. Send one summary request:
   - the same system prompt and the same tool definitions as the turn;
   - the history as the last request sent it, so the request hits the prompt cache.
     Only when that request would pass the hard cap, or after an overflow, does it
     send the pruned history;
   - as the last message, a user message with the summary prompt (below) and, for a
     manual compaction with `focus`, the line `Keep in the summary: <focus>`;
   - `max_output_tokens` of `SUMMARY_MAX_OUTPUT_TOKENS` (8000) for the summary plus
     `SUMMARY_REASONING_TOKENS` (24000) for the model's reasoning, which the Responses
     API counts in the same limit. The subscription backend refuses that member, so
     there the prompt alone asks for the length. On the API backend, a `max_output_tokens`
     in the model's entry of `[openai] models` wins over it. So the 8000 is a budget
     that the prompt asks for, not a cap that efr can make the provider keep.

   Some messages do not fit in the summary request. Then the oldest messages after the
   fresh block and the earlier summary go (never so that a tool result comes first),
   and one user message in their place says `N earlier messages are omitted: they did
   not fit in this request.` The provider counts more tokens than the estimate, so
   the target is the trigger scaled by what a refusal shows: below the estimate of
   the refused request times the trigger over the window. This happens:

   - after an overflow, before the first try, when the summary request is not smaller
     than the request that the provider refused (pruning freed nothing), because the
     provider would refuse it too;
   - when the provider refuses the summary request itself as too large; then the
     request goes once more.

   The messages before the cut that the summary never saw are counted in
   `omitted_messages`, and the daemon logs one `warn` line with the count.

   The answer's text is the summary. Tool calls in the answer are ignored. An answer
   without text fails the compaction, and so does an answer that the provider cut
   off (it reached the output limit, or the provider stopped it for its content): a
   summary that stops in the middle of its sections never replaces the history. The
   summary covers the whole history it saw,
   the tail too: the tail repeats the newest part word for word, and the summary must
   stand alone. An earlier summary is part of that history, so each summary carries
   the one before it forward.
3. Cut. Walk back from the newest message and keep messages in the tail while their
   estimate fits in `TAIL_TOKENS` (20000). The tail always holds at least the newest
   tool call with its result, or the newest prompt or steer, whichever is newer: the
   shortest tail. Move the cut back until it does not start with tool results. The cut
   may fall inside a turn (`through_message`). When the cut would be before the first
   message, nothing lies before it, and the compaction frees no room.
4. Record `conversation_compacted` (`Compaction`), see "Storage". `tokens_after` is the
   estimate of the rebuilt request.

A compaction that fails (the summary request fails, or its answer has no text or was
cut off) records nothing. An auto compaction that fails lets the turn go on when the request
fits under the hard cap; else the turn fails.

### The summary prompt and its record

The summary prompt is one constant, the text of `src/compaction/prompt.md`. It asks for
these sections, as Markdown headings in this order, with `None.` under a heading that
has nothing:

1. `## Task and state`: what the user asked for, and how far the work is.
2. `## Decisions`: the choices made and why; what the user approved or refused.
3. `## Important details`: the facts the rest of the work needs: names, values,
   errors, commands that worked or failed, and the instructions and preferences that
   the user gave in chat, quoted.
4. `## Files and places`: the files read or changed, directories, URLs and services
   touched.
5. `## Open work and next step`: what is left, and the next action.
6. `## Shell and directories`: the user's directory and the hidden shell's directory
   now, the earlier directories that matter, the shell state (variables, virtual
   environments) and the running jobs.
7. `## System tasks`: open system-manager tasks that need a follow-up or a cleanup,
   such as a service that was started, a package that is half installed, a timer or a
   mount.

The prompt also says: write facts, not a story; keep paths, commands and error texts
exact; at most about 2000 words; do not call tools. The same prompt and the same
sections make the handoff record of milestone 4: a record is the summary text plus
the event's `compaction_id`, `model`, time and conversation id.

### Storage

- `conversation_compacted` holds the whole compaction (`efr_protocol::Compaction`): its
  id, the turn that ran it (absent for a manual one), `trigger`, `focus`, `model`,
  `window`, `limit`, `tokens_before`, `tokens_after`, the cut (`through_turn`,
  `through_message`), `kept_turns`, `pruned_outputs`, `pruned_tokens`, what the
  summary never saw (`omitted_turns`, `omitted_messages`), `summary` and the `usage`
  of the summary request.
- The store keeps every compaction in the `compactions` projection
  (`efr_store::compactions`). A turn reads the newest compaction with a summary and the
  newest compaction of any kind with their own query (`compactions::latest`), never
  from the 4096-event page, so the page never decides what the model sees.
- Every event stays in the log. After the event is recorded, the `turn_messages` rows
  of the turns before `through_turn` may be deleted, and the row of `through_turn` too
  when `through_message` is absent.
- `efr history` shows every event and marks the place of each compaction with its
  scrollback line (see "Display").

### The history after a compaction

A request after a compaction with a summary has these parts, in this order:

1. the system prompt;
2. a fresh context block, one user message read from disk when the compaction ends
   (`fresh.rs`), between `<fresh-context>` and `</fresh-context>`: the user's directory
   and the hidden shell's directory, the running jobs of the hidden shell
   (`Toolbox::jobs`; left out when the toolbox cannot tell), the git status of the
   project root (`ScopeResolver::status`, at most 40 lines, git runs none of the work
   tree's programs), and the `AGENTS.md` files from the project root down to the user's
   directory, each cut at 32 KiB (memory joins at milestone 4). The project root is the
   registered project's, else the git work tree's; without one, only the user's
   directory is read. The actor keeps the block in memory with the compaction id, so
   every request until the next compaction sends the same bytes. After a daemon
   restart, the next turn reads it from disk again;
3. the summary, one user message: `<conversation-summary>`, the summary text,
   `</conversation-summary>`; when the summary never saw some turns or messages
   (`omitted_turns`, `omitted_messages`), one more user message after it says so:
   `The summary leaves out 2 earlier turns and 40 earlier messages: they did not fit
   in the model's context.`
4. the verbatim tail: the messages after the cut, from the turn's messages (the
   actor's cache or `turn_messages`), with the stub in each tool result before the cut
   of a newer prune-only compaction. A tool result whose call is not in the history
   is left out (a `warn` line gives the count): a turn rebuilt from its events can
   have other places than its transcript had, and a provider refuses a result
   without its call;
5. the new turn: its preamble and prompt, or, inside a turn, the rest of that turn.

Parts 1 to 3 stay the same until the next compaction, so the request prefix stays
stable and the prompt cache hits. After a prune-only compaction, the request is the
history as before, with the stub in each pruned result.

### Manual compaction

`conversation.compact { command_id, conversation_id, focus? }` (`efr compact [focus]`,
`,compact [focus]`), scope `operate`:

- While a turn of the conversation runs, or another compaction does, it is refused
  with `conflict` (`ConversationError::CompactionBusy`). efrd does not wait for the
  turn: the turn compacts on its own when it needs to, and a wait would hold the client
  for an unknown time. A retry with the command id of the compaction that runs, as
  after a dropped connection, is no other compaction: it waits for the same answer.
- When nothing lies before the cut, it is refused with `conflict`
  (`ConversationError::NothingToCompact`). A summary request that fails is
  `ConversationError::Summary`, an answer without text `EmptySummary`, an answer that
  was cut off `IncompleteSummary`; none records anything.
- Otherwise the actor runs the steps above as its one job, in a task of its own
  (`actor/compact.rs`), so it keeps answering, and `ConversationState::compacting` is
  true. A prompt that arrives meanwhile waits in the queue (its result says `queued`)
  and starts when the compaction ends. The summary request uses the model and the
  effort of the newest turn, so it shares the prefix of the last request. A manual
  compaction always writes a summary: it does not stop after the pruning. The event
  has trigger `manual`, no `turn_id`, and the `focus`; it is recorded with the
  command's receipt. The result has the event's `seq` and the compaction. A retry with
  the same command id returns the first result.
- It never starts a turn.

### Failures

A turn that fails for its context records `turn_failed` with code `internal`, data
`{"cause": "context_overflow", "tokens": <estimate or refused size>, "window":
<window>}`, and a message that names the cause and the way out (`context_full` in
`src/context.rs`):

- above the hard cap: `the context is full: about 260k of 272k tokens, above the cap
  of 258k; run ,compact or start a new conversation`;
- a refusal with auto off: `the context is full: the model refused about 281k of 272k
  tokens; run ,compact or start a new conversation`;
- a refusal whose compaction failed: `the context is full and the compaction failed
  (<why>): the model refused about 281k of 272k tokens; ...`, where `<why>` is the
  sentence of the provider's error (such as `the provider is rate limiting requests`)
  or of the summary's failure, and is left out when nothing lay before the tail;
- a request above the hard cap whose auto compaction failed: `the context is full and
  the compaction failed (<why>): about 260k of 272k tokens, above the cap of 258k;
  ...`;
- a refusal or a request above the hard cap after the breaker: `the context is full:
  compaction did not free enough room (still about 240k of 272k tokens); ...`;
- a second refusal after the compaction: `the context is still full after a
  compaction: the model refused about 150k of 272k tokens; ...`.

A `ContextOverflow` that reaches the generic mapping of provider errors gets the data
`{"cause": "context_overflow"}`.

### Display

`efr` shows the context inline, never in an alternate screen:

- The status row of a running turn always has `ctx N%`, from the newest `context`
  draft or compaction (`tokens_after`), and so does the line of a running call, which
  takes the row's place. N is `tokens` times 100 divided by `limit`, rounded down. The
  colour roles are `Success` below 50, `Warning` (not bold) from 50 and `Error` from
  90. On a screen that is too narrow, the gauge goes first.
- The end-of-turn line has `ctx N% (<tokens>/<limit>)` from the end event's `context`,
  such as `done in 42s, ctx 43% (89k/207k), 1.1k out`. A turn without `context` keeps
  today's line.
- After a `compacting` draft the status row says `compacting context`, until the
  `conversation_compacted` event, the next `context` draft or the end of the turn.
- A client that subscribes while a turn runs gets the turn's newest `context` draft
  (and its `compacting` draft while it compacts) first, so the gauge shows at once,
  not only at the next model call (`ConversationHandle::live_drafts`).
- Each `conversation_compacted` prints one muted line, wrapped at the width so that
  its way out is never cut off:
  - `context compacted (auto): 231k -> 24k tokens, kept 3 turns, summary 3.2k`
    (`(efr compact)` for a manual one; `pruned 12 outputs` in place of the summary
    for a prune-only one; `(left out 2 turns and 40 messages)` at the end when the
    summary never saw them, also on the overflow line);
  - for trigger `overflow`: `context full: the request was 281k of 272k tokens;
    compacted and retried`;
  - when `tokens_after` is at or above `limit` (a breaker miss): `context full:
    compaction did not free enough room (still 240k); run ,compact or efr new`, and
    `...; run efr new` after a manual one, which another `,compact` cannot help.

## Tier

Tier 3, the engine.

## Allowed dependencies

`efr-provider` (the `Provider` trait and canonical messages), `efr-permissions` (the
engine at the check point), `efr-scope` (scope derivation, `Home`), `efr-store` (the
writer and the readers), `efr-protocol` (events, ids, method params and results) and
`efr-stdx` (the clock, the generator, ids, `claim_dir`). The allowlist also names
`efr-tools`; see above for why it is not used. `xtask/src/deps.rs` forbids
`efr-conversation -> efr-shell` and `efr-conversation -> efr-transport`.

Third-party crates: `tokio` (the actor, its turn tasks, channels, `spawn_blocking`),
`futures` (the provider's stream), `async-trait` (`Toolbox`, `ScopeResolver`),
`serde` and `serde_json` (receipts, tool inputs), `jiff` (the date in a scratch name),
`thiserror` and `tracing`.

## Invariant

- `turn.rs::authorize_tool_call` is the only place where a tool call meets the
  permission engine, and nothing reaches `Toolbox::invoke` without passing it. A
  toolbox declares, `efr_permissions::Engine::decide` decides with the turn's scope,
  origin, permission mode (`ConversationConfig::mode`, read when the turn starts) and
  the conversation's policy, and the turn enforces: `Allow` runs, `Contain` runs in
  the sandbox, `Deny` gives the model an error that names each refused path with its
  class, `Ask` parks the turn on a `oneshot` until the user answers.
- In `auto` nothing runs outside the sandbox without a person: a contained call has
  `Launch::Contained`, and only a "yes" gives `Launch::Unsandboxed`, to a line of one
  command. A turn without a working sandbox runs as `cautious`, never as `auto` with
  less.
- The scope is derived again on every turn; it is never cached.
- The last command of a prompt never enters an event or a log field; `Debug` of the
  types that hold it leaves it out.
- Every change a request makes is one batch with its receipt, so a retried command
  runs once.
- Time and randomness come from the injected clock and generator.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-conversation
```

The turn and actor tests run a real actor against the real store in memory, a
`TestClock`, a seeded generator, a `ReplayProvider` that checks every request against
the one the test expects, a fake toolbox and a fake scope resolver: a text turn, a tool
call allowed, denied, asked then approved or denied, a phone turn that asks, the scope
of each turn deciding a write, an interrupt mid-stream, during an approval and during
a tool call, an approval that times out, a provider 401, a cwd move between turns,
provider items passed back to the same provider, also after a restart, and dropped
for another provider, steering, coalesced updates, drafts (`turn/tests/drafts.rs`: their parts
and `after_seq`, coalescing on the clock, the same log with and without a listener, no
timer without one), a queued second prompt, receipts and
the refusals, a steer after the last model call, and the requests of a client that has
just seen the end of a turn (`actor/tests.rs` holds the turn's task after its work
with a tracing layer, so an end recorded too early shows every time). The input row
of a turn (`actor/tests/turn_input.rs`) holds turns with gates, not time: a steer
after an interrupt, a steer as steering closes, a late steer with no running turn,
withdraws by turn and by terminal with their refusals, a withdraw just before and just
after the queued prompt would start, an interrupt that withdraws and resends in one
append (with the prompt of another terminal left queued and a retry that gets the same
sequence numbers), a steer that a model call read, and a refused interrupt that
changes nothing. The context tests (`turn/tests/context.rs`) cover the sums and the
last call's count on the end event, the `context` drafts, the estimate of the next turn
from the end of the last, the cache key, the hard cap with auto off, and the note for
omitted turns, the safety net that scales with the window, a newest turn that has no
end, and the breaker that stays open from turn to turn. The compaction tests (`turn/tests/compaction.rs`,
`actor/tests/compact.rs`, `compaction/tests.rs`) use a model with a window of 100000
tokens, big prompts and big reads: an auto compaction at the trigger that goes on with
the turn, the request of the next turn (an insta snapshot of its shape) and the same
request after a restart, an overflow that compacts once and sends the call again, a
second overflow and an overflow with auto off that fail the turn, a summary after an
overflow even when pruning looks enough, a failed summary under and above the hard
cap (with the provider's sentence), a summary request that leaves out its oldest
messages and the note that the next turn rebuilds, a summary cut off at the output
limit, a retry of the running manual compaction, the breaker, a pruning without a summary that the next
turn rebuilds, a manual compaction with a focus, a prompt that waits for it, and its
refusals; the pure steps (pruning frees at least 20000 tokens or does nothing, the tail
rule, the cut) and the summary prompt's sections have unit tests. The `auto` tests
(`turn/tests/sandbox.rs`) cover a contained call, a
network, write and privilege exit with their launches, a denied exit, the one-command
rule, a user's `ask` rule, the floor refusals that stop a turn at three, the fallback to
`cautious` (no sandbox, a project at home) and the quarantine question (answered,
expired, refused for a phone, interrupted). `exit/tests.rs` checks `grant`, the question
facts and the record against the real engine. The preamble is covered by insta
snapshots. The scratch and resolver
tests use temporary directories; the resolver tests run git, isolated from the user's
configuration. No test uses the network, a real model, real time or the user's home.
