# efr-proxy

## Purpose

The network proxy of the `auto` sandbox (spec 8.2, phase D2). A contained call has no
network of its own. efrd serves one Unix socket for each such call, and the call's
bridge (`efr-sbx bridge`) carries the tools' connections to it. The proxy lets a
connection through only to a host of the allow list or of the call's host grants,
only to a global address, and only up to the call's upload budget.

The crate is a library that runs as tokio tasks in efrd. Nothing in it runs until
efrd starts it.

## API contract

- `Proxy::new(deps, hosts)`: `deps` is a `Deps` (below); `hosts` is a
  `watch::Receiver<Arc<HostList>>`, the live global list. A reload sends a new list,
  and every request reads the newest one.
- `Proxy::serve(listener, policy) -> CallProxy` serves one call's `UnixListener` on the
  current runtime. `CallPolicy` holds the call id, the call's `Grant::Host` entries
  (one host and one port each), the call's own deny set and its upload budget. The
  policy is frozen; only the global list is live.
- `CallProxy::blocked()` gives the refusals so far. `CallProxy::close(grace)` stops the
  accept, waits at most `grace` for the open connections, cuts the rest, logs one debug
  line `phase="proxy" call=<id> tunnels=N bytes_up=N bytes_down=N blocked=N` and gives
  the `CallReport`. Dropping a `CallProxy` aborts every task of the call.
- `CallReport` holds at most `Limits::blocked` refusal records (`Blocked`, one for each
  host, port and reason, with a count), `blocked_more` for the rest, the bytes up and
  down, and the count of tunnels and plain requests that reached their upstream. A
  record names a host only when the host passed the host syntax; else the host is
  `<invalid>` and the port 0.

## What a request goes through

A `CONNECT host:port` (DP6):

1. The request head: at most `Limits::head_bytes` (8 KiB), within
   `Limits::head_timeout` (10 s) of the accept. A malformed head gets 400
   `bad_request`; a slow one gets 408 and no record.
2. The host form: an IP address that no `[[network.hosts]]` entry and no grant lists
   gets 403 `ip_literal`.
3. The budget: a spent budget gets 403 `upload_limit`.
4. The lists: the call's deny set and the deny list win, then the global list, then
   the call's grants. A host that a deny holds gets 403 `denied`, and one that none of
   them allows gets 403 `not_allowed`. Port 80 means plain `GET` and `HEAD` only, so a
   tunnel there gets 403 `method`. No lookup happens before this step, also not for a
   host that the proxy refuses.
5. The lookup: at most `Limits::resolve_timeout` (5 s), with at most 4 lookups of the
   call and 32 of the proxy at once.
6. The guard over every address: one address that fails refuses the name with 403
   `private_address`.
7. The connect to at most 4 checked addresses in order, 10 s each and 15 s in all.
   No second lookup. A failure gets 502 with the reason (`no such host`, `refused`,
   `timed out`, `unreachable`) and no record.
8. `200 Connection established`, then the ClientHello: at most `Limits::hello_bytes`
   (16 KiB) within `Limits::hello_timeout` (10 s). No hello, no server name, an
   Encrypted Client Hello or encrypted SNI extension cut the tunnel with `not_tls`; a
   server name that differs from the host (without case and one trailing dot) cuts it
   with `sni_mismatch`. A tunnel to a listed IP address needs a hello with no server
   name. No client byte leaves before this check.
9. The splice both ways. Each client byte after the `CONNECT` head, the hello too,
   takes its place in the call's budget first; the proxy forwards at most the rest of
   the budget, records `upload_limit` and cuts the tunnel.

A plain request (DP8): `GET` and `HEAD` only (else 405 `method`), only to an http
host: an entry with `http = true`, an `http://` pacman mirror, or a grant on port 80
(else 403 `method`). A byte after the head in the first reads is a pipelined request
and gets 400. The proxy reads no client byte after the head, writes the upstream head
itself (`head::upstream_head`), and sends the upstream's answer back until the
upstream closes.

A call has at most `Limits::connections` (64) open connections, counted from the
accept; the next one gets 503 and a `limit` record.

## Parsers

