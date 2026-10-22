# The network proxy

<!-- sources: README.md "The auto mode" (network, system queries, network and disks bullets); docs/sandbox.md "The network proxy" (all subsections), "The host's network view", "The bus filter", "Exits"; docs/config.md [network], [dbus]; crates/efr-proxy/src/proxy.rs, guard.rs; crates/efr-permissions/src/network.rs; crates/efr-daemon/src/sandbox/proxy.rs; crates/efr-cli/src/format/sandbox.rs; docs/adr/0012-network-proxy.md -->

In the `auto` mode, a command in the sandbox has no direct network. With
`network.mode = "proxy"` (the default), it reaches the hosts of your allow list
through efr's network proxy, with no question. So `cargo build`, `npm install` and
`pip install` work. Every other route stays closed.

This page is about the `auto` sandbox only. In `manual` and `cautious`, a command
that uses the network asks you. In `full`, a command uses your real network.

## The default allow list

`network.allow` holds presets, host names and `*.suffix` entries. The default holds
six presets:

| Preset | Hosts |
|---|---|
| `@crates` | `crates.io`, `index.crates.io`, `static.crates.io` |
| `@npm` | `registry.npmjs.org`, `registry.yarnpkg.com` |
| `@pypi` | `pypi.org`, `files.pythonhosted.org` |
| `@go` | `proxy.golang.org`, `sum.golang.org` |
| `@github-read` | `github.com`, `codeload.github.com`, `objects.githubusercontent.com`, `raw.githubusercontent.com`, `release-assets.githubusercontent.com` |
| `@pacman-mirrors` | the hosts of the `Server =` lines of `/etc/pacman.d/mirrorlist` and the files that `/etc/pacman.conf` includes |

An entry of `allow` allows port 443.

## How a connection goes

The call has its own network namespace, with only loopback. efr sets its proxy
variables, such as `HTTPS_PROXY` and `CARGO_HTTP_PROXY`, to `http://127.0.0.1:3128`,
where a small bridge copies each connection to efrd's proxy. `NO_PROXY` holds
`localhost,127.0.0.1,::1`, so a server that a test starts in the call stays direct. A
tool that does not read the proxy variables fails closed: it cannot resolve the host.

## What the proxy checks

For each connection, the proxy checks that:

- the host is on the allow list or has a grant of the call, and no deny holds it;
- every address of the host is a global address: not loopback, private, link-local or
  an address of your machine or of its networks;
- the first bytes are a TLS hello, and its server name is the host;
- the call did not send more than `network.max_upload_bytes` (4 MiB by default) in all.

A host that is not on a list gets no DNS lookup, so its name never reaches a DNS
server. An IP address as the host is refused, unless `[[network.hosts]]` lists it.
Plain `http://` works only for an http host, and only for `GET` and `HEAD`.

## Add a host

To add a host on port 443, such as a company registry or a `GOPROXY` host, add it to
`network.allow`. Write a name in lower case and in its ASCII (`xn--`) form. Then set
the registry in the tool's own config as usual.

`[[network.hosts]]` adds a host with options, such as another port or a mirror on
your LAN. Only an editor changes it:

```toml
[[network.hosts]]
host = "mirror.lan"
port = 8080
http = true       # plain GET and HEAD too
private = true    # it may resolve to an address of your LAN
```

`network.deny` holds entries that no command reaches. It wins over the allow list and
over every grant.

A change of `allow`, `deny` or `[[network.hosts]]` reaches every running call at once.
A change of `network.mode` applies to the next call.

## A host that is not on the list

- A host that the line shows, such as the URL of `curl` or the host of an https git
  remote, is an exit for that one host and port. The reviewer may allow it only when a
  message of yours names the host. The next call asks again. The `a` answer can save
  a `host` rule, so the same host runs with no question later.
- A host that the line does not show, such as a host in a script, gets 403. The
  agent reads a note, such as `[efr: the network proxy refused other.test:443: not on
  the allow list. To reach a host, call shell again with needs.hosts.]`, and can then
  ask for the host.
- Programs that do not use HTTP get the whole network after a review: `nslookup`,
  `dig`, `ping`, `nc`, `socat`, `telnet` and others. So does a need for `localhost`, a
  private address, or a port other than 443 and 80. This is the route for a database
  on your machine, such as `psql`.
- `ssh`, `mosh` and git over ssh run outside the sandbox, with your ssh keys. The
  reviewer may allow them when a message of yours names the host, or when the remote
  belongs to your registered project.
