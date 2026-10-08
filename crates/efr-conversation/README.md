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
  effort as `provider_options["reasoning_effort"]`, and its mode in the permission
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
  prompt. Prompts and steers that are not listed stay as they are, so those of other
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
request:

1. the system prompt (the static rules, `ConversationConfig::system_prompt`);
2. bounded history (`HistoryLimits`: 50 turns, 4096 events, 512 KiB of message JSON),
   each earlier turn from the actor's cache or the saved turn messages, else rebuilt
   from its events;
3. the newest prompt, whose first block is the live-state preamble regenerated every
   turn: the shell's directory and previous directory, the last command and its exit
   status, the git work tree and branch, home, host, OS, `$SCRATCH`, the hidden
   shell's own directory when it differs, the turn's permission mode, model and
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
`retry_after_ms`, an unknown model to `invalid` with the `model`, the rest to
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
changes nothing. The `auto` tests (`turn/tests/sandbox.rs`) cover a contained call, a
network, write and privilege exit with their launches, a denied exit, the one-command
rule, a user's `ask` rule, the floor refusals that stop a turn at three, the fallback to
`cautious` (no sandbox, a project at home) and the quarantine question (answered,
expired, refused for a phone, interrupted). `exit/tests.rs` checks `grant`, the question
facts and the record against the real engine. The preamble is covered by insta
snapshots. The scratch and resolver
tests use temporary directories; the resolver tests run git, isolated from the user's
configuration. No test uses the network, a real model, real time or the user's home.
