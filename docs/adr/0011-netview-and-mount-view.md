# 0011: Read-only views of the host's network and mounts, with the kernel as the only guard

Status: accepted, 2026-10-13.

## Context

Phase D1 of the auto spec (section 8.8) adds two read-only views of the host to a
contained call. Before phase D1, a contained call had a private network namespace and
the sandbox's mount tree. So `ss -tln`, `ip -s link` and `cat /proc/net/dev` showed
the sandbox's own loopback, and `df -h` showed the sandbox's binds and masks. These
lines were exits (`host_view` with the full network, and `outside`), so plain system
work such as "what listens on port 22?" or "how full are my disks?" went to the
reviewer each time, and an allowed `host_view` gave the whole network to a line that
only needed to read.

These choices are expensive to reverse: one view puts a call that can run any code in
the user's network namespace, and the other makes efrd measure every mount of the
machine for a call.

## Decision

1. **The network view keeps the host's network namespace and refuses every network
   socket.** A call with `Grant::NetView` has no `--unshare-net`. Its seccomp profile
   is an allow list of socket families, `AF_UNIX` and `AF_NETLINK`; every other family
   fails at `socket()` with `EAFNOSUPPORT`, also each family that a later kernel adds.
   Netlink is limited to `NETLINK_ROUTE` and `NETLINK_SOCK_DIAG`, which `ip` and `ss`
   need; other protocols fail with `EPROTONOSUPPORT`. Every compare on a socket
   argument is a 32-bit compare, as the kernel reads them. A deny list of the inet
   families, as the spec first said, would leave every family that it does not name
   open.

2. **`listen` and `pidfd_getfd` fail with `EPERM`, and Landlock handles TCP.** The
   call shares the host's abstract socket namespace. The Landlock scope stops only
   connects and sends to the outside, so without the `listen` refusal a call could bind
   an abstract stream name and wait for a host program. `pidfd_getfd` could take a
   socket of another process; Landlock's ptrace rule and the private pid namespace stop
   that already, and this is a second layer. Landlock also handles TCP bind and connect
   with no rule, a second layer behind seccomp. Every kernel that efr supports has
   these Landlock rights (ABI 9 or later).

3. **seccomp, Landlock and the `listen` refusal are the only layers.** The line rule
   (`efr-permissions/src/exits/view.rs`) gives the view to a line of `ss`, `ip` read
   forms, `nstat` and readers of `/proc/net` with fixed text helpers, and nothing
   else. It is a fit for `ss`-like work, never a security layer: a NetView call can
   run any code, such as a function named `ss` from the sandbox state of an earlier
   call, a zsh module or a bug in a helper. The narrow list keeps lines that start
   local servers, and every interpreter, out of the host's namespace, because such a
   call loses the sandbox's private loopback. A test of efrd runs such a function and
   checks that its socket fails with `EAFNOSUPPORT`.

4. **A view is a reason, not an exit.** The engine gives the view as a reason of its
   own (`Subject::View`) with no record, no review and no question. The launch unions
   the view grants with the grants of the exits by an allow list: the view stays only
   beside `Write` and `Unmask`, which do not depend on the network namespace. Every
   other grant drops it: a network grant needs sockets that the view refuses, and a
   device such as `/dev/net/tun` can send frames with no `socket()` call. A later
   grant drops it too, unless a later decision says otherwise with a test.

5. **The probe checks each refusal, and the engine follows the probe.** A second
   self-test launch with the view runs at each probe. Each check passes only on its
   exact error code, and `tcp4` runs first: when a TCP socket opens, the profile is not
   in place, and no other `socket()` call reaches the kernel. A failure is a warning:
   the view is off (`SandboxStatus.netview` is false), and the lines go to the
   reviewer again. efrd builds its engine after the first probe, before it opens its
   socket, and again when a later probe changes the result. `network.netview = false`
   turns the view off by hand.

6. **The mount view is a snapshot from efrd, a renderer in the launcher and two zsh
   functions.** The probe of the mount view tried the other forms: a bind over
   `/proc/self/mountinfo`, `LIBMOUNT_MTAB`, a `df` pointed at a file and an
   `LD_PRELOAD` shim all failed or covered too little. efrd runs in the user's mount
   namespace, reads its own mount table, and measures the sizes outside the sandbox.
   It writes the table and the sizes into the call's dir, only for a line that runs
   `df` or `findmnt`. In the call, `efr-sbx mounts df` prints them in the format of GNU
   `df`, and the child shell's `df` and `findmnt` functions use them for the forms
   that they know; every other form runs the real program.

7. **One killable helper measures the mounts, and efrd never waits for it.** A
   `statvfs` of a hung NFS or FUSE mount can wait in the kernel, and only SIGKILL ends
   it; a thread of efrd in such a wait could never stop. So `efr-sbx statvfs` measures
   in its own process, with one thread per mount, and efrd kills it after one second
   and hands it to a reaper. At most one helper lives at a time. The dummy types, such
   as `autofs`, are never measured, so a `df` never mounts a share. Another user's
   mounts and the mounts at or below the call's masks get no sizes.

8. **efrd checks its mount namespace at start.** The mount view is true only when
   efrd's mount namespace is the one of its parent. A unit drop-in that gives efrd a
   namespace of its own (`PrivateTmp`, `ProtectHome` and the like) turns the view off,
   and `df` and `findmnt` stay `outside` exits, which show the true view after a
   review. A test fails when `systemd/efrd.service` gets such a key.

## Consequences

- A NetView call runs in the user's network namespace. A path that the profile misses
  would send from the host. The allow lists, the probe at every start and the escape
  tests on this machine and in the VM of CI are the guard.
- A host program can send a datagram to an abstract name that a NetView call binds,
  and the datagram can carry a connected socket (`SCM_RIGHTS`), which the call could
  then use with no `socket()` call. This needs a host program that sends to that name
  on purpose. `listen` fails, so the call offers no stream service, and a job ends at
  its time limit.
- The model sees the host's sockets, peers and routes. Before, the same output reached
  it after a review.
- A running NetView job keeps the view up to its time limit after the key goes off or a
  probe fails: its filter is in place, and the kernel does not change under it.
  `job_stop` ends it.
- Every contained call could already list every mount of the machine through
  `/proc/<pid>/mountinfo`. A new procfs or `hidepid` would hide that, and both hide
  `ps`. The mount view adds the sizes of the user's own mounts outside the masks.
- The mount view works only for `df` and `findmnt` that the shell itself runs by bare
  name on the line; a script shows the sandbox's mounts. A path or a wrapper, such as
  `/bin/df`, `command df`, `timeout 10 df` or `watch df`, runs the real program, so
  that line stays an `outside` exit. A job keeps the snapshot of its start.
- `nmcli` and `iwctl` use the system bus, not netlink, so they are `bus` exits; the
  user decides each form that changes something until a filter of the bus exists.
- The proxy of phase D2 must keep these rules: a call with the proxy keeps its private
  namespace, and the union rule stays an allow list.
