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
| `src/error.rs` | `ErrorCode`, `ErrorBody`, `ErrorFrame`, and `ProtocolError`, the crate's error type |
| `src/event.rs` | `Event`, `EventEnvelope`, `ApprovalDecision`, `InputWait`, `Usage` |
| `src/method.rs` | `Method` and `ScopeName::for_method` |
| `src/methods.rs` | the types that several methods share: `PageCursor`, `Base64Bytes`, `ConfigFileError` |
| `src/methods/*.rs` | one file per method: its params, and its result or stream item |
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
- An event of a kind that this build does not know decodes as `Event::Unknown` and
  encodes back to the same JSON, so old readers keep advancing their cursor.
- `ErrorCode` is a closed set; a new code bumps the protocol version. New optional
  fields, capability keys and event kinds do not.

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
