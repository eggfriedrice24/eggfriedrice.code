# efr-protocol

## Purpose

The contract. Everything that crosses the daemon's sockets is defined here: frames,
the `Method` enum with one params type per method, results and stream items, `Event`
and its envelope, ids, `Scope`, `ShellContext`, turn settings, screen snapshots, wire
errors, the length-prefix framing and `PROTOCOL_VERSION`. The daemon, `efr`, the tests, the PTY
proxy and the WebSocket clients compile against these types. The phone app reads
`docs/protocol.md` and the frozen fixtures in `fixtures/v1/`.

The crate has no runtime: serde types, pure functions and one pure state machine.

Where things are:

| File | Holds |
|---|---|
| `src/version.rs` | `PROTOCOL_VERSION` |
| `src/ids.rs` | `ConversationId`, `TurnId`, `CommandId`, `CallId`, `PtyId`, `DeviceId`, `QuestionId`, `DaemonId` (UUID newtypes), `Seq`, `RequestId` |
| `src/scope.rs` | `Scope`, `ProjectId`, `Origin`, `ScopeName` |
| `src/shell_context.rs` | `ShellContext`, the zsh plugin's observed state |
| `src/settings.rs` | `Mode`, `TurnSettings` (what a prompt asks for), `EffectiveSettings` and `OverriddenSettings` (what a turn runs with) |
| `src/sandbox.rs` | the `auto` sandbox: `Launch`, `Grant`, `BusKind`, `CacheMode`, `NetworkMode`, `ModeFallback` |
| `src/sandbox/exit.rs` | `ExitKind`, `ExitSource`, `Needs` (the shell tool's `needs` input and its schema), `ExitInfo` |
| `src/sandbox/record.rs` | the classifier's `ExitRecord` and its facts, `PathClassName`, `Judgement`, `Verdict`, `Risk`, `UserAuthorization`, `JudgeKind` |
| `src/sandbox/status.rs` | the probe's `SandboxStatus`, `SandboxCheck`, `CheckOutcome`, `SandboxPaths` |
| `src/sandbox/summary.rs` | what a call reports back: `SandboxSummary`, `Blocked`, `BlockReason`, `SurfaceChange`, `ReportedFile` |
| `src/screen.rs` | `ScreenSnapshot`, `RowCells`, `Cell`, `Color`, `Cursor`, `Size` |
| `src/capabilities.rs` | `Capabilities`: known keys plus extras |
| `src/changes.rs` | what a call or a turn changed in files: `FileChanges`, `FileChange`, `ChangeKind`, and the caps `MAX_LISTED_FILES`, `MAX_CALL_DIFF_LINES` and `MAX_TURN_DIFF_LINES` |
| `src/error.rs` | `ErrorCode`, `ErrorBody`, `ErrorFrame`, and `ProtocolError`, the crate's error type |
| `src/event.rs` | `Event`, `EventEnvelope`, `ApprovalDecision`, `InputWait`, `Usage` |
| `src/method.rs` | `Method` and `ScopeName::for_method` |
| `src/methods.rs` | the types that several methods share: `PageCursor`, `Base64Bytes`, `ConfigFileError` |
| `src/methods/*.rs` | one file per method: its params, and its result or stream item; `turn_steer.rs` also holds `LateSteer`, `turn_interrupt.rs` `ResentSteers` and `prompt_withdraw.rs` `WithdrawTarget` and `WithdrawnPrompt` |
| `src/secret_text.rs` | `SecretText`, typed text such as a password: a plain string on the wire, redacted in `Debug`; its own buffer and each clone's are zeroed on drop, copies made outside it are not |
| `src/frame.rs` | `ClientFrame`, `ServerFrame` |
| `src/framing.rs` | `encode` and `Decoder`: the 4-byte big-endian length prefix, 16 MiB cap |
| `src/schema.rs` | the JSON Schema document, behind the `schema` feature |
| `src/fixtures_check.rs` | the samples behind `fixtures/v1/` and their checks (tests only) |

### Wire rules

- A frame on the Unix socket is a 4-byte big-endian length and then one JSON object of
  at most 16 MiB. The WebSocket transport (a later milestone) sends the same JSON
  object as one text message.
- Client frames: `{id, method, params}` and `{cancel: id}`. Server frames:
  `{id, item}`, `{id, end: true}`, `{id, error}` and `{ack: id}`. Every request gets
  zero or more `item` frames and then exactly one `end` or `error`. A unary method sends
  exactly one item.
- `hello` comes first. Both sides send `PROTOCOL_VERSION`; a mismatch fails with
  `protocol_mismatch` before any other method runs.
- Names on the wire are snake_case. Optional fields are left out when empty, unknown
  fields are ignored, and ids are lowercase hyphenated UUIDs. `Seq` and `RequestId` are
  JSON numbers. Timestamps are RFC 3339 strings in UTC. PTY bytes are standard base64.
- Every internally tagged enum uses the member `kind` as its tag.
- `conversation.subscribe` with `drafts` also streams `draft` items: parts of a running
  turn before the event log has them. A draft has no sequence number of its own, the
  daemon never stores it, and it is best effort. Its `after_seq` names the last event
  that the turn recorded before it.
- An event of a kind that this build does not know decodes as `Event::Unknown` and
  encodes back to the same JSON, so old readers keep advancing their cursor.
- `ErrorCode` is a closed set; a new code bumps the protocol version. New optional
  fields, capability keys and event kinds do not.

### Turn input: steer, queue, withdraw

The input row of a turn (`render.turn_input` in `efr`) uses `turn.steer`,
`prompt.send`, `prompt.withdraw` and `turn.interrupt`. This section is the contract
between efrd and `efr`. The doc comments of the types say the same per member.

Words:

- A steer is a `turn.steer` that efrd records as `turn_steered`. It is unread until a
  `steering_delivered` event names its seq. A model call reads it after that.
- A queued prompt is a `prompt_queued` event whose turn did not start. Its id is its
  `turn_id`.
- A steer is late when the turn makes no more model calls: the model answered without
  a tool call and steering closed, an interrupt was requested, `turn_id` names a turn
  that does not run, or no turn runs.

efrd must:

1. `turn.steer` without `if_late`: refuse a late steer with `conflict`
   (`NoRunningTurn`), as before. Never record a late steer as `turn_steered`. A steer
   after `turn_interrupt_requested` is late, so the model never reads it.
2. `turn.steer` with `if_late: {kind: "queue", ...}`: in the actor step that finds the
   steer late, record `prompt_queued` with a new `turn_id`, the steer's `command_id`
   and text, the origin of the connection, and the `context` and `settings` of
   `if_late`. Hand `last_command` to the turn in memory only, as `prompt.send` does.
   The prompt waits behind the queue, or starts at once when no turn runs. The result
   has `queued: true`, the new `turn_id` and the seq of `prompt_queued`. A steer that is
   not late ignores `if_late`.
3. Record `steering_delivered` (`turn_id`, the `turn_steered` seqs in order) before or
   in the same append as the model call that sends those steers. Each `turn_steered`
   is named by at most one `steering_delivered`.
4. `prompt.withdraw`: take one queued prompt out of the queue and record
   `prompt_withdrawn` (`turn_id`, origin of the connection). That turn never starts, and
   `prompt_withdrawn` is its last event: projections, `conversation.subscribe`
   followers and `efr history` must treat it as the end of the turn. Target `turn`:
   that turn. Target `newest_from_tty`: the newest queued prompt whose `prompt_queued`
   context has that `tty`. Errors: `conflict` for a turn that started, ended or was
   withdrawn; `not_found` for a turn that the conversation never queued, and for a tty
   without a queued prompt. Keep a receipt, so a retry with the same `command_id`
   returns the first result.
5. `turn.interrupt` with `resend_steers` or `withdraw`: do all of it in the one actor
   step that records `turn_interrupt_requested`, in one append, so no queued prompt can
   start in between. Record in this order: `turn_interrupt_requested`, then one
   `prompt_withdrawn` per withdrawn prompt in queue order, then the `prompt_queued` of
   the resent steers.
   - `withdraw`: withdraw each listed turn that is still queued in this conversation.
     Skip the others (started, ended, withdrawn, unknown). Prompts that are not listed
     stay queued, also those of other terminals.
   - `resend_steers`: take the listed seqs that are unread `turn_steered` events of
     this turn. Skip the others. When one or more remain, record one `prompt_queued`:
     a new `turn_id`, the texts joined with `\n` in seq order, `steers` set to those
     seqs, the `context` and `settings` of the interrupted turn's own `prompt_queued`,
     the origin of the connection and the interrupt's `command_id`. This prompt runs
     next, before the prompts that wait in the queue.
   - When the turn does not run, answer `conflict` and do none of it.
   - The result has `resent` (absent when no steer was unread) and `withdrawn` (the
     texts, in queue order).
6. efrd implements points 1 to 5. An older efrd ignores `if_late`, `resend_steers`
   and `withdraw`, and refuses `prompt.withdraw` with `invalid`. `efr` must accept
   these answers too.

`efr` must:

1. Enter: send `turn.steer` with the followed `turn_id` and
   `if_late: {kind: "queue", context, last_command, settings}`, with the values that
   `prompt.send` from this terminal would carry. With `queued: false`, keep the
   result's `seq` as an unread steer. With `queued: true`, keep the result's `turn_id`
   as a queued prompt.
2. Tab: send `prompt.send` to the followed conversation. Keep the result's `turn_id`
   as a queued prompt.
3. `steering_delivered`: move each named steer of this view from the unread list to
   the scrollback, in the style of a user message. `prompt_queued` with `steers`: those
   steers are now that prompt.
4. Esc: send `turn.interrupt` with the followed `turn_id`, `resend_steers` set to the
   unread steers of this view and `withdraw` set to the queued prompts of this view.
   Put the `withdrawn` texts back into the input row. When `resent` is present, show
   the note "interrupted to send your message".
5. Alt+Up: send `prompt.withdraw` with target `turn` and the newest queued prompt of
   this view. Put the text back into the input row. On `conflict` the prompt already
   started: drop it from the list.
6. Keep following until the followed turn and every queued prompt of this view ended:
   `turn_completed`, `turn_failed`, `turn_interrupted`, `turn_cancelled` or
   `prompt_withdrawn`.

Outside the protocol, the zsh plugin gets back the text that is still in the input
row when `efr` ends. The plugin sets `EFR_DRAFT_FILE` to a path in its runtime
directory. `efr` writes the text there as UTF-8, mode 0600, without the leading `, `
and without a final newline. The plugin's `precmd` reads the file, removes it and
pushes `, <text>` with `print -z`. Without `EFR_DRAFT_FILE`, `efr` prints the text as
one muted note.

### Frozen fixtures

`fixtures/v1/` holds one JSON file per method's params (the `method` and `params`
members of a request frame), per result or stream item variant, per event kind, per
frame shape, every kind of input wait, every mode, and the closed sets of error codes
and scope names. Two tests guard them:

