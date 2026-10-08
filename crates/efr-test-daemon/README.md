# efr-test-daemon

## Purpose

The real efr daemon, in-process, for the integration tests of `efr-daemon` and
`efr-cli`, and the NDJSON scenario replay of the structure document (section 7).

- `test_daemon`: `TestDaemon::builder()` starts `efr_daemon::start` on a temporary tree
  (`efr_test_support::TestDirs`: the four efr roots and a stand-in home), with the
  socket in the temporary runtime directory, vt100 screens, a `TestClock`, a seeded
  `TestRng`, an in-memory store (`persistent()` keeps `efr.sqlite` for a restart),
  isolated git, the host `testhost` running `TestOS`, the UTC time zone, a short
  system prompt and no idle shell collector. The hidden shells run on the
  `FakePtyHolder` unless `local_pty()` asks for the build's own holder. The model is a
  `ReplayProvider`, a provider the test wrote (`custom_provider`), or the real
  `openai-api` provider against a `ResponsesServer`; without one, every model call
  fails, so no test reaches the network by accident. The daemon serves in a task of its
  own. The test gets `efr_client::Client`s (`client`, `client_for_tty`, `connect`),
  moves time through `clock()`, reads a conversation's log with `events` (paging
  `conversation.history`), and calls `stop` or `restart` (the same tree, clock, holder
  and provider; a new generator seed so ids never collide). For the `auto` sandbox,
  `sandbox_launcher` names the `efr-sbx` to copy (the real one from
  `EFR_TEST_SBX_BIN`, or a fake script) and `probe_override` sets the probe's result;
  without them the daemon finds no launcher next to its test binary, and `auto` runs as
  `cautious`.
- `test_daemon/responses`: `ResponsesServer`, a wiremock server that answers
  `POST /v1/responses` from a queue of `ResponsesAnswer`s (a status and a body;
  `ResponsesAnswer::text` builds a whole streamed text answer, `ResponsesAnswer::tool_call`
  one function call sent whole, `ResponsesAnswer::custom_tool_call` one call of a
  freeform tool whose text streams in two deltas) and keeps every request,
  `Authorization` header included (never shown by `Debug`). It also answers
  `GET /v1/models`, the model catalog that efrd fetches in the background for the
  subscription: with the `ModelsAnswer` of `set_models` (a catalog with its `ETag`, a
  bare status such as 304 or 503, and a delay for a backend that hangs), else with a
  404. It keeps each fetch as a `ModelsRequest` (`client_version`, `If-None-Match`,
  `originator`).
- `pty_script`: `FakePtyHolder`, a `PtyHolder` whose masters are socketpairs. The test
  takes the other end of the n-th spawned PTY as a `FakeTerminal`, reads what the
  session types (`typed_line`, `typed_against`) and prints what a zsh with the efr
  integration prints (`PROMPT`, `command_output`). `wait` returns when the test ends the
  child or the daemon sends it `SIGHUP` or `SIGKILL`. `foreground` answers the shell's
  own pid while it runs, until the test sets another answer with `set_foreground` (a
  job that holds the terminal, for the end of a sandboxed run). A `PtyScript` is a list of prints
  and expected input that a terminal plays.
- `replay`: `Replay` drives a `Scenario` (a fixture in `fixtures/` and a line in
  `SCENARIOS` saying how its daemon runs) through a test daemon, record by record:
  client frames are sent (results kept by frame id), events must arrive in order on a
  subscription to the conversation (the chunking-dependent kinds
  `assistant_message_updated` and `tool_call_output_updated` are skipped unless a record
  expects one), PTY bytes are printed or must be typed, the clock moves. The provider is
  paced, so an answer never runs ahead of the records before it, and `verify` checks at
  the end that it got every request of the transcript and no other. Minted ids are
  placeholders (`<conversation:1>`, `<turn:2>`, `<call:1>`, `<pty:1>`, numbered in the
  order the replay sees them, `Bindings`); paths and timestamps are the redactor's
  (`<TMP>`, `<CWD>`, `<HOME>`, `<TIMESTAMP>`). A scenario in the Responses mode queues
  each exchange's `provider_sse` bodies as the answer, with a first line `: status <code>`
  (an SSE comment) setting the HTTP status, and compares the requests at the end.
  `Replay::bless(name)` rewrites a fixture's outbound records with what the daemon
  actually sent and keeps the inbound ones, so a fixture is written as a skeleton and
  reviewed as a diff.

The fourteen fixtures are the milestone 1 list: `single_turn_text`,
`tool_call_shell_ok`, `tool_call_shell_nonzero_exit`, `approval_ask_then_allow`,
`approval_deny`, `cwd_move_between_turns`, `interrupt_mid_stream`,
`queue_second_prompt`, `subscribe_resume_after_seq`,
`subscribe_gap_too_large_snapshot`, `duplicate_command_id_receipt`,
`provider_401_refresh_once`, `restart_reconcile_inflight_turn` and
`pty_attach_since_seq`. The tests in `crates/efr-daemon/tests/` replay them and add the
assertions of each case.

## Tier

Tier T. Kind `dev`, `publish = false`: only the `tests/` targets of `efr-daemon` and
`efr-cli` name it, under `[dev-dependencies]`.

## Allowed dependencies

`efr-daemon` (without its default `local-pty` feature, with `test-sandbox-fake` for the
sandbox's test seams), `efr-test-support`,
`efr-client` and `efr-protocol`. `xtask/src/deps.rs` holds the allowlist. The holder
types come through `efr-daemon`'s re-exports, so this crate needs no `efr-holder` edge.

Third-party crates: `tokio`, `tokio-util` (the daemon's shutdown token), `futures`,
`async-trait` (the holder trait), `serde_json`, `jiff` (the time zone), `wiremock`
(the Responses server) and `thiserror`.

## Invariant

- `efr_test_daemon` appears only in `tests/` directories and in this crate (a tidy
  rule), so no library's unit tests link a second copy of the daemon.
- A test daemon never touches the network, the real home, config or runtime directory,
  or the machine's git configuration, and never waits on real time: every deadline runs
  on its `TestClock`.
- A fixture is the contract of its scenario: every outbound record is compared exactly
  after the placeholders, and bless changes outbound records only.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-test-daemon
```

The unit tests drive the fake holder over its socketpairs (typing, printing, scripts,
signals, `wait`, release), the id placeholders, the Responses server over plain HTTP,
the test daemon's start, stop and restart (with and without a file store), and the
replay: the scenario table against the fixture directory, a whole scenario, and the
failures a fixture can show (a different event, typed line or request, an unbound
placeholder). Rewriting the fixtures is an ignored test that `just bless` runs:

```sh
cargo nextest run -p efr-test-daemon --test it --run-ignored only -E 'test(/^bless::/)'
```

The scenario tests themselves run with `efr-daemon`:

```sh
cargo nextest run -p efr-daemon
EFR_TEST_ZSH=1 cargo nextest run -p efr-daemon shell_   # also the real zsh
```
