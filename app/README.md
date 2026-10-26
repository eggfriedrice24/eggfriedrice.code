# efr phone app

The iPhone app of efr. It connects to efrd over Tailscale, shows your conversations
live, sends prompts and answers the questions of the agent. It is a React Native and Expo
app. It lives outside the Cargo workspace, and every JS config file is in this directory.

The user guide is `docs/use/phone.md`. The protocol is `docs/protocol.md`, section
"Remote devices". The decision record is `docs/adr/0022-remote-listener-and-device-keys.md`.

## Status

The app is a preview. It is not in the App Store.

- Done and tested in Vitest: the protocol types, the WebSocket client (hello, the proof,
  the subscribe with drafts, the liveness probe, the reconnect and the backoff), the
  store of each conversation, the pairing flow, the session that joins them for the
  screens (`src/session`), the pacer of the streamed text, and the replay of the
  recordings of efrd.
- Written, but not built yet: the screens, the Swift module of the device key and the
  native adapters. They need an EAS build. Nobody ran the app on an iPhone yet.
- `src/app/_layout.tsx` gives the screens the session of `src/session/native.ts`. It
  reads the pairing, connects to efrd and keeps the connection up. A dev build can
  also open a demo session, which reaches no computer.
- The app hears of the foreground from AppState. No package of the app reports a
  change of the network path, such as from Wi-Fi to cellular, so the client finds a
  dead socket with its liveness rule instead.

## Layout

| Path                     | What it holds                                                                                         |
| ------------------------ | ----------------------------------------------------------------------------------------------------- |
| `src/app/`               | The routes of expo-router: the conversation list, the pairing screen and a conversation.              |
| `src/core/`              | Pure code: the frame envelope, the reducer, the pacer, the liveness rule, the backoff, UTF-8 offsets. |
| `src/protocol/`          | `generated.ts` (the protocol types) and the signed message of `device.prove`.                         |
| `src/net/`               | The WebSocket client and the connection. `native/` holds the adapters of React Native.                |
| `src/store/`             | The conversations, the approvals and the event cache in expo-sqlite.                                  |
| `src/pairing/`           | The QR payload, the pairing flow, the device key store and the paired device record.                  |
| `src/session/`           | The session of the screens over the client, the stores and the pairing flow. `native.ts` wires it.    |
| `src/ui/`                | The screens, the transcript, the approval cards, the tool cards and the diff view.                    |
| `src/theme/palette.ts`   | The colours of the design system.                                                                     |
| `modules/efr-device-key` | The local Expo module of the Secure Enclave key (Swift). Its README tells what it does.               |
| `test/recordings/`       | What efrd sends to a phone during scripted turns. The replay tests read it.                           |
| `scripts/`               | The code generator of `src/protocol/generated.ts`.                                                    |

## Toolchain

Every version is pinned exactly. Do not change a version without a reason, and bump
`typescript` and `oxlint-tsgolint` as a pair.

- Node 24.22.1 (`devEngines.runtime` in `package.json`; pnpm downloads it), pnpm 11.28.8
  (`packageManager`).
- Expo SDK 58 (expo ~58.0.19), React Native 0.88.0, React 19.3.0.
- TypeScript 7.0.2. Expo pins an older version, so `expo.install.exclude` keeps it out of
  `expo install`.
- oxlint 1.89.0 with oxlint-tsgolint 7.0.2005 (type-aware, warnings fail), oxfmt 0.74.0,
  vitest 5.0.5, fast-check 4.10.2, json-schema-to-typescript 16.0.0.

pnpm 11 keeps its safe defaults. Only the packages in `allowBuilds` of
`pnpm-workspace.yaml` can run an install script. Add a package only from the table of the
research, or with the approval of the user.

## Work on Linux

Install pnpm 11.28.8, then:

```sh
cd app
pnpm install --frozen-lockfile
```

The gates are the same as in CI. From the repository root:

```sh
just app-check
```

