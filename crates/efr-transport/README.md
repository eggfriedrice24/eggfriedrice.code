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
- `connection`: the per-connection loop, over a `FrameSource` and a `FrameSink`: the
  Unix socket's length-prefixed frames, or a WebSocket's text messages. Both get the
  same loop. It owns the request-id table: every request
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
- `context`: `ConnectionContext { surface, peer, conn_id }`, carried by every
  request. The surface comes from hello. The peer is `Peer::Local(PeerCred)`, whose
  uid and pid come from the kernel, or `Peer::Remote(RemotePeer)`: the socket address
  of a WebSocket peer and an opaque admission value that the daemon fills when it
  admits the connection. A remote peer's surface is always `phone`. `ConnId::next`
  gives the ids of every listener, so no two connections of a process share one.
- `codec`: `ServerCodec`, the tokio codec over `efr_protocol::framing`. Because an
  `input.respond` frame carries a password, it overwrites the bytes it read with zeros
  once the push decoder has copied them, and a frame's payload once it is decoded or
  dropped. Not zeroed: a frame left unfinished in the push decoder when the connection
  ends, and serde_json's scratch buffer for a string that holds an escape.

- `ws`: `serve_websocket`, the phone's side of the edge. efrd owns the TCP listener on
  the tailnet address and the check of the peer (the whois of tailscaled), and hands
  each admitted stream, with its `RemotePeer`, to `serve_websocket`:
  - `ws/handshake`: a pure parser of the opening handshake. The request head is at
    most 8 KiB with at most 32 headers. Only `GET /` over HTTP/1.1 with `Upgrade:
    websocket`, `Connection: upgrade`, version 13, one `Host` and a key of 16 bytes
    passes. The `Origin` must be missing or equal, byte for byte, to one of the
    origins of `WebSocketOptions` (efrd gives `http://<bound ip>:<port>`), so `null`
    and every web page are refused. A query string, a body, a repeated `Host`,
    `Origin`, key or version are refused too. Every other request gets a plain HTTP
    error (400, 403, 404, 405, 426, 431 or 505) with `Connection: close`, never a 200.
    The parser never reads a `Cookie` or `Authorization` header and copies no header
    value into an error, so nothing a client sends reaches a log. The head must come
    within 10 s on the injected clock.
  - `ws/frames`: the frames of fastwebsockets in server role, without extensions
    (no permessage-deflate) and without a subprotocol. One text message carries one
    JSON frame, the same frames as on the Unix socket; the sink sends the bare JSON
    body of each server frame. The client's messages are at most 1 MiB (the limit is
    set one byte higher, because fastwebsockets refuses a length equal to its
    limit), the server's at most 16 MiB (`MAX_FRAME_LEN`). A binary frame, a frame
    without FIN, a continuation frame (close 1003), a larger message (1009), text
    that is not UTF-8 (1007), an unmasked client frame and any other broken rule
    (1002) close the connection; `TransportError::WebSocketClosed` names the reason.
    fastwebsockets does not check the client's mask, so the reader checks the mask
    bit of every frame itself. The parser is not cancel safe, so a reader task fills
    a bounded queue that the connection loop reads.
  - `ws/keepalive`: a ping every 25 s on the injected clock; when two pings wait for
    their pong at a tick, the connection closes with 1001. A pong answers its ping
    and every older one.

  The shutdown token that `serve_websocket` takes belongs to one connection: efrd
  gives each connection a child of its own shutdown, so it can close one connection
  alone, such as those of a revoked device. Once it is cancelled, the frames still
  queued are dropped and the peer gets close code 1001.

  A frame's payload is zeroed once it is decoded. Not zeroed: the bytes left in the
  parser's read buffer when it grows, and serde_json's scratch buffer. So efrd must
  not take a password over the WebSocket.

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
of the client), `fastwebsockets` (the WebSocket frames, server role), `httparse` (the
handshake's request head), `sha1` and `base64` (the handshake's accept value).

The client's tests build their fake daemons from the real server codec and run one
test against the real listener, so both sides of the protocol are checked against
each other.

## Invariant

- Only processes of the daemon's own uid ever exchange a frame with it: the socket is
  0600 from the moment it has its name and unreachable to others before that, and each
  peer's kernel-reported uid is checked before its first frame is read.
- A WebSocket peer exchanges a frame only after the daemon admitted its stream and
  its handshake passed, with an allowed origin or none. A remote peer's surface is
  always `phone`, and it has no uid or pid.
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
temporary directory. The WebSocket tests drive `serve_websocket` over an in-memory
pipe with a raw client that encodes and decodes frames by hand, so they check the bytes
on the wire; a test runs one session over both wires and compares the frames. The
handshake parser has property tests (every prefix of a valid head is partial, the order
and case of headers do not matter, any bytes never panic). The ping tests use a manual
clock whose sleeps end only when the test fires them, after the code under test
sleeps. The codec has a proptest that any split of a byte stream decodes
to the same frames. The tests use no network, no real-time sleeps (fake clocks drive
the close grace) and no Zig.

The client's tests mostly run against a scripted fake daemon on an in-memory pipe, built from
the crate's `ServerCodec`. One test runs against the real `UnixListener`
on a socket in a temporary directory, and the discovery tests use temporary XDG
directories. The codec has a proptest that any split of a byte stream decodes to the
same frames. The tests use no network, no real-time sleeps and no Zig.

`cargo nextest run -p efr-transport -E 'test(/^client::/)'` runs only the client's
tests.
