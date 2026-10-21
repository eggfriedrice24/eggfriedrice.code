# 0016: Port forwards for contained background jobs

Status: accepted, 2026-10-21.

## Context

A contained call of `auto` has a network namespace of its own, with only `lo`. A
background job that runs a dev server, such as `python3 -m http.server 8765`, listens
on that private loopback. The live test of phase D2 (step 7) showed the result: the job
started with no question, and the user's browser and `curl http://127.0.0.1:8765/` got
"connection refused". The only route was a `host` exit of `localhost` that gave the
job the whole network after a review. That route opens far more than the one port,
and the reviewer allows it only when a message of the user names the host.

The user decided that efr forwards each loopback port of a contained job to the host
and shows one line, with no question. These choices are expensive to reverse: a helper
runs inside the job's sandbox, efrd decodes bytes from the sandbox and listens on the
user's loopback for the job, and a host process that connects reaches code that the
model controls.

## Decision

1. **Which work gets forwards.** Only a contained background job in `auto` with a
   network namespace of its own: `network.mode = "none"` or the proxy, with no grant
   of the whole network and no network view. A job in the exit child and a job with
   the whole network already listen on the host's loopback. A network view job cannot
   listen. A foreground call gets no forward: a forwarder for each call would cost
   every call a process and a socket, and a server that the user must open runs as a
   job. `jobs.forward_ports` (default `true`) turns it off; a job reads it when it
   starts.

2. **A confined helper in the job finds the listens.** The inner stage starts
   `efr-sbx forward` as it starts the bridge of ADR 0012: two `Command` spawns, a
   starter and a server, in parallel with the layer helper. The server closes its
   descriptors, makes its own session, names itself `efr-forward`, opens the job's
   `/proc/self/net/tcp` and `tcp6` and reads the ephemeral range, turns off dumps, and
   applies its own Landlock domain (no file right, `RESOLVE_UNIX` only on the forward
   socket, the TCP bind right handled with no rule, the signal and abstract socket
   scopes) and its own seccomp profile (only `AF_UNIX`, `AF_INET` and `AF_INET6`
   sockets; `bind`, `listen`, `execve`, `execveat`, `ptrace` and `pidfd_getfd`
   refused). Only then does it write its ready byte. It reads the two tables every 500
   ms, and a port counts after it shows in polls 1 second apart (10 seconds in the
   ephemeral range). The parse and the debounce are pure functions in `efr-sandbox`,
   with property tests. Unlike the bridge, a forwarder that does not start fails
   nothing: the job runs with no forwards, and the helper never serves unconfined.

3. **One Unix socket for each job, and fixed frames.** efrd binds `$CALL/forward.sock`
   (0600) before the job starts, through `/proc/self/fd` as the proxy socket. The plan
   binds it read-only at `$XRT/efr-forward.sock`, and the child's domain has no
   `RESOLVE_UNIX` rule for it. The forwarder's first connection sends `Hello` and then
   `Listen` and `Gone` frames. For each host connection, efrd sends `Open` on that
   connection, and the forwarder connects inside the job and opens a new connection to
   the socket with `Data`. Then both sides copy opaque bytes. A frame has 28 bytes, and
   efrd decodes it with a hand decoder that refuses an unknown kind, a reserved byte
   that is not 0, a family other than 4 and 6, an address that is not loopback and a
   port below 1024. efrd takes only the first `Hello` of a job.

4. **efrd checks every report again.** The forwarder runs where the job may control
   it. So efrd refuses again a port below 1024, port 3128 of a proxy job (the bridge's
   port, which would give host processes the job's proxy grants), and a built-in list
   of service ports where a client sends a secret first (`efr login`'s 1455, the ports
   of databases, caches, brokers, container, cluster and secret services, a local
   model server and the proxy ports). A job forwards at most 8 ports and efrd at most
   32.

5. **The host side binds the same port of `127.0.0.1` only.** Never `0.0.0.0`, never
   `::1`, never another port, and no `SO_REUSEPORT`. A port is in use when the bind
   fails, when the host's `tcp6` table has a `LISTEN` row on `::1` or `::` of the port
   (a browser tries `::1` first for `localhost`, so the URL would open that server),
   or when another job holds it. A port that the real service holds is never taken.

6. **Only the user's own processes connect.** On each accept, efrd reads the host's
   `/proc/net/tcp`, finds the peer's row (local port the peer's port, remote port the
   forwarded port, in that order, so it never takes the row of its own accepted
   socket) and compares the uid with its own. Another uid, or no row, closes the
   connection at once.

7. **No question, no review, no exit.** A forward opens only an inbound path to the
   job. The job gets no new right on the machine and no new route out: efrd connects
   only to the job's socket, and the forwarder connects only inside the job's
   namespace. The risks come from what host clients send to the job, and a question
   at each listen would not tell the user which client will connect. One line in the
   view and one `job_port_forwarded` event are the notice.

8. **The port stays the job's until the job ends.** A `Gone` keeps the port, so a
   watcher that restarts its server shows one line and the port never changes hands.
   The listeners close when the job ends, when efrd stops, and at a reload that sets
   `jobs.forward_ports = false`. Nothing lives on after a restart: the listeners are
   efrd's own sockets.

Rejected:

- efrd reads `/proc/<pid>/net/tcp` of a process in the job: efrd would need a pid in
  the namespace, and the connect into the namespace needs a process there anyway.
- seccomp user notification on `listen`: every `listen` of the job would wait for a
  supervisor, a dead supervisor breaks the job, and it is more code in a security
  boundary.
- A helper outside that joins the job's user and network namespaces: it would run with
  every capability of the job's user namespace until it confines itself. The bridge's
  pattern (start inside, confine, then serve) is proven and smaller.
- One socket for each port: more binds and more Landlock rules for the same trust.
- Passing the connected TCP descriptor to efrd with `SCM_RIGHTS`: efrd would hold
  descriptors that the sandbox chose, and it has no code for ancillary data.
- A remap to another host port, and a listen on `::1` of the host: the URL that the
  dev server and the model print would then be wrong or ambiguous.
- Forwards for foreground calls: a process and a socket for every call (decision 1).
- A check of the `Host` header: efr copies opaque bytes and parses no HTTP in efrd;
  dev servers such as Vite check the header themselves.
- A question before each forward: decision 7.

## Consequences

- A model that a prompt injection steers can listen on the port of a service that
  does not run at that time, and receive what a client of the user sends first. The
  list of service ports, the in-use rule, the line and the job's time limit bound it;
  `docs/sandbox.md` names it as a known limit.
- A page that a forwarded server serves runs in the user's browser, outside the
  sandbox, with the browser's network. The browser sends the cookies of `localhost`
  to the job, and DNS rebinding lets a web site read the server. These are the same as
  for any dev server, and the docs name them.
- A server of the user on `::1` or `::` only makes the port count as in use.
- A port in the ephemeral range gets its forward after 10 seconds.
- efrd parses 28-byte frames from the sandbox with a hand decoder and property tests,
  and copies every other byte opaque.
- Each contained job costs one more process (two spawns, in parallel with the overlay
  mounts), one socket in efrd and two reads of small `/proc` files every 500 ms. In
  the cost test of `efr-sbx`, the forwarder adds about 2 ms at the median to the start
  of a job (a debug build).
- The job stop spares a process named `efr-forward` whose parent is the namespace's
  init, as it spares `efr-bridge`. This is a listing rule, not a safety rule: the pid
  namespace ends the forwarder with the job.
- UDP is not forwarded.
