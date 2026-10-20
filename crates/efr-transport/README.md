# efr-transport

## Purpose

Both sides of the protocol's transport. The crate root is the daemon's protocol edge,
without the engine, and the module `client` is the client side (see "The client"
below).

The daemon's edge turns sockets into requests for the daemon and the daemon's answers
back into frames:

- `unix_listener`: `UnixListener`, the socket at `$XDG_RUNTIME_DIR/efr/daemon.sock`.
  It is bound inside a fresh staging directory of mode 0700 next to its path, set to
  mode 0600 there and renamed into place, so no other user can reach it while it has
  the umask's mode. The staged socket is `.s<pid>/s`, never longer than
  `daemon.sock`, so any socket path that fits the 107 bytes of a socket address binds;
  a longer path fails with `PathTooLong`, which names it, before anything is created.
  A missing socket directory is created with mode 0700. Every accepted peer's uid, from `SO_PEERCRED`, must be the daemon's own.
  A stale socket is replaced; a socket that answers is never taken over. `serve` runs
  one task per connection and, on shutdown, cancels everything and removes the file.
- `connection`: the per-connection loop. It owns the request-id table: every request
  runs in its own task, a `{cancel: id}` frame drops that request's handler and ends
  it with `cancelled`, and when the client goes away or the daemon stops, every
  request in flight is cancelled. A slow reader fills only its own bounded outbound
  queue; a closing connection gets a short grace period on the injected clock to
  flush.
- `hello`: the decision table for the first request. Nothing reaches the dispatcher
  before a hello with the same `PROTOCOL_VERSION`; a mismatch is answered with
  `protocol_mismatch` and the connection closes.
- `dispatch`: the `Dispatcher` trait that `efr-daemon` implements (`hello`,
  `dispatch`, and `closed`, which reports once that a connection with an accepted
  hello has closed, after all its handlers are gone), the `Request` it receives and
  the `Responder` it answers through. The transport sends the one frame that ends each
  request and enforces that a unary method sends exactly one result. A responder that
  a handler moved into a task of its own is refused with `RequestEnded` once the
  request has ended, so no item ever follows the end frame.
- `subscriptions`: bounded per-subscriber queues of 64 items. An offer never waits;
  overflow closes that subscription, which then delivers what was queued and ends with
  `overflow` carrying `last_seq`, so the client resubscribes without a gap. A lossy
  item, such as a draft of a running turn, has no sequence number and its own room of
  16 items in the same queue, so the order of all items holds: a full room drops the
  item, and a lossy item never takes the room of a sequenced one, so it never causes
  an overflow.
- `context`: `ConnectionContext { surface, uid, pid, conn_id }`, carried by every
  request. The uid and pid come from the kernel, the surface from hello.
- `codec`: `ServerCodec`, the tokio codec over `efr_protocol::framing`. Because an
  `input.respond` frame carries a password, it overwrites the bytes it read with zeros
  once the push decoder has copied them, and a frame's payload once it is decoded or
  dropped. Not zeroed: a frame left unfinished in the push decoder when the connection
  ends, and serde_json's scratch buffer for a string that holds an escape.

The WebSocket listener for the phone lands here in a later milestone and reuses the
connection loop.

### The client

The module `efr_transport::client` (`src/client.rs` and `src/client/`) is the client
side of the efr protocol. `efr` uses it for every command, `efr-test-daemon` uses it
to drive the real daemon in tests, and later the PTY proxy and the WebSocket tests use
it too.

- `client/discovery`: `discover` reads `$XDG_RUNTIME_DIR/efr/daemon.json`
  (`DaemonInfo`: `{pid, socket, protocol, daemon_id, tailnet_endpoint?}`) to find the socket, and
  falls back to the default socket path when the file is missing. A daemon of another
  protocol version is refused before connecting.
- `client/unix`: connects to the socket with a timeout on the injected clock. A
  missing socket and a refused connection both mean `DaemonNotRunning`. A socket path longer than a
  socket address holds (`efr_stdx::paths::MAX_SOCKET_PATH`) is `SocketPathTooLong`
  before any connect, because the operating system's error does not name the limit.