It runs, in this order: `pnpm install --frozen-lockfile`, `pnpm run fmt:check` (oxfmt),
`pnpm run lint` (oxlint), `pnpm run typecheck` (`tsc --noEmit`), `pnpm run test` (vitest)
and `pnpm run check:protocol`. `pnpm run fmt` formats the files.

The tests cover the pure code: `src/core`, `src/protocol`, `src/net`, `src/store`,
`src/pairing`, `src/session`, the pure parts of `src/ui`, and the code generator. They run in Node and
import nothing from React Native. A test uses a fake socket and a fake clock, and never
reaches the network.

### When the protocol changes

The types in `src/protocol/generated.ts` come from `docs/protocol.schema.json`. Do not
edit the file by hand. After a change of `efr-protocol`:

```sh
cargo xtask protocol-schema
cd app && pnpm gen:protocol
```

Commit the schema and `generated.ts` together. `just tidy` fails when the schema is not
current, and `pnpm run check:protocol` fails when `generated.ts` is not.

### The recordings

`test/recordings/*.ndjson` are what efrd sends to a phone during scripted turns.
`just bless` writes them again. Read the diff before you commit it. The README of that
directory gives the format and the scenarios.

### The colours

`src/theme/palette.ts` holds the colours of the eggfriedrice design system, copied from
the palette file of eggfriedrice.nvim. Its header names the file and the commit. Use only
those roles, and never invent a colour or a mapping. The app is dark only.

## Build for an iPhone with EAS

You cannot build an iOS app on Linux. EAS Build builds it in the cloud.

You need:

- a paid Apple Developer account (EAS cannot install a build on an iPhone without one),
- an Expo account,
- the iPhone with Developer Mode on (Settings, Privacy and Security, Developer Mode).

`eas.json` has one profile, `preview`: a release build for internal distribution, with
Node 24.22.1 and pnpm 11.28.8. There is no `development` profile, because the app does not
have `expo-dev-client` yet. That package needs the approval of the user.

The steps, in `app/`:

1. Log in to Expo:

   ```sh
   pnpm dlx eas-cli@25.3.0 login
   ```

2. Link the project to your Expo account. This adds `extra.eas.projectId` to `app.json`.
   Commit that change.

   ```sh
   pnpm dlx eas-cli@25.3.0 init
   ```

3. Register the iPhone for internal distribution. Open the link that the command shows on
   the iPhone and install the profile.

   ```sh
   pnpm dlx eas-cli@25.3.0 device:create
   ```

4. Build. EAS asks for your Apple account, and makes the certificate and the
   provisioning profile.

   ```sh
   pnpm dlx eas-cli@25.3.0 build --platform ios --profile preview
   ```

5. Open the link of the build on the iPhone and install the app.

A `preview` build holds its JavaScript, so each change of the app needs a new build.
Collect the changes of native code, so that the builds stay few.

### Checks on the first build

1. Do the checks of `modules/efr-device-key/README.md`, section "Checks on an EAS build".
2. Stream a long reply (about 10,000 tokens) and look at the memory and the lag of
   react-native-enriched-markdown 1.1.1. If it grows or lags, try 1.2.0.
3. Compare the inverted FlashList with Legend List on a long transcript. The transcript
   list is behind one small component (`src/ui/transcript_list.tsx`), so the list library
   can change.
4. The app sends everything over the WebSocket and calls no `fetch()` to efrd. Check once
   that `ws://` to the 100.x address works with only `NSAllowsLocalNetworking`.

Then test with the real efrd: set `[remote] enabled = true`, restart efrd, pair the phone
with `efr device add`, send a prompt, answer an approval, and revoke the device with
`efr device revoke`.

## Rules of the app

- No raw HTML in Markdown and no WebView with JavaScript. The app asks before it opens a
  link.
- The device key never leaves the Secure Enclave. The app keeps the device id and the
  endpoint in expo-secure-store.
- The app sends no `input.respond`, no attachments and no "always allow". efrd refuses
  them from a phone anyway.
- Offsets of text count bytes of UTF-8, never UTF-16 units (`src/core/utf8.ts`).
