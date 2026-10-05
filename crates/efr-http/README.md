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
- `unix`: `UnixClient`, HTTP/1.1 over a Unix socket with hyper and hyper-util, for
  tailscaled's LocalAPI at the phone milestone. Its timeout runs on the injected
  clock.
- `redact`: header and URL redaction for logs, errors and transcripts.
- `recorder`: the `Recorder` hook that receives provider traffic, for capturing a
  transcript later; no binary installs one yet. Recording is opt-in per request.

Consumers: `efr-provider-openai` (the streaming Responses API), `efr-oauth-openai`
(token exchange and refresh), and later the daemon's tailnet whois check.

## Tier

Tier 1.

## Allowed dependencies

`efr-stdx` only (the `Clock` and `Rng` traits). `xtask/src/deps.rs` holds the
allowlist.

Third-party crates: `reqwest` (with `rustls`, `http2`, `json`, `stream`), `hyper`,
`hyper-util`, `http-body-util`, `http`, `tokio`, `bytes`, `futures`, `url`, `jiff`,
`secrecy`, `zeroize`, `serde`, `serde_json`, `thiserror`. The `http` types in the API
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
- The crate knows no vendor: endpoints, provider headers and event names belong to the
  provider crates.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-http
```

The client tests talk to a `wiremock` server on the loopback interface and the
Unix-socket tests to a fake server on a socket in a temporary directory; nothing
leaves the machine. The SSE decoder has a proptest that checks that any split of a
stream into chunks yields the same events. The tests use no real-time sleeps and no
Zig.
