# efr-transport

## Purpose

The daemon's protocol edge, without the engine. It turns sockets into requests for
the daemon and the daemon's answers back into frames:

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
  `overflow` carrying `last_seq`, so the client resubscribes without a gap.
- `context`: `ConnectionContext { surface, uid, pid, conn_id }`, carried by every
  request. The uid and pid come from the kernel, the surface from hello.
- `codec`: `ServerCodec`, the tokio codec over `efr_protocol::framing`. Because an
  `input.respond` frame carries a password, it overwrites the bytes it read with zeros
  once the push decoder has copied them, and a frame's payload once it is decoded or
  dropped. Not zeroed: a frame left unfinished in the push decoder when the connection
  ends, and serde_json's scratch buffer for a string that holds an escape.

The WebSocket listener for the phone lands here in a later milestone and reuses the
connection loop.

## Tier

Tier 3 by role (the protocol edge next to the engine), but it depends only on tier 0.

## Allowed dependencies

`efr-protocol` (frames, framing, methods, error codes) and `efr-stdx` (the `Clock`).
`xtask/src/deps.rs` holds the allowlist. `efr-transport -> efr-store` is a forbidden
edge: the transport never touches the database.

Third-party crates: `tokio`, `tokio-util` (codec and `CancellationToken`), `futures`,
`bytes`, `nix` (`SO_PEERCRED` and `getuid`), `serde`, `serde_json`, `thiserror`,
`tracing`, `zeroize` (the bytes of client frames).

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
