<!-- sources: docs/config.md [remote]; docs/protocol.md "Remote devices"; docs/permissions.md (the phone rules); crates/efr-config/src/tables/remote.rs; crates/efr-cli/src/cli.rs (DeviceCommand, DeviceAddArgs); crates/efr-cli/src/commands/device.rs; crates/efr-cli/src/error.rs (the device errors and hints); crates/efr-daemon/src/pairing.rs; crates/efr-daemon/src/remote/listener.rs, tailnet.rs, admission.rs, prove.rs, live.rs; crates/efr-daemon/src/error.rs (RemoteOff, NotListening); crates/efr-transport/README.md "ws"; app/src/features (the screens and their texts); app/src/features/machines (the connection of each machine); app/src/features/conversation/model/outbox.ts (the outbox of the prompts); app/src/features/conversations/conversations-queries.ts (the watch and the poll of the lists); app/modules/efr-device-key/README.md; docs/adr/0022-remote-listener-and-device-keys.md -->

# The phone app

The efr phone app lets you follow your conversations on an iPhone, send a prompt and
answer the questions of the agent. The phone connects to efrd directly over your
Tailscale network. There is no relay and no server of a third party.

> The app is a preview. It is not in the App Store. You build it yourself from the
> `app/` directory of the efr repository, with an Apple Developer account and EAS
> Build. The
> [README of the app](https://github.com/eggfriedrice24/eggfriedrice.code/blob/main/app/README.md)
> tells how, with the steps of EAS and the dev, preview and production variants of the
> app, and what works in the app now. The app ran as a dev build on an iPhone with iOS 26:
> the pairing, the chat, the approvals, steer and queue, and the conversations sheet
> work.

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

On the phone, open the app. When no machine is paired, the app opens on the first step
of the pairing, which shows this command. Tap "Scan code" and point the camera at the
code. The app first checks that the code comes from this efrd. Then it makes the device
key and sends the public key with the code. iOS asks for the camera on the first scan.

The terminal then shows the facts of the phone and asks you:

```text
A phone wants to pair:
  name      Ana's iPhone
  tailscale ana-iphone (ana@example.com)
  key       SHA256:0ed3a6ab957ff6f5
Compare the key with the one that the app shows. Pair this phone? [y/N]
```

The app shows "Confirm on" with the name of the machine, and under it the name of the
phone and its key. Make sure that the key in the terminal is the same as the key that
the app shows. Then press `y`.
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

## Pair more machines

The app can pair with more than one computer, and it connects to all of them at the
same time. To pair another machine, open Settings and tap "Pair a new machine", or
pick "Pair a new machine" in the machine menu of the blank root. Then run
`efr device add` on that machine and scan its code. Each machine gives the phone its
own device key.

## Use the app

After the pairing, the app connects on its own.

### The blank root

The app opens on a blank conversation on the machine that you used last. The title in
the bar is the name of the machine. The status line of the composer shows `~` and the
mode of the new conversation. Type a prompt and tap Send: efrd starts a new
conversation on that machine, and the app opens it. The blank root stays under it.

The menu at the left of the bar changes the machine of the blank root. It shows each
paired machine, and the machines that are offline say so. The bar of each screen has
Settings and New (`+`) at the right. New goes back to the blank root.

### Find a conversation

- Tap the title of the bar. A sheet opens with the conversations of every machine:
  "New conversation" on top, then the ones that wait for an answer, today, this week
  and older. Pull it up to see more. The search field finds a conversation by its
  title, its machine or its directory, and then asks efrd.
- Tap a row to open the conversation. The sheet closes.
- Touch and hold a row of the sheet to copy its title or its id.

The app keeps at most 8 conversations on its stack. A conversation that is on the stack
opens where it is. Touch and hold the back button to see the stack.

### Follow a conversation

A conversation shows your prompts, the replies of the model, its reasoning, the tools
that it runs, the edits with their diff, and a line at the end of each turn. A turn
that runs now comes at once with the text that the model has sent so far, and the rest
streams in.

- Tap a reasoning row or a tool block to open it.
- Touch and hold a message to copy its text. A code block has its own copy button.
- The app asks before it opens a link. A web page then opens in the app. An image in
  a reply shows as a link, and HTML shows as text, so the app loads nothing that you
  did not tap.
- Near the end, the list follows the new text. When you are more than one screen from
  the end, "Jump to latest" shows above the composer. Tap it to go back to the end.
- At the top, the app gets the older turns from efrd. It keeps them on the phone, so
  they are there when you open the app again.
- A failed turn shows the reason, with "retry" and "copy error". Retry sends the prompt
  of the turn again.

### Send a prompt

The composer floats over the end of the transcript. Its status line shows the
directory and the mode of the conversation and how full the context of the model is,
such as `ctx 42%`. It shows "waiting for you" while a question of this conversation
waits, and "reconnecting" while the app connects again.

When no turn runs, Send sends your prompt. While a turn runs:

- Stop stops the turn.
- When you type, a capsule shows Steer or Queue. "Steer now" adds your text to the
  running turn. "Queue" sends it when the turn ends. Touch and hold the capsule to pick
  one. The app keeps your choice.
- A queued prompt shows "withdraw". Tap it to take the prompt back before it runs.

A prompt shows at once, with "sending", until efrd has it. When the connection stops,
or when iOS closes the app, the app sends the prompt again later. efrd runs it only
once. Send is off while the machine is offline. The app keeps the draft of each
conversation.

### Answer a question

A question of the agent shows the command, or the file and its diff, the directory,
and a line that tells why the permission mode asks. Answer with Allow or Deny. The
choices are the same as in the terminal, except "always allow".

The first answer wins. When you answer a question on the phone, the terminal shows
the answer, and the reverse.

A question shows how long it can wait. When nobody answers in time, it shows
"expired", or "expired while you were away". The call did not run in both cases.

When another conversation waits for an answer, a strip under the bar tells you. Tap it
to open that conversation at the question. When two or more wait, the strip opens the
sheet with them on top. A yolk dot on the title of the bar tells you too. efrd sends
each change of its list of conversations to the app, so the strip and the dot show a
new question at once. With an older efrd, the app asks every 10 seconds, so they can
come up to 10 seconds late.

### When a machine is offline

A line under the bar tells you when the machine of the conversation is offline, or
when Tailscale on the phone is off. The composer then says so too. When the app comes
back to the screen after 10 seconds or more, or when the phone changes its network,
the app connects again at once and gets what it missed.

### Settings

Settings shows each paired machine, with its connection and the version of its efrd.
The menu of a row copies the node name, pairs the machine again, or forgets it.
Settings also has the Haptics switch, which turns off all the haptics of the app, and
the version of the app.

Forget deletes the pairing key of that machine on this iPhone, and its conversations
leave the app. Nothing changes on the machine: to take the access of the phone away,
also revoke it there (see [Revoke a device](#revoke-a-device)). After you forget the
last machine, the app goes back to the first step of the pairing.

## What the phone cannot do

A phone has fewer rights than your terminal. efrd checks each of these rules itself,
so a changed app cannot get past them:

- A turn from the phone runs with at most `cautious`. `auto` and `full` count as
  `cautious`, and efr asks for everything outside `$SCRATCH`. See
  [Permission modes](modes.md).
- Only your own computer answers some questions: a command that runs as another user
  (`sudo`, `doas`, `su`, `pkexec`, `run0`), a background job, and a delete that full
  mode guards. The app shows "answer on" and the name of the machine, with the reason,
  and no buttons. The turn waits for your answer there.
- The phone cannot choose "always allow". An always-allow rule changes your config.
  When a question offers it, the app shows Allow and Deny, and tells that the rule is
  set in the terminal.
- The phone cannot type an input for a command, such as a password. The app shows
  that the command can ask for input.
- The phone cannot undo a turn and cannot change settings.
- The phone never gets the `admin` scope, so it cannot pair or revoke a device.

Limits of this version:

- iPhone only. No Android, no iPad layout.
- No push notifications. A question waits until you open the app.
- No attachments, such as images.
- No terminal view of the hidden shell.
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
