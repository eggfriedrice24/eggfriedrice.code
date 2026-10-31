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
  `ReplayProvider`, a provider the test wrote (`custom_provider`), the real
  `openai-api` provider against a `ResponsesServer` (`responses`), or the real
  `anthropic-api` provider against a `MessagesServer` (`messages`, with
  `ANTHROPIC_API_KEY` stored as its credential; the daemon has no model list until it
  fetches the one of `set_models`); without one, every model call fails, so no test
  reaches the network by accident. The daemon serves in a task of its
  own. The test gets `efr_transport::client::Client`s (`client`, `client_for_tty`, `connect`),
  moves time through `clock()`, sets which shell runs and which terminal a peer runs on
  through a `FakeProcView` (`proc_view`; without one, routing reads `/proc`), reads a
  conversation's log with `events` (paging
  `conversation.history`), and calls `stop` or `restart` (the same tree, clock, holder
  and provider; a new generator seed so ids never collide). The daemon's search
  indexer runs as in efrd: `search_index` gives its status (the cursor, and when it
  sleeps until, so a test moves the clock past the 500 ms after a turn's end), and
  `wait_indexed(seq)` waits until every event up to `seq` is in the index; it never
  moves the clock. `search_hook` runs a hook in the row loop of each search, so a test
  can hold a search while it moves the clock (on a `persistent` daemon: an in-memory
  store searches on its writer). The crate exports efrd's `SearchRowHook`, so the
  tests of efr-cli need no efr-daemon edge. For the `auto` sandbox,
  `sandbox_launcher` names the `efr-sbx` to copy (the real one from
  `EFR_TEST_SBX_BIN`, or a fake script) and `probe_override` sets the probe's result;
  without them the daemon finds no launcher next to its test binary, and `auto` runs as
  `cautious`. `statvfs_helper` names the program that measures the host's mounts for
  a line with `df` or `findmnt` (`path statvfs`, the paths on stdin): a script that
  prints fixed answers, or one that blocks until efrd kills it at its deadline. For background jobs, `job_env` gives every job a fixed environment in
  place of the `env` file of a real hidden zsh, and `TestDaemon::peer_side` says where a
  process stands on the daemon's socket (`User`, `ModelSide` or `Unknown`), as the scope
  check decides it. For the network proxy (phase D2), a test daemon has
  `network.mode = "none"` unless `network_mode(NetworkMode::Proxy)` turns it on, and the
  proxy then runs only with a `probe_override` whose `proxy` is true. `"none"` has the
  source `flag TestDaemon`, so a reload of `config.toml` (also the one of
  `admin.project_add`) keeps it. `proxy_hosts`
  maps test names (`registry.test`) to the servers behind them, each on a loopback
  address of its own: the name resolves to that address, and port 443 or 80 of it
  reaches the server. Without it the proxy refuses every name and every address, so a
  test daemon never reaches the internet.
- `TestDaemonBuilder::remote()` turns the remote listener on (`[remote] enabled`) with
  an `efr_test_support::FakeTailscaled` of its own (`TestDaemon::tailscaled`). The
  listener binds a free port of `127.0.0.1` in place of the tailnet address
  (`Deps::with_remote_listen`), and `TestDaemon::remote_addr` waits until
  `daemon.json` names it. The fake's whois decides each peer, so a phone of the test
  connects from loopback as any node of the tailnet; the real listener never binds
  loopback. Without `remote()`, `remote.tailscaled_socket` names a socket in the
  temporary tree that nothing binds, also when a test turns `remote.enabled` on by
  hand, so a test daemon never reaches the machine's tailscaled.
- `TestDaemonBuilder::bus_proxy(system, session)` turns the bus filter on with the
  test's own bus sockets, and `bus_proxy_program` names the `xdg-dbus-proxy` to run.
  Without them the daemon keeps `dbus.proxy = false` across reloads and has no bus at
  all, so it never reaches the machine's buses.
- For full mode, `TestDaemonBuilder::as_root(true)` makes the daemon count as running
  as root, so `admin.status` and every full turn see `FullRefusal::Root` without the
  test being root, and `config_watch()` turns the file watcher on, so a change of
  `config.toml` reloads by itself (with the trigger `file`) as in the real daemon; its
  quiet time of 200 ms runs on the test's clock. Without it, the file reloads only
  when a test asks (`admin.config_reload`, the project methods).
- `fake_launcher`: `FakeLauncher`, one `/bin/sh` script in place of `efr-sbx` for the
  tests of `efr-daemon`, `efr-cli` and the eval. `run --call-dir D` runs a call's line
  with `/bin/sh`; `run --call-dir D --job` runs a job as the real launcher's contract
  says: its own session, the start byte on stdin before anything starts, `D/started`,
  the line with stdin `/dev/null` and its output on stdout, `D/term-marker` and the end
  of the job at SIGTERM or SIGHUP, the end of the job at the end of its stdin, and
  `D/result.json` last. It closes the descriptors from 3 to 9 that it inherited, and it
  keeps each spec and each job's `env` in its own dir. `dbus-proxy --dir D -- PROGRAM`
  is the bus filter's wrapper without its Landlock domain: it changes to `D`, moves
  efrd's sync pipe from stdout to fd 3, and runs the program. Marker files there make it fail,
  ignore SIGTERM, stop itself, write no result or name a survivor. A test passes its
  path to `sandbox_launcher`; there is no second launcher seam.
- `fake_forwarder`: `FakeForwarder`, the forwarder of a job's port forwards
  (`efr-sbx forward`) as a test plays it, because the fake launcher starts none. It
  connects to the job's forward socket (from the spec that the fake launcher kept;
  `connect_socket` goes through `/proc/self/fd`, as the path is longer than a socket
  address holds), sends `Hello`, then `listen`, `gone` or any 28 bytes (`send_raw`). On
  each `Open` of efrd it opens a data connection, sends `Data` and copies bytes to the
  test's server that stands for the port, such as an `echo_server` on a free loopback
  port; for a port with no server it answers status 111. `closed` waits until efrd
  closed the control connection, and `opens` counts the `Open` frames. It encodes the
  frames by hand, as the layout of `efr_sandbox::Frame` says. The daemon binds each
  forwarded port through `TestHostPorts`: a free port of `127.0.0.1` in place of the
  job's port, which `TestDaemon::host_ports` names (`real_port`), and the machine's own
  tcp tables, where a test can make a port count as in use, add a listener on `::1` or
  show another uid. Only `TestDaemonBuilder::system_host_ports` binds the real ports.
