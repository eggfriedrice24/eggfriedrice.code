# 0022: A tailnet listener for the phone, with paired device keys

Status: accepted, 2026-10-26.

## Context

The user wants to follow conversations and answer approvals from an iPhone. efrd runs
commands with the user's rights, so any network listener of efrd is a large target.
Until now efrd had only a Unix socket of mode 0600, and the kernel told it the uid
and the pid of each peer.

The research compared t3code, OpenCode, Happy, the Codex app-server and Claude Code
Remote Control. Most of them use a hosted relay, or a plain listener with a shared
password or a bearer token. None of them asks tailscaled who a peer is, and none of
them keeps a hardware key on the phone. OpenCode had three advisories, all from a
browser that reached its server.

The user already runs Tailscale on the computer and on the phone. WireGuard gives an
encrypted tunnel, NAT traversal and a peer identity that tailscaled can tell. The user
read the research and accepted each of its recommendations. These choices are
expensive to reverse: the phone app, the pairing record in the database and the
signed message all depend on them.

## Decision

1. **A direct WebSocket on the tailnet address, with no relay.** With `[remote]
   enabled` (default `false`), efrd listens on the Tailscale IPv4 address of the
   machine and on no other address: not `0.0.0.0`, not loopback, not the LAN. It
   reads the address from tailscaled's LocalAPI status and binds only when the
   backend state is `Running` and tailscaled has a TUN device. In userspace
   networking mode, inbound tailnet TCP comes from `127.0.0.1`, so efrd refuses that
   mode. It needs tailscaled 1.100 or later (`InitialStatus` on the IPN bus). It
   watches the IPN bus and binds again when the address or the tailnet changes. The
   frames are those of the Unix socket, one JSON frame per text message. The
   WebSocket is fastwebsockets 0.10.0 in server role, with a handshake that efr writes
   itself, with no extension and no subprotocol. efrd refuses binary and fragmented
   messages, and checks the mask bit of each client frame itself, because
   fastwebsockets does not. A client message is at most 1 MiB.

2. **Plain `ws://` inside WireGuard, no TLS in v1.** WireGuard encrypts the link. A
   `wss://` endpoint needs the HTTPS feature of the tailnet, which puts the name of
   the machine in the public certificate logs, and operator rights on tailscaled. iOS
   App Transport Security does not apply to the WebSocket of React Native
   (SocketRocket opens a CFNetwork stream), and the app keeps
   `NSAllowsLocalNetworking` for the rest.

3. **Admission by whois before any byte, fail closed.** Right after `accept()`, efrd
   asks tailscaled's whois for the peer address. It refuses the connection, with no
   HTTP answer, when the whois fails or knows no node, and for:
   - this machine itself, by its node id or any of its Tailscale addresses, because
     every local process that dials the address looks like this node (an agent's
     command among them);
   - a tagged node, because its user is a synthetic one;
   - a node that another tailnet shares;
   - a node of another user, and every node when this machine is tagged.

   The pairing pins the `Node.StableID` (a string; the numeric `NodeID` can pass
   2^53), the `UserProfile.ID` and the `CurrentTailnet.StableID` of efrd. Each later
   connection must match all three. A changed node id after a valid signature gives
   a "re-pair needed" error, not a plain refusal.

4. **An Origin rule against web pages.** The handshake accepts a missing `Origin`, or
   exactly the origin of the listener: `http://<tailnet IPv4>:<port>`. It refuses
   every other value, `null` and the MagicDNS name too. SocketRocket always sends the
   origin of the URL, and the app dials only the IPv4 address, so the app passes.
   efrd serves no page, so no web page has that origin. Every other request gets a
   plain HTTP error, never a 200, and efrd never reads a `Cookie` or a credential in
   the URL.

5. **A device key in the Secure Enclave, proved on each connection.** The app makes a
   P-256 key in the Secure Enclave (an Expo module of its own in `app/modules`), with
   `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` and no Face ID gate, because the app
   proves the key at each reconnect. The Secure Enclave cannot hold Ed25519 keys.
   efrd keeps the 65-byte public point. The `hello` result of a remote connection
   holds a fresh challenge and the facts to sign. The phone signs seven fields, each
   with a 4-byte length: the domain tag `efr-device-prove-v1`, the challenge, the
   daemon id, the protocol version, the device id, the phone's node id and efrd's
   tailnet id. efrd checks the DER signature with aws-lc-rs
   (`ECDSA_P256_SHA256_ASN1`), which is already in the build. Before the proof, only
   `hello`, `device.enroll` and `device.prove` run. Three failed proofs close the
   connection.

