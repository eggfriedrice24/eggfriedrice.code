# 0012: A network proxy in efrd, with a bridge in each call

Status: accepted, 2026-10-19.

## Context

Before phase D2, a contained call of `auto` had no network. Every network need was a
`host` exit, and an allowed `host` exit gave the call the whole network of the
machine. So `cargo build` with a new dependency, `npm install` and `pip install` went
to the reviewer each time, and an allow opened far more than the registry that the
command needed. The auto spec (section 8) asks for a proxy that lets a contained call
reach a short list of hosts, with no review, and keeps every other route closed.

These choices are expensive to reverse: the proxy parses bytes from the sandbox in
efrd, which runs with the user's full rights, and the bridge is a process inside each
call that can reach a socket of efrd.

## Decision

1. **The proxy is a library in efrd, not a process of its own.** `efr-proxy` runs as
   tokio tasks in efrd, with one listener for each contained call. A process of its own
   would need a new binary, a lifecycle and a protocol between it and efrd. The risk
   of a parser bug (R23) stays small: efr parses only the request head and the TLS
   ClientHello, with hand parsers of a strict subset, hard caps and proptest targets.
   No TLS library and no third-party HTTP parser touches the bytes from the sandbox.
   After the checks, the proxy copies opaque bytes. `efr-proxy` has
   `#![forbid(unsafe_code)]` and may not reach `efr-daemon`.

2. **One Unix socket for each call, bound read-only into the call.** efrd binds
   `$CALL/proxy.sock` (mode 0600) through `/proc/self/fd/<dirfd>/proxy.sock`, because
   the path of a job's socket can be longer than a socket address holds. The plan binds
   it read-only at `$XRT/efr-proxy.sock`, so the child cannot `chmod` the host's
   socket. The child's Landlock domain has no `RESOLVE_UNIX` rule for it: only the
   bridge can connect. The socket of a foreground call closes when the call ends (200
   ms of grace for open tunnels), the socket of a call past its timeout when its
   `result.json` comes or its dir goes, and the socket of a job when the job ends.

3. **A bridge in the call, with its own confinement.** Tools speak TCP to a proxy, and
   the call has no network but loopback. So the inner stage binds `127.0.0.1:3128` and
   starts `efr-sbx bridge` before its own Landlock, with two `Command` spawns: a
   starter and a server. The workspace denies `unsafe_code`, so there is no `fork`.
   The server closes every other descriptor, calls `setsid`, names itself
   `efr-bridge`, turns off dumps, applies its own Landlock domain (no write right,
   `RESOLVE_UNIX` only on the socket, the signal and abstract socket scopes) and its
   own seccomp profile (only `AF_UNIX` sockets; no `bind`, `listen`, `execve` or
   `ptrace`). Only then does it write a ready byte. A setup error ends it before it
   serves, and the call fails to start. The bridge costs about 0.6 ms of launch time
   for each call (DP34, accepted by the user).

