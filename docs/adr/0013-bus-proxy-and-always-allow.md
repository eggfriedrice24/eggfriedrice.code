# 0013: A filtered bus for each call, and always-allow rules

Status: accepted, 2026-10-19.

## Context

Before phase E, a contained call of `auto` had no D-Bus. `hostnamectl`,
`systemctl --failed`, `systemctl --user --failed` and `loginctl list-sessions` were
`bus` exits. An allowed `bus` exit gave the call the whole system or session bus, on
which one method call can start a unit, power off the machine or run code outside the
sandbox (`StartTransientUnit`). So the reviewer judged each read-only system query, and
an allow opened far more than the query needed.

The auto spec (sections 11.1 to 11.3) asks for two things. First, a filter that lets a
contained call make the read calls of these tools, with no review. Second, a standing
answer: the user can say "always" for a narrow exit, so the same exit asks nothing
again.

These choices are expensive to reverse. The filter is a new process outside the
sandbox that parses bytes from it. A standing rule stays open for every later call,
also under prompt injection.

## Decision

1. **`xdg-dbus-proxy`, not an own proxy.** efr runs the `xdg-dbus-proxy` of the
   distribution, the filter that Flatpak runs for each app. efrd takes it only as a
   trusted program: root owns its dir and its file, and efrd checks its inode before
   each run. An own proxy in Rust would need a D-Bus parser in efrd, which runs with
   the user's full rights, or a new crate that `deny.toml` does not hold. No crate was
   added.

2. **One proxy for each filtered bus of a call.** efrd starts a proxy only for a call
   whose launch holds the view `Grant::BusFilter { bus }`, before the launcher starts,
   and ends it after the launcher's result. A call needs at most two proxies, and most
   calls need none. A background job keeps its proxies for its whole life, with the
   rules of its start. A proxy per conversation would need its own idle stop, a restart
   on a rule change that a running job's bind cannot follow, and one log for many
   calls. The cost: about 1.5 ms to the ready byte and 8 MB for each running proxy,
   paid only by calls that read the bus.

3. **The proxy runs under a wrapper in the launcher.** efrd runs
   `efr-sbx dbus-proxy`, the launcher copy that it already checks. The wrapper applies
   its own Landlock domain (read and run only the program and its libraries, connect
   only to the two real bus sockets, make sockets only in the call's `bus/` dir, no
   TCP, the signal and abstract socket scopes), a seccomp filter (only `AF_UNIX`
   sockets, no `ptrace` and no `pidfd_getfd`; `execve` stays, because the wrapper sets
   the filter before it runs the program, and Landlock lets it run only the program
   and the library dirs), an fd limit of 64 and `no_new_privs`, then runs the program. It moves its stdout to fd 3 with the
   existing `fds.rs` code, so no new `unsafe` came. The ready byte on fd 3 within 2 s
   means ready; no byte is a start failure, and the call fails. It never gets the whole
   bus instead.

4. **Exact rules only.** The defaults name each method by its full name and path: the
   read methods that the probe recorded for these tools on systemd 262 (and the
   recording of the CI image adds the members of systemd 255). `xdg-dbus-proxy` has no
   member globs, and `IFACE.*`, a path-only rule or `--talk` would allow every method,
   so efr never writes them. `GetUnitProcesses` and `GetUnitByPID` stay out: they show
   or map the processes of the host, which the pid namespace hides. On the session bus
   no Properties rule exists at `/org/freedesktop/systemd1`, so the user manager's
   `Environment` stays out.

5. **A view, by a line rule.** The engine gives `Grant::BusFilter { bus }` to a line
   in which at least one simple command is a read form of `systemctl`,
   `systemctl --user`, `loginctl`, `hostnamectl`, `timedatectl` or `localectl` with no
   option that names another host, machine, root or image. The rule picks the lines
   that fit; it is not a security layer: the other commands of the line get the same
   filtered bus. Every other bus form keeps its `bus` exit for the reviewer or the
   user, and a `bus` exit still gives the whole bus for one call.

6. **`[[dbus.allow]]` has a floor.** A user entry adds methods to the defaults, never
   replaces them. For the default services an entry may add only a read member
   (`Get*`, `List*`, `Can*`, `Lookup*`, `Properties.Get` and `GetAll`). For other
   services efr refuses wildcards, path-only rules, the bus driver, `Properties.Set`,
   secrets, portals, polkit, Flatpak, kwallet, dconf and a member that starts with a
   known write or power word. For those services efr cannot tell a read from a write,
   and the docs say that the user owns each method that they add.

