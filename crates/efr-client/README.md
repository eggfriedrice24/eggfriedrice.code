# efr-client

## Purpose

The client side of the efr protocol. `efr` uses it for every command, `efr-test-daemon`
uses it to drive the real daemon in tests, and later the PTY proxy and the WebSocket
tests use it too.

- `discovery`: `discover` reads `$XDG_RUNTIME_DIR/efr/daemon.json` (`DaemonInfo`:
  `{pid, socket, protocol, daemon_id, tailnet_endpoint?}`) to find the socket, and
  falls back to the default socket path when the file is missing. A daemon of another
  protocol version is refused before connecting.
- `unix`: connects to the socket with a timeout on the injected clock. A missing socket
  and a refused connection both mean `DaemonNotRunning`.
- `client`: `Client::connect` connects and says hello (`ConnectOptions` carries the
  origin, the client name, the tty and the timeout). Both a refused hello and an answer
  with another version are `ProtocolMismatch`. `call` runs a unary method and decodes
  its result; `stream` returns an `ItemStream` of decoded items; `cancel` sends
  `{cancel: id}`. Dropping a call or a stream before it ends cancels it on the daemon.
  Requests run concurrently on one connection and are matched by id.
- `codec`: `ClientCodec`, the tokio codec over `efr_protocol::framing`.

A consumer that falls behind is never buffered without limit: when a request's queue
of 256 frames is full, the client cancels the request on the daemon and the stream ends
with `StreamOverflow` after the items already queued, the same rule the daemon applies
to its subscribers.

## Tier

Tier 3 by role (the client edge), but it depends only on tier 0.

## Allowed dependencies

`efr-protocol` (frames, framing, methods) and `efr-stdx` (the `Clock` and the XDG
paths). `xtask/src/deps.rs` holds the allowlist. `efr-transport` is a dev-dependency
only: the tests build their fake daemons from the real server codec and run one test
against the real listener, so both sides of the protocol are checked against each
other.

Third-party crates: `tokio`, `tokio-util` (codec), `futures`, `bytes`, `serde`,
`serde_json`, `thiserror`.

## Invariant

- The client never speaks to a daemon of another protocol version: it checks
  `daemon.json` before connecting and the hello answer after.
- Every request ends exactly once for its caller (a result, an error, or `Closed` when
  the connection ends), and a dropped caller never leaves the daemon working for
  nobody: the request is cancelled.
- A slow consumer costs bounded memory and never stalls the other requests on the
  connection.

The `daemon.json` shape is written by `efr-daemon/src/discovery.rs`, which may not
depend on this crate; the two definitions must stay in step.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-client
```

Most client tests run against a scripted fake daemon on an in-memory pipe, built from
`efr_transport::ServerCodec`. One test runs against the real `efr_transport::UnixListener`
on a socket in a temporary directory, and the discovery tests use temporary XDG
directories. The codec has a proptest that any split of a byte stream decodes to the
same frames. The tests use no network, no real-time sleeps and no Zig.