4. **The checks run in a fixed order, and the lists come before any lookup.** For a
   `CONNECT`: the head (8 KiB, 10 s), control characters, the host form (an IP literal
   only from `[[network.hosts]]`), the upload budget, the port rule (port 80 means
   plain `GET` and `HEAD`, never a tunnel), the lists (deny wins, then the allow list,
   then the call's host grants). A host that is not listed gets 403 with no DNS
   lookup, so a `*.suffix` entry or an unlisted name never becomes a DNS channel. Then
   the lookup (5 s), the address guard, a connect to a checked address only, `200
   Connection established`, and the ClientHello (16 KiB, 10 s): its server name must
   equal the host. A hello with no server name, bytes that are not TLS, and a hello
   with an Encrypted Client Hello or ESNI extension cut the tunnel before any byte of
   the client leaves.

5. **The address guard is an allow list.** An address passes only when no block of
   the IANA special-purpose registries holds it, and for IPv6 only inside `2000::/3`.
   efr unwraps the IPv4-mapped, IPv4-compatible and NAT64 forms first. The guard also
   refuses every address of the machine and every address on one of its links, read
   again at most once a second, so a router, a printer or a NAS on the LAN with a
   global address is refused. A name passes only when every address of it passes, and
   the proxy connects only to an address that it checked. A `private = true` entry may
   resolve to a LAN address, never to loopback, link-local or an address of the
   machine. A host grant never lifts the guard.

6. **Plain HTTP only for marked hosts, with a head that efr writes.** `GET` and `HEAD`
   in the absolute form, for an entry with `http = true`, an `http://` pacman mirror
   or a host grant on port 80. The proxy reads no byte after the head, and writes a new
   head from the parsed values and a fixed list of client headers. So no pipelined or
   smuggled request reaches the server.

7. **An upload budget for each call, from the first byte after the head.** The proxy
   counts every byte that the client sends after the `CONNECT` head, the ClientHello
   included, over every connection of the call (a job is one call for its whole
   life). It forwards at most the rest of the budget, then cuts, and refuses each new
   tunnel with 403. The default is 4 MiB (`network.max_upload_bytes`).

8. **A host that is not listed is a narrow grant, predicted before the run.** When the
   line or a git remote shows such a host, the engine gives a `host` exit with
   `Grant::Host` for that host and port, for that one call. The reviewer may allow it
   only when a message of the user names the host. Raw network programs (`nslookup`,
   `dig`, `ping`, `nc` and the like), `needs.hosts` for `localhost`, a private host or a
   port other than 443 and 80 keep the whole network after a review (R12): the proxy
   carries only HTTPS and plain HTTP, and a dev server that the user's browser must
   reach needs the host's namespace.

9. **The refusals are facts, never names.** The proxy records each refusal (at most 32
   records with a count, and the count of the rest). efrd puts them in the tool result,
   in `JobEnded` and in the record of the next exit (`refused_by_proxy`). The reviewer's
   prompt says that a refusal shows only that a command tried the host, never that the
   user wants it. There is no `NetworkBlocked` event.

10. **Credentials stay out of reach.** The masks hide the credential files, the
    environment filter drops `GIT_CONFIG_*`, names that end in `_AUTH` and values with
    a URL password, and a token in a file that the sandbox must read (the project's
    registry files, the git config with its global and include files) makes efrd deny
    that host for the call.

11. **The proxy runs only after its self-test.** `network.mode = "proxy"` turns it on.
    The probe runs a third launch with a proxy plan and checks the bridge (a `CONNECT`
    through `127.0.0.1:3128`), `EACCES` on a direct connect to the socket,
    `ENETUNREACH` for other TCP, and `EPERM` for a signal to the bridge. When a check
    fails, efrd falls back to no network, which is the behavior before phase D2, and
    says so. `network.mode = "none"` turns the proxy off by hand.

## Consequences

- efrd parses untrusted bytes. The caps, the hand parsers, the proptests and the
  forbid of `unsafe` are the guard; the bytes after the checks are never parsed.
- The proxy cannot see inside TLS. A command can send data to an allowed host with no
  credentials, up to the budget (R3). A CDN that serves another site by the `Host`
  header inside TLS (fronting) widens this; Fastly answered 421 and Cloudflare 403 in
  the probe.
- The proxy checks only the first ClientHello. A second hello after a
  HelloRetryRequest, a TLS 1.2 renegotiation and the `:authority` of HTTP/2 are not
  checked (R27).
- The Go module proxy fetches a missing module from its origin host, so a request
  through `@go` can carry data to another host (R24). A `*.suffix` entry lets a command
  pick a DNS name below the suffix, which the proxy resolves (R24).
- Tools that send an Encrypted Client Hello, also as a decoy (GREASE), such as
  headless Chrome, Electron and Playwright, cannot fetch through the proxy (R25).
- A socket grant that reaches the network goes around the proxy: systemd-resolved's
  varlink socket, nscd, the Docker and Podman sockets, tailscaled and a SOCKS socket.
  A socket exit already shows the path to the reviewer; a later phase may make such a
  socket a review for the whole network (OQP22).
- A foreground call past its timeout loses its network when efrd restarts (R26). A
  restart ends every job anyway.
- A token in a file that the sandbox must read, but that is not on efr's list, still
  reaches an allowed host (R28).
- Node's `fetch` needs `NODE_USE_ENV_PROXY=1`, so Node prints an "experimental"
  warning in sandboxed commands.
- The job stop spares a process named `efr-bridge` whose parent is the namespace's
  init. This is a listing rule, not a safety rule: the pid namespace ends the bridge
  with the job.
- A call with the proxy keeps its private network namespace, and the launcher refuses
  a proxy plan with `Grant::OpenNetwork`, so the bridge never listens on the host's
  loopback.