- `client/connection`: `Client::connect` connects and says hello (`ConnectOptions`
  carries the origin, the client name, the tty and the timeout). Both a refused hello and an answer
  with another version are `ProtocolMismatch`. `call` runs a unary method and decodes
  its result; `stream` returns an `ItemStream` of decoded items; `cancel` sends
  `{cancel: id}`. Dropping a call or a stream before it ends cancels it on the daemon.
  Requests run concurrently on one connection and are matched by id. Every request frame
  is encoded into a buffer that is overwritten with zeros when it is dropped, once it is
  written or when the connection closes first, because an `input.respond` frame carries
  a password. Two copies are not zeroed: `efr_protocol::framing::encode` grows its
  buffer as the JSON is written, and each move leaves the bytes written so far in freed
  memory; and the connection's write buffer (tokio's `BufWriter`, 8 KiB) copies a
  smaller frame and keeps it until later frames overwrite it or the connection ends,
  when it is freed unzeroed.
- `client/codec`: `ClientCodec`, the tokio codec over `efr_protocol::framing`.

A consumer that falls behind is never buffered without limit: when a request's queue
of 256 frames is full, the client cancels the request on the daemon and the stream ends
with `StreamOverflow` after the items already queued, the same rule the daemon applies
to its subscribers.

The client was a crate of its own until ADR 0014 folded it into this one. The server
side adds only `nix` and `tracing` to what the client needs, and `efr` already links
`tracing`, so the client has no cargo feature: `efr` links the server side too, and
the linker drops the code that it does not call.

## Tier

Tier 3 by role (the protocol edge next to the engine, and the client edge), but it
depends only on tier 0.

## Allowed dependencies

`efr-protocol` (frames, framing, methods, error codes) and `efr-stdx` (the `Clock`, and
the XDG paths of the client's discovery).
`xtask/src/deps.rs` holds the allowlist. `efr-transport -> efr-store` is a forbidden
edge: the transport never touches the database.

Third-party crates: `tokio`, `tokio-util` (codec and `CancellationToken`), `futures`,
`bytes`, `nix` (`SO_PEERCRED` and `getuid`), `serde`, `serde_json`, `thiserror`,
`tracing`, `zeroize` (the bytes of client frames on the server, and the request frames
of the client).

The client's tests build their fake daemons from the real server codec and run one
test against the real listener, so both sides of the protocol are checked against
each other.

## Invariant

- Only processes of the daemon's own uid ever exchange a frame with it: the socket is
  0600 from the moment it has its name and unreachable to others before that, and each
  peer's kernel-reported uid is checked before its first frame is read.
- No method runs on a connection whose protocol version was not checked.
- Every request ends with exactly one `end` or `error` frame, and no frame of a request
  follows it. An id is free for reuse by the time the client reads that frame.
- A dead or slow client never pins resources or slows a producer: closing cancels its
  requests and then tells the dispatcher through `Dispatcher::closed`, so per
  connection state such as a lease is dropped, and a full subscriber queue closes that
  subscriber with `overflow`.
- The transport knows no method's meaning and no daemon error; mapping those to wire
  errors stays in `efr-daemon/src/error.rs`.

The client:

- The client never speaks to a daemon of another protocol version: it checks
  `daemon.json` before connecting and the hello answer after.
- Every request ends exactly once for its caller (a result, an error, or `Closed` when
  the connection ends), and a dropped caller never leaves the daemon working for
  nobody: the request is cancelled.
- A slow consumer costs bounded memory and never stalls the other requests on the
  connection.

The `daemon.json` shape is written by `efr-daemon/src/discovery.rs`, which keeps a
definition of its own; the two definitions must stay in step.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-transport
```

The connection tests drive the real connection loop over an in-memory pipe with a raw
frame client and a scripted dispatcher; the listener tests use real sockets in a
temporary directory. The codec has a proptest that any split of a byte stream decodes
to the same frames. The tests use no network, no real-time sleeps (fake clocks drive
the close grace) and no Zig.

The client's tests mostly run against a scripted fake daemon on an in-memory pipe, built from
the crate's `ServerCodec`. One test runs against the real `UnixListener`
on a socket in a temporary directory, and the discovery tests use temporary XDG
directories. The codec has a proptest that any split of a byte stream decodes to the
same frames. The tests use no network, no real-time sleeps and no Zig.

`cargo nextest run -p efr-transport -E 'test(/^client::/)'` runs only the client's
tests.