- `fake_reach`: `FakeReach`, the reach helper of a contained call (`efr-sbx reach`) as
  a test plays it, because the fake launcher starts none. It connects to the call's
  reach socket (from the spec that the fake launcher kept, through `connect_socket`)
  and sends `Hello`. It keeps each `Reach` and `Unreach` of efrd (`told`,
  `told_at_least`), sends a `Bound` (`bound`) or any 28 bytes (`send_raw`), and
  `closed` waits until efrd closed the control connection. `open` is a process of the
  call that connects to a port: a data connection that sends `Connect` and then carries
  the bytes. It binds no TCP port, and it encodes the frames by hand.
- `fake_phone`: `FakePhone`, the phone app on the remote listener: a TCP stream to
  `remote_addr`, the opening handshake of a WebSocket with the origin of the app's
  web view (`connect`), another origin or none (`connect_with_origin`), or only the
  status line of the answer (`handshake`, empty for a peer that the listener refused
  with no byte). Then the protocol in text frames: `hello` as the phone,
  `call` and `result` for one item, `send` and `next_item` for a stream such as a
  subscription, `prove`, and `closed`, which waits until efrd closes the connection.
  Frames of other requests that come first are kept for their own read. Every read
  has a real time limit. `PhoneKey` is a software P-256 key in place of the app's
  Secure Enclave key: the 65-byte X9.63 public key, the fingerprint that the terminal
  shows (`SHA256:` and 8 bytes in hex), and the DER signature of `device.prove` over
  `ProveMessage`.
