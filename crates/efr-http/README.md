# efr-http

## Purpose

Everything efr needs to speak HTTP, with no knowledge of any provider:

- `client`: `HttpClient`, the one outbound client. reqwest over rustls (no OpenSSL;
  `deny.toml` bans it), with a connect timeout, a read timeout that catches a stalled
  stream without limiting a healthy one, and a cap on bodies read into memory. Clones
  share one connection pool.
- `request` and `response`: `HttpRequest` (built in memory, cheap to clone, so a
  retry resends it; `Debug` never shows the body) and `HttpResponse` (status and
  headers, with the body read whole, as chunks or as server-sent events).
- `retry`: `RetryPolicy`, exponential backoff with equal jitter, `Retry-After` (as
  seconds or an HTTP date) and `retry-after-ms`. It sleeps on the injected
  `efr_stdx::time::Clock` and draws jitter from the injected `efr_stdx::rng::Rng`.
  A request is sent again only when the server certainly did not act on it: no
  connection could be made (DNS, TCP, TLS handshake, connect timeout), or the server
  answered 408, 429 or 503. A timeout, a connection that broke after the request
  went out, and 500, 502 or 504 are retried only for an idempotent request (`GET`,
  `PUT`, `DELETE` and the like, or one marked `idempotent()`), so a `POST` to the
  Responses API never runs twice.
- `sse`: `SseDecoder` and `SseStream`, the WHATWG event stream rules over chunks that
  split anywhere (inside a line, a CRLF pair, a UTF-8 sequence or the byte order
  mark).
- `websocket`: `WebSocket`, a WebSocket client connection. `HttpClient::websocket`
  sends the opening handshake of RFC 6455 as an HTTP/1.1 `GET` through a second
  reqwest client with the same settings over HTTP/1.1 only (a server that picks HTTP/2
  cannot upgrade), checks `Sec-WebSocket-Accept`, and hands the upgraded connection to
  `fastwebsockets`. A reader task collects whole messages and answers pings, a writer
  task sends frames in order, so `WebSocket::next` is safe to cancel.
  `WebSocket::ping` sends a ping whose payload is its number and waits for the pong
  with that number (or a newer one); it fails when the connection ends first, and the
  caller bounds the wait. The handshake
  key comes from the injected `Rng`. The handshake is never retried and never
  recorded.
- `unix`: `UnixClient`, HTTP/1.1 over a Unix socket with hyper and hyper-util. Its
  timeout runs on the injected clock. The caller names the socket; this crate has no
  default path.
- `json_lines`: `JsonLines`, newline-delimited JSON over a body that may never end
  (`HttpResponse::into_json_lines`). It holds one line at a time and refuses a line
  longer than its cap (`HttpError::LineTooLarge`) before the rest of the line
  arrives. `next_line` and `next_json` are cancel safe.
- `localapi`: `LocalApi`, a small typed client of tailscaled's LocalAPI over
  `UnixClient`, for efrd's remote listener. `status` sends
  `GET /localapi/v0/status?peers=false` and keeps `BackendState`, `TUN`,
  `TailscaleIPs`, `Self` (its `ID`, the stable node id) and `CurrentTailnet` (its
  `StableID`) in a `TailscaleStatus`. `whois` sends
  `GET /localapi/v0/whois?addr=<ip:port>&proto=tcp` (an IPv4 address mapped into
  IPv6 is unmapped first) and keeps `Node.StableID`, `Node.Name`, `Node.Tags`,
  `Node.Hostinfo.ShareeNode`, `Node.Hostinfo.Hostname` and `UserProfile.ID`,
  `LoginName` and `DisplayName` in a `PeerIdentity`; it never reads the numeric
  `Node.ID`, which can pass 2^53. A 404 is `None` (an unknown peer, which a caller
  refuses); another failure status is `HttpError::LocalApiStatus`, and a reply
  without a node, a user or a stable node id is an error, never a partial identity.
  `watch` sends `GET /localapi/v0/watch-ipn-bus?mask=16386` (`WATCH_MASK`:
  `NotifyInitialState` and `NotifyInitialStatus`, never bit 8, which tailscaled
  refuses) and returns an `IpnWatch`, whose `next` reads one `IpnNotify` per line
  (`Version`, `ErrMessage`, `State`, `SelfChange`, `InitialStatus`) with a cap of
  8 MiB per line. A reader that falls behind gets an `ErrMessage` and then the end of
  the stream (`IpnNotify::fell_behind`). `TailscaleVersion` reads the
  `Tailscale-Version` header (each result keeps it) and `MINIMUM_REMOTE` (1.100.0)
  is the oldest tailscaled that efrd's remote listener supports. What efrd admits or
  binds from these answers is the daemon's decision, not this crate's.
- `redact`: header and URL redaction for logs, errors and transcripts.
- `recorder`: the `Recorder` hook that receives provider traffic, for capturing a
  transcript later; no binary installs one yet. Recording is opt-in per request.

Consumers: `efr_providers::openai` (the streaming Responses API over HTTP and over a
WebSocket), `efr-oauth-openai`
(token exchange and refresh), and efrd's remote listener (`LocalApi`).

## Tier

Tier 1.

## Allowed dependencies

`efr-stdx` only (the `Clock` and `Rng` traits). `xtask/src/deps.rs` holds the
allowlist.

Third-party crates: `reqwest` (with `rustls`, `http2`, `json`, `stream`), `hyper`,
`hyper-util`, `http-body-util`, `http`, `tokio`, `bytes`, `futures`, `url`, `jiff`,
`secrecy`, `zeroize`, `serde`, `serde_json`, `thiserror`, `fastwebsockets` (the frames
of a WebSocket, with `unstable-split` and without its default `simd`; it pulls no
hyper or TLS of its own), `sha1` and `base64` (the accept value of a WebSocket
handshake). The `http` types in the API
are re-exported, so callers need no direct dependency on `http`, `reqwest` or `hyper`.

## Invariant

- Nothing secret leaves through a log, an error or a transcript: every URL in an
  error or a record goes through `redact::url`, reqwest errors are stored without
  their URL, header values are never stored in errors, recorded headers go through
  `redact::header_map`, and only requests marked `recorded()` reach the recorder, so
  a token exchange is never recorded by accident.
- Retries and the Unix-socket timeout wait on the injected clock, never on a real
  timer, so tests drive them without sleeping.
- A request that is not idempotent is never sent a second time after it may have
  reached the server.
- The crate knows no model vendor: endpoints, provider headers and event names belong
  to the provider crates. The one service it knows is tailscaled's LocalAPI, a local
  daemon on a Unix socket, because the remote listener needs it below `efr-daemon`.
- No LocalAPI request sends an `Origin` or `Referer` header, and each one sends
  `Host: local-tailscaled.sock`, which tailscaled requires.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-http
```

The client tests talk to a `wiremock` server on the loopback interface and the
Unix-socket and LocalAPI tests to a fake server on a socket in a temporary directory
(never the real tailscaled); the WebSocket
tests run a fake server on the loopback interface that answers the handshake by hand
and frames with `fastwebsockets`. Nothing leaves the machine. The SSE decoder has a proptest that checks that any split of a
stream into chunks yields the same events, and `JsonLines` has one that checks the same
for lines. The tests use no real-time sleeps and no
Zig.