- every sample in `src/fixtures_check.rs` encodes to exactly the bytes of its file;
- every file decodes as its type and encodes back to exactly the same bytes.

Another test fails when a method or an event kind has no fixture, or when a file has no
sample. To change a fixture on purpose, change the sample, rewrite the files and add a
line to the changelog in `docs/protocol.md`:

```sh
cargo test -p efr-protocol --lib -- --ignored --exact fixtures_check::tests::bless_fixtures
```

The comparison is a plain byte comparison of the files, not `insta`: an insta snapshot
file starts with a metadata header, and the phone repository needs plain JSON files.

### The schema feature

`--features schema` builds `efr_protocol::schema::document()`, a JSON Schema document of
every frame, method, result, item and event, for `cargo xtask protocol-docs`. The
`JsonSchema` derives on the wire types are always compiled, so `schemars` is a normal
dependency: the tidy rule allows `cfg(feature)` in this crate only on the one
`mod schema;` line of `lib.rs`, which rules out a `cfg_attr` derive on each type, and
the alternative, a hand-kept copy of every wire type inside `schema.rs`, would drift.

## Tier

Tier 0. Every crate that speaks the protocol depends on it.

## Allowed dependencies

`xtask/src/deps.rs` allows `efr-stdx`, but the crate uses no workspace crate at all:
`efr-stdx` depends on tokio, and `efr-protocol -> tokio` is a forbidden edge, checked
through every chain of dependencies. So ids are built from a UUID that the caller mints
with `efr_stdx::id::uuid_v7(clock, rng)`, and timestamps come from the caller's `Clock`.

Third-party crates: `base64`, `jiff`, `schemars`, `serde`, `serde_json`, `strum`,
`thiserror`, `uuid`, `zeroize`.

## Invariant

The protocol crate stays free of a runtime and of IO, so any client can compile it and
the framing can be tested without sockets. It is also the only place that knows the
wire form of a type: other crates exchange these types, never hand-written JSON.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-protocol
cargo test -p efr-protocol --doc
cargo test -p efr-protocol --features schema schema
```

The tests use no network, no sockets, no real-time sleeps and no Zig. The framing tests
include a proptest that any split of a byte stream into chunks decodes to the same
frames.
