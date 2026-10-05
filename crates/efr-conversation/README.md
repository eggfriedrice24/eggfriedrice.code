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
- `steer` records `turn_steered`; the turn sends the text to the model before its next
  model call, and a turn that would end with steering waiting makes one more call.
- `interrupt` is two-phase: the actor records `turn_interrupt_requested`, the turn
  drops the provider's stream (or the parked approval, or the running tool call, which
  the toolbox is asked to `cancel`), completes the text that streamed so far, and only
  then records `turn_interrupted`.
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
   each earlier turn rebuilt from its events;
3. the newest prompt, whose first block is the live-state preamble regenerated every
   turn: the shell's directory and previous directory, the last command and its exit
   status, the git work tree and branch, home, host, OS, `$SCRATCH`, and the hidden
   shell's own directory when it differs;
4. the tool definitions.

It streams the provider and records coalesced `assistant_message_updated` events (at
most one per `update_interval` on the injected clock, the text held back sent when the
interval ends), each with only the text added since the previous one and its byte
offset, then `assistant_message_completed` with the whole text. Earlier turns are
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
`turn_interrupted`.

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
  hidden shell is, which an earlier call may have moved;
- `invoke`: `ToolRegistry::invoke` with the `OutputSink` passed through as the
  `ToolOutputSink` (output and input waits; the daemon itself answers whether a person
  can answer hidden input), and `ToolResult` copied into `ToolOutcome`; a `ToolError`
  becomes an error outcome;
- `cancel`: `ShellSessions::interrupt(conversation_id)` for the shell tool, so an
  interrupted command does not keep running in the hidden shell;
- `preview`: the diff of a `write_file` call, once the tools offer one.

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
  holds queued prompts); the actor starts with an empty queue.
- It routes `prompt.send` to the tty's active conversation and spawns or reuses its
  actor.

### Provider items do not survive a restart

`provider_raw` must go back unchanged to the model that made it, but no event in
`efr-protocol` carries it. The actor keeps the exact messages of the turns it ran (as
many as the history may carry), each with the provider and the model that answered
it, and uses them while both are the same; with another provider or another model
(a prompt that names one, or a new default in the config) they lose `provider_raw`:
the other model's encrypted reasoning and item ids go, the text, the tool calls and
their results stay, as opencode does. After a restart, history is rebuilt from the
events with the daemon's call ids and without provider items, which costs the
encrypted reasoning of earlier turns but nothing else.

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
  the conversation's policy, and the turn enforces: `Allow` runs, `Deny`
  gives the model an error that names each refused path with its class, `Ask` parks
  the turn on a `oneshot` until the user answers.
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
provider items passed back to the same provider and dropped after a restart with
another provider, steering, coalesced updates, a queued second prompt, receipts and
the refusals. The preamble is covered by insta snapshots. The scratch and resolver
tests use temporary directories; the resolver tests run git, isolated from the user's
configuration. No test uses the network, a real model, real time or the user's home.