- `test_daemon/responses`: `ResponsesServer`, a wiremock server that answers
  `POST /v1/responses` from a queue of `ResponsesAnswer`s (a status and a body;
  `ResponsesAnswer::text` builds a whole streamed text answer, `ResponsesAnswer::tool_call`
  one function call sent whole, `ResponsesAnswer::custom_tool_call` one call of a
  freeform tool whose text streams in two deltas) and keeps every request,
  `Authorization` header included (never shown by `Debug`), and the time when each
  request arrived (`received_times`), for a bench. It also answers
  `GET /v1/models`, the model list that efrd fetches in the background and the check
  of a key at a login: with the `ModelsAnswer` of `set_models` (a catalog with its
  `ETag`, a bare status such as 304 or 503, a JSON body with a status such as the API
  key backend's ids or its refusal of a key, and a delay for a backend that hangs),
  else with a 404. It keeps each fetch as a `ModelsRequest` (`client_version`,
  `If-None-Match`, `originator`, `Authorization` (never shown by `Debug`),
  `OpenAI-Organization` and `OpenAI-Project`).
- `test_daemon/messages`: `MessagesServer`, a wiremock server in place of Anthropic's
  Messages API. It answers `POST /v1/messages` from a queue of `MessagesAnswer`s (a
  status, a body and an optional `retry-after`; `MessagesAnswer::text` streams one text
  block that ends the turn, `MessagesAnswer::tool_use` one tool call whose input
  streams in two parts, `MessagesAnswer::events` any scripted events, and
  `MessagesAnswer::error` an error body of the API), with a `request-id` header on
  each answer. It keeps every call as a `MessagesRequest` (the body, `Authorization`,
  `x-api-key`, `anthropic-version`, `anthropic-beta` and `anthropic-workspace-id`;
  `Debug` hides the key) and the time when each call arrived (`received_times`), for a
  bench. It also answers `GET /v1/models` in pages by `limit` (20
  when absent) and `after_id`, from the list of `set_models` (`MessagesServer::model`
  makes an active entry), or with the answer of `refuse_models`, else with a 404, and
  keeps each request as a `ModelsPageRequest`. The provider's catalog fetch and its
  key check both reach it.
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
  (`<TMP>`, `<CWD>`, `<HOME>`, `<TIMESTAMP>`). The commit and the time of a
  `snapshot_taken` are `<COMMIT>` and `<MS>`, because they change from run to run. A scenario in the Responses mode (OpenAI's wire)
  or the Messages mode (Anthropic's wire, the real `anthropic-api` provider) queues
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
`pty_attach_since_seq`. Later fixtures build on them: `messages_thinking_two_turns` runs
Claude over Anthropic's wire and checks that signed thinking goes back unchanged, the
first step to cross-provider tests. The tests in `crates/efr-daemon/tests/` replay them
and add the assertions of each case.

## Tier

Tier T. Kind `dev`, `publish = false`: only the `tests/` targets of `efr-daemon` and
`efr-cli` name it, under `[dev-dependencies]`.

## Allowed dependencies

`efr-daemon` (without its default `local-pty` feature, with `test-sandbox-fake` for the
sandbox's test seams), `efr-test-support`,
`efr-transport` (its `client` module) and `efr-protocol`. `xtask/src/deps.rs` holds the
allowlist. The holder types come through `efr-daemon`'s re-exports, so this crate needs no `efr-holder` edge.

Third-party crates: `tokio`, `tokio-util` (the daemon's shutdown token), `futures`,
`async-trait` (the holder trait), `serde` and `serde_json`, `fastwebsockets` and
`aws-lc-rs` (the fake phone's WebSocket and key), `jiff` (the time zone), `wiremock`
(the Responses and Messages servers) and `thiserror`.

## Invariant

- `efr_test_daemon` appears only in `tests/` directories and in this crate (a tidy
  rule), so no library's unit tests link a second copy of the daemon.
- A test daemon never touches the network, the real home, config or runtime directory,
  or the machine's git configuration, and never waits on real time: every deadline runs
  on its `TestClock`.
- A test daemon never reaches the machine's tailscaled: its remote listener talks to
  a `FakeTailscaled` or to a socket that does not exist, and binds only `127.0.0.1`.
- A job port forward of a test daemon binds a free port of `127.0.0.1`
  (`TestHostPorts`), never a fixed port of the machine, unless the test asks for
  `system_host_ports`.
- A fixture is the contract of its scenario: every outbound record is compared exactly
  after the placeholders, and bless changes outbound records only.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-test-daemon
```

The unit tests drive the fake holder over its socketpairs (typing, printing, scripts,
signals, `wait`, release), the id placeholders, the Responses and Messages servers
over plain HTTP (the Messages server's answers, its kept headers and its model pages),
the test daemon's start, stop and restart (with and without a file store), the fake
phone's key (a DER signature that aws-lc-rs verifies, the fingerprint) and a phone on
the remote listener of a test daemon (the hello, a refused origin, an unknown peer),
and the replay: the scenario table against the fixture directory, a whole scenario, and the
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
