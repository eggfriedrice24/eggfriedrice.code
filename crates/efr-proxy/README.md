# efr-proxy

## Purpose

The network proxy of the `auto` sandbox (spec 8.2, phase D2). A contained call has no
network of its own. efrd serves one Unix socket for each such call, and the call's
bridge (`efr-sbx bridge`) carries the tools' connections to it. The proxy lets a
connection through only to a host of the allow list or of the call's host grants,
only to a global address, and only up to the call's upload budget.

The crate is a library. Nothing in it runs until efrd starts it.

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

## The address guard

`classify(addr, own)` gives the `AddressClass` of an address. It first unwraps the
IPv4-mapped, IPv4-compatible and NAT64 forms. An address is `Global` only outside
every block of the IANA special-purpose registries (and for IPv6 inside `2000::/3`).
An address of the host is `Own`, and an address in the prefix of one of them is
`OnLink`. `Guard::strict()` passes only `Global`, and for a `private = true` entry
also `Private`, `Shared`, `UniqueLocal` and `OnLink`. `Guard::with_loopback()` also
passes `Loopback`; only tests use it.

## Tier

Tier 2.

## Allowed dependencies

`efr-protocol` (the refusal reasons `Blocked` and `BlockReason`, the call id),
`efr-permissions` (the host list `HostList` and the host syntax). `xtask/src/deps.rs`
holds the allowlist. Only `efr-daemon` may depend on this crate.

## Invariant

The proxy decides on the allow and deny lists before any DNS lookup, connects only to
an address that the guard checked, and forwards no client byte of a tunnel before the
ClientHello names the host of the `CONNECT`.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-proxy
```

The unit tests sit next to each module (`src/<module>/tests.rs`). ClientHello bytes
come from `rustls::ClientConnection` written into a `Vec` (no server, no network) and
from byte fixtures that the tests build, which do not change with the rustls version.
No test reaches a real host or the real resolver.