- `git push` and other uploads run outside the sandbox. They ask you unless a message
  of yours names them.

A dev server that your browser must reach needs none of this. The agent starts it as
a [background job](jobs.md), and efr forwards its port to the same port of your
`127.0.0.1`.

## Refusals

A command that the proxy refused shows a dim line under its result, such as:

```text
network: blocked other.test:443 (not on the allow list)
```

| Reason | Why |
|---|---|
| `not on the allow list` | no list and no grant of the call names the host |
| `denied for this command` | the deny list holds the host, or a file can give the call a token for it |
| `a private address` | the name resolves to a local or private address |
| `the TLS name differs from the host` | the server name of the TLS hello is not the host |
| `not TLS` | no TLS hello with a server name |
| `ip address` | the command named an IP address |
| `over the upload limit` | the call sent more than `max_upload_bytes` |

[The auto sandbox](../sandbox.md#refusals) lists every reason.

## Credentials

The proxy lets a command reach an allowed host, so efr keeps credentials out of the
call:

- The read masks hide the credential files of git, the registries and the package
  tools.
- When a project file, such as `.npmrc` or `.cargo/config.toml`, or the git config
  holds a token for a host, efrd blocks that host for the call. The output says why,
  such as `[efr: efr blocks registry.npmjs.org for this command: ~/p/app/.npmrc holds a
  credential for it. Move the credential to a file that the sandbox hides, such as
  ~/.npmrc.]`.

## No network

With `network.mode = "none"`, a command in the sandbox has no network. Loopback still
works. Each network need then goes to the reviewer, and an allow opens the whole
network for one call.

`efr status` shows the network in its `sandbox` line, such as `ready (Landlock ABI 10,
bubblewrap 0.13.0, caches tmp, network proxy)`. `network none` means no network, and
`network none (proxy self-test failed)` means that the proxy is off because its check
at start failed.

## Your machine's network view

`ss`, the read forms of `ip`, `nstat` and reads of `/proc/net` run in the sandbox with
no question. They show your real sockets, links and counters. Such a command keeps
your network namespace, but every socket except Unix and netlink sockets fails, so it
cannot send anything. `efr status` shows `netview yes` when the view is safe on your
machine. `network.netview = false` sends these commands to the reviewer again.

## The bus filter

Read-only system queries run in the sandbox with no question: `hostnamectl`,
`timedatectl`, `localectl`, `systemctl --failed`, `systemctl status <unit>`,
`systemctl --user --failed`, `loginctl list-sessions` and `notify-send`. They reach
the system or the session bus only through efr's bus filter, `xdg-dbus-proxy`. The
filter lets through only read methods, so a script that calls a method such as
`StartUnit` fails at that call.

Install `xdg-dbus-proxy` 0.1.9 or newer for the filter. `efr status` shows `bus filter
on`. Without the program, or with `dbus.proxy = false`, these commands need the whole
bus: the reviewer decides for the system bus, and only you for the session bus.

A command that changes something, such as `systemctl restart nginx`, is never a read
form. It asks you, as `sudo` does.

## What the proxy cannot stop

- The proxy cannot see inside TLS. A command can send data to a host on your allow
  list, with no credentials, up to the upload limit.
- The Go module proxy fetches a missing module from its origin host, so a request
  through `@go` can carry data to another host.
- A `*.suffix` entry lets a command pick any name below the suffix.
- Tools that send an Encrypted Client Hello, such as headless Chrome, Electron and
  Playwright, cannot fetch through the proxy.

## Settings

| Key | Default | What it does |
|---|---|---|
| `network.mode` | `"proxy"` | `proxy` or `none` |
| `network.allow` | the six presets | hosts on port 443 |
| `network.deny` | `[]` | hosts that no command reaches |
| `network.hosts` | `[]` | hosts with a port, plain HTTP or a LAN address |
| `network.max_upload_bytes` | `4194304` | the bytes that a call may send |
| `network.netview` | `true` | the network view of `ss` and `ip` |
| `dbus.proxy` | `true` | the bus filter |
| `dbus.allow` | `[]` | more bus methods for the filter |

[config.toml reference](../config.md#network) lists every key.
[The auto sandbox](../sandbox.md#the-network-proxy) has the full details, and
[ADR 0012](https://github.com/eggfriedrice24/eggfriedrice.code/blob/main/docs/adr/0012-network-proxy.md) records the decision.