The proxy reads untrusted bytes in efrd, which has no sandbox. So it parses only what
it must, with hand parsers of a strict subset and hard caps. No TLS library and no
third-party HTTP parser touches these bytes.

- `head::parse(bytes, limit)` reads the request head: at most `limit` bytes up to the
  first `CRLF CRLF`, visible ASCII only, `CONNECT host:port` in the authority form or
  `GET` and `HEAD` in the absolute form `http://host[:port]/path`. A control
  character, a folded line, user info, `%` or brackets in a name, a fragment, a
  `Host` header that names another host or port, a second `Host`, `Content-Length`
  or `Transfer-Encoding`, and a body on `GET`, `HEAD` or `CONNECT` are refused with
  `BlockReason::BadRequest`. `head::upstream_head` writes the head that a plain
  request sends upstream: the method, the origin-form target, `Host`,
  `Connection: close` and only the headers of `head::FORWARDED`.
- `hello::parse(bytes)` reads the TLS ClientHello of a tunnel: handshake records
  joined, every length checked against its parent, each extension type at most
  once. It gives the `host_name` of the `server_name` extension and marks the
  Encrypted Client Hello and encrypted SNI extensions (`0xfe08` to `0xfe0d`,
  `0xffce`). A first byte that cannot start a TLS record is `Hello::NotTls` at once.

Both never panic and never read past their slice. Proptests check it.

The proxy checks only the first ClientHello of a tunnel. A second hello after a
HelloRetryRequest, a TLS 1.2 renegotiation and an HTTP/2 `:authority` inside the
tunnel are not checked.

## The address guard

`classify(addr, own)` gives the `AddressClass` of an address. It first unwraps the
IPv4-mapped, IPv4-compatible and NAT64 forms. An address is `Global` only outside
every block of the IANA special-purpose registries (and for IPv6 inside `2000::/3`).
An address of the host is `Own`, and an address in the prefix of one of them is
`OnLink`. `Guard::strict()` passes only `Global`, and for a `private = true` entry
also `Private`, `Shared`, `UniqueLocal` and `OnLink`. `Guard::with_loopback()` also
passes `Loopback`; only tests use it.

## Seams

`Deps` holds the `Resolver`, the `Connector`, the `OwnAddresses`, the `Guard` and the
`Limits`. They are constructor arguments with no default, so a test always says which
ones it uses. The real ones are `SystemResolver` (`tokio::net::lookup_host`, every
address of every family), `SystemConnector` (a TCP connect) and `SystemAddresses`
(`getifaddrs` with the netmask as the prefix length, read again at most once a
second, because VPN and DHCP change them). `src/system.rs` is the one file that calls
the resolver and the connect of the system. Only efrd builds them; no test does.

Time limits go through `efr_stdx::time::SystemClock`. No test uses a paused clock: a
test sets short `Limits`, such as 200 ms.

## Tier

Tier 2.

## Allowed dependencies

`efr-protocol` (the call id, the refusal reasons `Blocked` and `BlockReason`),
`efr-permissions` (the host list `HostList` and the host syntax), `efr-stdx` (the
clock). Third-party crates: `tokio`, `nix` (with its `net` feature, for
`getifaddrs`) and `tracing`. `xtask/src/deps.rs` holds the allowlist. Only
`efr-daemon` may depend on this crate.

## Invariant

The proxy decides on the allow and deny lists before any DNS lookup, connects only to
an address that the guard checked, forwards no client byte of a tunnel before the
ClientHello names the host of the `CONNECT`, and forwards no client byte over the
call's upload budget.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-proxy
```

The unit tests sit next to each module (`src/<module>/tests.rs`). The integration
binary `tests/it` serves real Unix sockets with the test kit of `tests/it/kit.rs`: a
table resolver, a connector that reaches only the test's own loopback servers, fixed
own addresses and the constants `GLOBAL_V4`, `GLOBAL_V4_OWN`, `GLOBAL_V6` and
`GLOBAL_V6_OWN` (addresses outside both special-purpose registries, which the test
connector refuses). ClientHello bytes come from `rustls::ClientConnection` written into
a `Vec` (no server, no network) and from byte fixtures that the tests build, which do
not change with the rustls version. A test asserts what the upstream server got,
never the proxy's own log. No test reaches a real host or the real resolver.
