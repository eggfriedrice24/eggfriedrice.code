<!-- sources: docs/config.md [remote]; docs/protocol.md "Remote devices"; docs/permissions.md (the phone rules); crates/efr-config/src/tables/remote.rs; crates/efr-cli/src/cli.rs (DeviceCommand, DeviceAddArgs); crates/efr-cli/src/commands/device.rs; crates/efr-cli/src/error.rs (the device errors and hints); crates/efr-daemon/src/pairing.rs; crates/efr-daemon/src/remote/listener.rs, tailnet.rs, admission.rs, prove.rs, live.rs; crates/efr-daemon/src/error.rs (RemoteOff, NotListening); crates/efr-transport/README.md "ws"; app/src/ui (the screens and their texts); app/src/session (the session of the screens); app/modules/efr-device-key/README.md; docs/adr/0022-remote-listener-and-device-keys.md -->

# The phone app

The efr phone app lets you follow your conversations on an iPhone, send a prompt and
answer the questions of the agent. The phone connects to efrd directly over your
Tailscale network. There is no relay and no server of a third party.

> The app is a preview. It is not in the App Store. You build it yourself from the
> `app/` directory of the efr repository, with an Apple Developer account and EAS
> Build. The
> [README of the app](https://github.com/eggfriedrice24/eggfriedrice.code/blob/main/app/README.md)
> tells how, and what works in the app now. The parts of efrd work: the listener, the
> pairing, the device keys and the revoke. The app was tested on Linux only. Nobody
> built it for an iPhone and ran it there yet, so the first build can show problems.

## What you need

- Tailscale on the computer that runs efrd: tailscaled 1.100 or later, logged in and
  connected. tailscaled must use a TUN device. efrd does not listen when tailscaled
  runs with `--tun=userspace-networking`.
- Tailscale on the iPhone, logged in as the same Tailscale user. efrd refuses a phone
  of another user, a phone that another tailnet shares with you, and a tagged device.
- A computer that is not a tagged device. A tagged computer belongs to no user, so
  efrd refuses every phone.
- A tailnet policy that lets the phone reach the computer on the port of efrd (7419
  by default). The default policy of Tailscale lets each device of a user reach the
  other devices of that user.

efrd only reads from tailscaled: the status of this machine, the identity of each
peer, and the changes of the network. It does not change your Tailscale settings.

## Turn on the listener

The listener is off by default. Turn it on and restart efrd:

```sh
efr config set remote.enabled true
systemctl --user restart efrd
```

efrd then listens on the Tailscale IPv4 address of this machine (an address in
`100.64.0.0/10`), and on no other address. It does not listen on your LAN, on
loopback or on the internet. The log of efrd shows the address:

```sh
journalctl --user -u efrd | grep 'remote listener'
```

`daemon.json` in the runtime directory also holds it, as `tailnet_endpoint`.

The `[remote]` table of `config.toml` has three keys. Each one needs a restart of
efrd:

| Key | Default | What it does |
|---|---|---|
| `enabled` | `false` | `true` turns on the listener. |
| `port` | `7419` | The TCP port on the Tailscale address, from 1024 to 65535. |
| `tailscaled_socket` | `/var/run/tailscale/tailscaled.sock` | The LocalAPI socket of tailscaled. Change it only when your tailscaled uses another path. |

[config.toml reference](../config.md#remote) has the details.

When Tailscale stops, efrd closes the phone connections and waits. When Tailscale
comes back, or the address or the tailnet of this machine changes, efrd listens again
on the new address. You do not restart efrd for this.

The agent cannot turn on the listener for you. The settings tool counts a change of
`remote.enabled` or `remote.tailscaled_socket` as a change that loosens your
permissions, so it always asks you first. A turn from the phone cannot change
settings.

To turn off the listener, set `remote.enabled` to `false` and restart efrd.

## Pair a phone

Pair each phone once. Run this in a terminal on the computer:

```sh
efr device add
```

efr shows a QR code below the command line:

```text
Scan this code with the efr app on your phone:

  (the code)

The code is good for one phone until 2026-10-26 14:05:00 CET (in 5m 0s). Ctrl+C stops the pairing.
```

On the phone, open the app, tap "Pair this phone", then "Scan the code", and point the
camera at the code. The app first checks that the code comes from this efrd. Then it
makes the device key and sends the public key with the code.

The terminal then shows the facts of the phone and asks you:

```text
A phone wants to pair:
  name      Ana's iPhone
  tailscale ana-iphone (ana@example.com)
  key       SHA256:0ed3a6ab957ff6f5
Compare the key with the one that the app shows. Pair this phone? [y/N]
```

Make sure that the key is the same as the key that the app shows. Then press `y`.
efr prints:

```text
paired the device 0193f2a1; efr device revoke 0193f2a1 takes its access away
```

Any other key refuses the phone, and efr prints `nothing was paired`.

The rules of a pairing:

- The code holds a one-time token. It is good for one phone and for 5 minutes.
  efrd keeps only a hash of it, and only in memory. A restart of efrd ends every
  pairing.
- Nothing is paired until you press `y` in the terminal.
- After too many wrong tokens, efrd ends the open pairings.
- Ctrl+C ends the pairing. The token is then good for nothing.
- `efr device add` needs a terminal for its question. In a pipe it stops before it
  opens a pairing.

Options:

- `--name NAME` gives the device a name of your own in `efr device list`. Without it,
  efr uses the name that the phone sends.
- `--invert` swaps the dark and the light parts of the code, for a terminal with a
  light background.

When the window is narrower than the code, efr says how many columns it needs. Make
the window wider.

## The device key

The app makes a P-256 key in the Secure Enclave of the iPhone. The private key never
leaves the phone, and it is not in a backup. efrd keeps only the public key.

On each connection, efrd sends a new challenge, and the phone signs it with its key.
The signature also holds the Tailscale node of the phone and the tailnet of efrd. So a
signature that worked once does not work again, and it does not work from another
device. Until the phone proves its key, efrd answers only the calls of the pairing and
of the proof.

The key needs no Face ID. The app proves the key each time that it connects again.

## Use the app

> The app was not run on an iPhone yet. These are the features of its code; the first
> EAS build must show that they work on a phone.

After the pairing, the app connects on its own. You can:

- see your conversations,
- follow a conversation live, also a turn that runs now: the text that the model has
  sent so far comes at once, and the rest streams in,
- send a prompt to a conversation; a prompt that you send while a turn runs waits
  behind it,
- answer the questions of the agent, with the same choices as in the terminal, except
  "always allow".

The first answer wins. When you answer a question on the phone, the terminal shows
the answer, and the reverse.

A question shows how long it can wait. When nobody answers in time, the app shows
"The time to answer is over", or "Expired while you were away". The call did not run
in both cases.

When the app comes back to the screen after 10 seconds or more, it connects again at
once and gets what it missed.

## What the phone cannot do

A phone has fewer rights than your terminal. efrd checks each of these rules itself,
so a changed app cannot get past them:

- A turn from the phone runs with at most `cautious`. `auto` and `full` count as
  `cautious`, and efr asks for everything outside `$SCRATCH`. See
  [Permission modes](modes.md).
- Only your own computer answers some questions: a command that runs as another user
  (`sudo`, `doas`, `su`, `pkexec`, `run0`), a background job, and a delete that full
  mode guards. The app shows "Answer on your computer", and the turn waits for your
  answer there.
- The phone cannot choose "always allow". An always-allow rule changes your config.
- The phone cannot type an input for a command, such as a password. The app shows
  that the command can ask for input.
- The phone cannot undo a turn and cannot change settings.
- The phone never gets the `admin` scope, so it cannot pair or revoke a device.

Limits of this version:

- iPhone only. No Android, no iPad layout.
- No push notifications. A question waits until you open the app.
- No attachments, such as images.
- No terminal view of the hidden shell.
- The app starts no new conversation. It sends prompts to the conversations that
  exist.
- A dark theme only.
- The connection is `ws://` inside Tailscale. WireGuard encrypts it, and efrd adds no
  TLS of its own.

## Revoke a device

List the paired devices:

```sh
efr device list
```

Each row shows the start of the device id, its name, its Tailscale node, when it was
paired and when it was last seen. The list also shows the revoked devices.
`--json` prints the answer of efrd, for scripts.

Revoke a device when you lose the phone, or when you do not use it any more:

```sh
efr device revoke 0193f2a1
```

The id can be the start of an id, as the list shows it: at least 4 characters, and
only one device may have that start. The open connections of the device close at once. A revoke cannot be undone.
To use the phone again, pair it again with `efr device add`.

## How efrd keeps the listener safe

- efrd listens only on the Tailscale IPv4 address, never on all interfaces.
- For each connection, efrd asks tailscaled who the peer is before it reads a byte.
  When tailscaled cannot tell, efrd closes the connection.
- efrd refuses a connection from this computer itself. So a local process, such as a
  command of the agent, cannot use the listener.
- efrd refuses a connection from an address that is not a Tailscale address, such as a
  device on your home network behind a subnet route.
- efrd refuses tagged devices, shared devices, and devices of other users or other
  tailnets. A paired device must connect from the Tailscale node, the user and the
  tailnet that it paired from.
- efrd refuses a web page. A request with an `Origin` header fails, unless the header
  is the address of the listener: `http://`, the Tailscale IPv4 address of this
  machine, and the port. So a browser cannot reach efrd through a page.
- The phone proves its device key on each connection.
- A revoke closes the device's connections at once, and efrd drops what it had not
  sent yet.
- efrd keeps at most 16 phone connections at one time.

If you install Tailscale again on the phone, the phone becomes a new Tailscale node.
efrd then refuses it, and the app shows "Pair this phone again". Pair it again with
`efr device add`, and revoke the old entry.

[Troubleshooting](../help/troubleshooting.md#the-phone) tells what to do when the
phone does not connect.