6. **Pairing from the terminal, with a one-time token.** `efr device add` runs only
   over the Unix socket. It shows a QR code inline below the command line (efr is not
   a TUI) with an `efr://pair` URL: the daemon id, the tailnet address and port, the
   MagicDNS name (for display only) and a token of 144 random bits. efrd keeps only the token's SHA-256,
   in memory. The token is good for one phone and for 5 minutes, and too many wrong
   tokens end the open pairings. The app checks the daemon id and the protocol before
   it sends the token. The terminal shows the phone's name, its Tailscale node and
   user and the fingerprint of its key, and only the user's yes writes the device
   row. There is no typed code in v1.

7. **The scopes of a phone come from its row, and some answers stay local.** A phone
   gets `read`, `operate` and `approve`, never `admin` or `terminal`. The `/proc` walk
   of the Unix socket does not apply. A turn from the phone runs with at most
   `cautious`. efrd refuses from a phone: an answer to a `local_only` question (a
   command that runs as another user, a background job, a delete that full mode
   guards), every "always allow" answer, `input.respond`, `turn.undo` and a change of
   settings. The settings tool counts `remote.enabled` and `remote.tailscaled_socket`
   as changes that loosen permissions, so the agent cannot open the listener without
   the user.

8. **A revoke closes the sockets at once.** efrd subscribes to the revoke feed of the
   store when it starts, and `device.prove` adds its connection to the live table
   before it reads the device row. So a revoke that commits before the read shows in
   the row, and a revoke after it reaches the follower of the feed. A revoke cancels
   each connection of the device: efrd drops the frames that wait and closes with
   1001. The lifetime of a socket depends on the revoke,
   not on a short token lifetime.

9. **The phone gets drafts, merged when it is slow.** The phone subscribes with
   `drafts: true`. efrd merges the drafts of a slow subscriber instead of dropping
   them, and a snapshot holds the live text and reasoning of the running message, so
   a phone that joins a turn shows it at once.

## Consequences

- No third party carries the traffic, and nothing listens outside the tailnet. A
  stolen token without the terminal's yes, a key without its tailnet node, and a
  local process all fail.
- efr depends on tailscaled 1.100 or later with a TUN device. When Tailscale is down,
  the phone cannot connect, and the Unix socket works as before.
- When the user installs Tailscale on the phone again, the phone is a new node and
  must pair again.
- A tagged machine cannot serve a phone. This fails closed by design.
- fastwebsockets has had no release for 21 months. If it breaks, tokio-websockets
  0.13.3 is the fallback; it adds one crate. tokio-tungstenite needs the `log` crate,
  so it needs a change of `deny.toml`.
- The app dials only the tailnet IPv4 address, never the MagicDNS name, so the
  Origin rule takes only the bound address. A later change of the address needs a
  new pairing code.
- The payload of a frame is zeroed after its decode, but not every buffer of the
  parser. So efrd must not take a password over the WebSocket, and v1 has no
  `input.respond` from a phone.
- The phone app is iPhone only, built with EAS, which needs a paid Apple Developer
  account. Android would need a StrongBox or Keystore key in place of the Secure
  Enclave.
- Later options that keep this design: `wss://` through the HTTPS feature of the
  tailnet or Tailscale Serve, an app capability in the tailnet policy that efrd reads
  from the whois, push through APNs, and image uploads by a method of their own.

## Rejected

- A hosted relay (Happy, Codex RC, Claude RC): a third party and an account, for work
  that WireGuard already does.
- A listener on `0.0.0.0` with an address filter (t3code): one wrong rule exposes
  efrd to the LAN.
- A shared password or a bearer token (OpenCode, t3code): a copied secret is enough,
  and a revoke of one device signs out all of them.
- A software Ed25519 key in the keychain: the key can leave the phone.
- Redeeming the pairing with a GET (OpenCode): a link preview spends the token.