7. **Always-allow rules are narrow, and sit below every floor.** An `a` answer, then a
   `y` at a confirm line, saves one rule in `[[sandbox.allow]]` of `config.toml`: a
   `host` (one host and port through the proxy), a `write` (one dir, below which the
   bind of the yes lies) or a `socket` (one socket, never a link). The order of the
   decision is: the floors, your `deny` and `ask` rules, the always rules, the
   reviewer, you. A rule covers a call only when it covers every exit of the call; the
   exit's approver is the reviewer; it runs in the sandbox; its grants are narrow; each
   path matches by the real path that efrd found; no floor, synced dir or project lies
   at or below a dir grant (also a floor that does not exist yet); and no `deny` or
   `ask` rule of the user names a path at or below a grant. So a rule never covers a
   privilege, persistence, upload, desktop, masked-read, bus or device exit, or a file
   tool.

8. **Only the user's own terminal saves a rule, through efrd.** efrd accepts `a` only
   from a local origin with the `approve` scope; a phone gets `forbidden`, and the
   question stays open. The conversation asks efrd to save the rule; efrd runs the
   same floor check as the load, writes the file with its format-preserving writer
   (the comments stay), and waits for the reload, so the next call sees the rule. A
   rule that cannot be saved does not stop the call: it runs once as a yes, and the
   user sees `rule not saved: <reason>`.

9. **A bad rule is disabled, not fatal.** A rule can fail a floor check later with no
   edit: a new project, a new dotfile link. So efrd keeps such a rule in the list,
   marked disabled with its reason, warns once, and starts. Only a rule of a bad shape
   stops efrd at start or refuses a reload.

10. **The filter is on by default after its tests.** `dbus.proxy` was `false` while
    phase E was built, and is `true` from its last commit. efrd turns the filter off
    when the program is missing, older than 0.1.5, or fails its self-test, and says
    why. Every bus form then keeps its `bus` exit, which is stricter, never looser.

## Consequences

- The proxy keeps an unfiltered connection to the real bus for each client. A memory
  bug in its parser reaches the user's full rights on that bus, and on the session bus
  that includes `StartTransientUnit`, which runs code outside the sandbox. The wrapper
  limits files, network, system calls and fds; it does not limit what the proxy may
  send on its upstream connection.
- The filter checks no arguments. `Properties.Get` and `GetAll` on an allowed path
  read every property there. A session unit's properties can show the environment of a
  transient unit (`systemd-run --user -E`), which the sandbox hides.
- A client can send broadcast signals with no rule. The sender is its unique name, so
  it cannot pose as a well-known name, and a listener that checks the sender is safe.
- The bus and polkit see the proxy, which runs in efrd's cgroup and session, not the
  user's seat session. The defaults hold no polkit-guarded method; a write still goes
  through a `bus` exit of the whole bus.
- A `--call` rule gives its name the TALK level, so the bus can start a listed service
  by activation. `~/.local/share/dbus-1` is a floor, so a contained call cannot plant a
  service file there.
- `systemctl --user` reaches the filter only from the line, through a function of the
  child shell that unsets `XDG_RUNTIME_DIR` for it. A script that runs
  `systemctl --user` itself fails, because systemd then looks only for its private
  socket.
- A running job keeps the bus rules of its start: removing a `[[dbus.allow]]` entry
  or `dbus.proxy = false` reaches only new calls and jobs.
- A standing host or write rule stays open for every later call, also when a message
  or a file tries to steer the model. The kinds are narrow, the floors hold, and
  `efr sandbox allow remove N` takes a rule out. Any program of the user that can
  write `config.toml` can add a rule; the reload applies it.
- A one-time yes for a write to a dir above a floor that does not exist yet, such as
  `~/.config` with no `~/.config/autostart`, can make that floor. Always rules never
  cover such a dir; the one-time case waits for a later change of the plan builder.
- The tests need `xdg-dbus-proxy`, `dbus-daemon` and Python's Gio on the test machine
  and in CI. They run private buses with a fake service and never reach a real bus.
