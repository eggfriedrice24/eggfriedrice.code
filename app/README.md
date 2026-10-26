# efr phone app

The iPhone app of efr. It connects to efrd over Tailscale, shows your conversations
live, sends prompts and answers the questions of the agent. It is a React Native and Expo
app. It lives outside the Cargo workspace, and every JS config file is in this directory.

The user guide is `docs/use/phone.md`. The protocol is `docs/protocol.md`, section
"Remote devices". The decision record is `docs/adr/0022-remote-listener-and-device-keys.md`.
The rules for coding agents are in `AGENTS.md` in this directory.

## Status

The app is a preview. It is not in the App Store. Nobody ran it on an iPhone yet.

- The screens follow the design canvas of the app: a blank conversation as the root,
  the conversation with its transcript, composer and approval block, the All
  conversations sheet, Settings, and the pairing screens with each error. Each screen
  and each state of the canvas is in the code.
- The app pairs with more than one machine and connects to all of them at the same time.
  The route of a conversation names its machine (`/c/<daemon id>/<conversation id>`).
- Done and tested on Linux: the protocol types and their zod schemas, the WebSocket
  client (hello, the proof, the subscribe with drafts, the liveness probe, the reconnect
  and the backoff), the stream store of each conversation, the outbox of the prompts,
  the drafts, the machine registry and its supervisors, the queries, the pairing flow,
  the pacer of the streamed text, the replay of the recordings of efrd, and the
  components and screens in jest-expo.
- Written, but not built yet: the Swift module of the device key and the native
  adapters. They need an EAS build.
- The app hears of the foreground from AppState, and of a change of the network path
  (such as from Wi-Fi to cellular) from NetInfo. Then each machine connects again at
  once.
- A dev build and a preview build have the dev tools: three demo machines (desktop,
  laptop and homelab), which reach no computer, and a gallery of every board of the
  design canvas. A long press on the version in Settings, section About, opens the
  gallery. The gallery also opens the bench of the frame decoder. The render counter
  is only in a dev build, because React counts renders only there.

## Layout

| Path                     | What it holds                                                                                          |
| ------------------------ | ------------------------------------------------------------------------------------------------------ |
| `src/app/`               | The routes of expo-router. Each one renders one screen.                                                |
| `src/features/`          | The features: machines, pairing, conversation, conversations, navigation, settings and the demo.       |
| `src/components/ui/`     | The parts of the design that every feature uses: text, buttons, menus, materials and loaders.          |
| `src/lib/protocol/`      | `generated.ts` (the protocol types), `generated-zod.ts` (their schemas), the frame envelope and prove. |
| `src/lib/stream/`        | The reducer of a conversation, the text buffer, the pacer and UTF-8 offsets.                           |
| `src/lib/net/`           | The WebSocket client, the connection, the errors, the liveness rule and the backoff.                   |
| `src/lib/store/`         | The frame store (one update per display frame), the event cache and the key-value store.               |
| `src/lib/query/`         | The QueryClient, its persister, its managers and the shared query keys.                                |
| `src/lib/native/`        | The adapters of React Native and Expo: socket, clock, database, haptics, clipboard, links.             |
| `src/theme/`             | The palette, the colour roles, the type scale, the metrics, the materials and the generated theme CSS. |
| `modules/efr-device-key` | The local Expo module of the Secure Enclave key (Swift). Its README tells what it does.                |
| `assets/icon/`           | The app icon, "the yolk". Its README tells how to make the PNG again.                                  |
| `test/`                  | The rules of the code (dependency graph, effects budget, React Compiler), the codegen and the harness. |
| `test/recordings/`       | What efrd sends to a phone during scripted turns. The replay tests read it.                            |
| `scripts/`               | The code generators of the protocol and the theme, the effects budget and the decoder bench.           |

`AGENTS.md` tells the shape of a feature and the rules of the layers.

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
- jest 30.5.2 with jest-expo 58.0.12, @testing-library/react-native 14.1.0 and
  test-renderer 1.3.1 for the tests of the components and the routes (`*.test.tsx`).
- The React Compiler (`experiments.reactCompiler` in `app.json`) and uniwind 1.13.0 with
  tailwindcss 4.3.3 (`metro.config.js`). Metro writes `uniwind-types.d.ts` again on each
  run; commit it when it changes.
- TanStack Query 5.104.5 (with its persister), TanStack Store 0.11.2, TanStack Pacer
  0.23.1 and zod 4.6.7. Store and Pacer are before 1.0, so a bump can change their API.

pnpm 11 keeps its safe defaults. Only the packages in `allowBuilds` of
`pnpm-workspace.yaml` can run an install script. Add a package only with the approval of
the user.

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

It runs, in this order:

1. `pnpm install --frozen-lockfile`
2. `pnpm run fmt:check` (oxfmt)
3. `pnpm run lint` (oxlint, type-aware)
4. `pnpm run typecheck` (`tsc --noEmit`)
5. `pnpm run test` (vitest)
6. `pnpm run test:ui` (jest)
7. `pnpm run check:protocol` (the generated protocol files are current)
8. `pnpm run check:theme` (the generated theme CSS is current)
9. `pnpm run bundle:check` (`expo export --platform ios`, the JavaScript bundle of the
   app, with the React Compiler)

`pnpm run fmt` formats the files.

There are two test runners:

- Vitest runs the `*.test.ts` files in Node: the pure code of `src/lib`, the models of
  the features, the schemas over every fixture of efr-protocol, the replay of the
  recordings, the code generators, and the rules of the code in `test/`. These tests
  import nothing from React Native.
- jest-expo runs the `*.test.tsx` files: the components, the screens and the routes,
  with React Native Testing Library and the testing library of expo-router.
  `jest-matchers.d.ts` gives the matchers of both libraries their types.

A test uses fake sockets, a fake clock and memory stores, and never reaches the
network.

The rules of the code are tests too:

- `test/dependency-graph.test.ts`: the layers, the `@/` alias, no cycle, no barrel, and
  zod calls only in schema files.
- `test/effect-budget.test.ts`: the `useEffect` and `useLayoutEffect` calls of each file
  against `scripts/effect-budget.json`. The budget only goes down.
- `test/react-compiler-coverage.test.ts`: each component and hook compiles with the
  React Compiler and no bailout.

`node scripts/bench-decode.mjs` measures the decoder of the app in Node, with zod, with
zod in jitless mode and with the old hand-written guard. Run it alone and at nice 19.
It does not give a number for the iPhone: the bench route of a dev build does.

### When the protocol changes

The types in `src/lib/protocol/generated.ts` and the schemas in
`src/lib/protocol/generated-zod.ts` come from `docs/protocol.schema.json`. Do not edit
them by hand. After a change of `efr-protocol`:

```sh
cargo xtask protocol-schema
cd app && pnpm gen:protocol
```

Commit the schema and the generated files together. `just tidy` fails when the schema
is not current, and `pnpm run check:protocol` fails when the generated files are not.

Each schema satisfies its generated type, so a schema that does not agree with the type
fails `tsc`. The decoder turns an unknown kind, and a known kind that does not parse,
into an `other` item, and the stream goes on.

### The recordings

`test/recordings/*.ndjson` are what efrd sends to a phone during scripted turns.
`just bless` writes them again. Read the diff before you commit it. The README of that
directory gives the format and the scenarios.

Vitest keeps the list of fixture files in `node_modules/.vitest-cache`. After you add a
fixture or a recording, delete that directory, or the new file can stay hidden.

### The colours and the theme

`src/theme/palette.ts` holds the colours of the eggfriedrice design system, copied from
the palette file of eggfriedrice.nvim. Its header names the file and the commit. Use only
those colours, and never invent a colour or a mapping. The app is dark only.

`src/theme/roles.ts` gives each colour of the app a role, and each role names one entry
of the palette. `pnpm gen:theme` writes `src/theme/theme.css` from the roles, the type
scale (`type.ts`) and the metrics (`metrics.ts`). The components use its classes with
uniwind, such as `text-text-muted`, `text-mono-xs` or `rounded-box`. Do not edit
`theme.css` by hand.

## Build for an iPhone with EAS

You cannot build an iOS app on Linux. EAS Build builds it in the cloud.

You need:

- a paid Apple Developer account (EAS cannot install a build on an iPhone without one),
- an Expo account,
- the iPhone with Developer Mode on (Settings, Privacy and Security, Developer Mode).

`app.config.ts` makes three variants of the app from `APP_VARIANT`, as bondid does. Each
has its own name, bundle id and scheme, so all three can be on one iPhone:

| Variant                     | Name        | Bundle id                      | Scheme        |
| --------------------------- | ----------- | ------------------------------ | ------------- |
| `development` (the default) | efr Dev     | `dev.eggfriedrice.efr.dev`     | `efr-dev`     |
| `preview`                   | efr Preview | `dev.eggfriedrice.efr.preview` | `efr-preview` |
| `production`                | efr         | `dev.eggfriedrice.efr`         | `efr`         |

`eas.json` has one profile per variant, with Node 24.22.1 and pnpm 11.28.8. Each profile
sets `APP_VARIANT`:

- `development`: a dev build with `expo-dev-client`, for internal distribution. It loads
  its JavaScript from Metro on your computer (`pnpm dev:client`), and `__DEV__` is on, so
  it has the dev tools and the render counter.
- `preview`: a release build for internal distribution that holds its JavaScript. It sets
  `EXPO_PUBLIC_EFR_DEV_TOOLS=1`, so it has the demo machines, the gallery and the bench
  route (`src/lib/dev/dev-tools.ts`), but not the render counter.
- `production`: the TestFlight and App Store build. The build number comes from EAS and
  goes up on each build.

A build without `EXPO_PUBLIC_EFR_DEV_TOOLS` and without `__DEV__` sends each dev route to
`/`.

The bench reads `src/lib/dev/recordings.ts`, a copy of `test/recordings`. A test fails
when the copy and the recordings differ. To write the copy again, run
`EFR_WRITE_DEV_RECORDINGS=1 pnpm vitest run src/lib/dev/recordings.test.ts` in `app/`.

The project is linked to the Expo account `eggfriedrice` (`extra.eas.projectId` in
`app.json`). `eas-cli` is a dev dependency, so `pnpm exec eas` runs the pinned version.

The steps, in `app/`:

1. Log in to Expo once: `pnpm exec eas login`.
2. Register the iPhone for internal distribution once: `pnpm exec eas device:create`.
   Open the link that it shows on the iPhone and install the profile.
3. Build the dev client: `pnpm eas:ios:dev`. The first build of each bundle id asks for
   your Apple account and makes the certificate and the provisioning profile, so run it
   in a terminal where you can answer.
4. Open the link of the build on the iPhone and install "efr Dev".
5. Start Metro: `pnpm dev:client`. The iPhone must reach your computer on the LAN or the
   tailnet. Open efr Dev and pick the server.

Other builds:

- `pnpm eas:ios:preview`: the preview build (gallery and bench without Metro).
- `pnpm eas:ios:testflight`: the production build, sent to TestFlight when it is done.
- `pnpm config:dev`, `pnpm config:preview` and `pnpm config:prod` show the resolved config
  of each variant.

A `preview` or `production` build holds its JavaScript, so each change of the app needs a
new build. Collect the changes of native code, so that the builds stay few. The native
modules of the app are react-native-menu, expo-blur, expo-glass-effect, expo-symbols,
expo-clipboard, expo-web-browser, NetInfo, expo-device, react-native-enriched-markdown
and the module of the device key.

## Device checklist

Linux cannot build iOS and cannot show the UI. Do these checks on the iPhone after the
first EAS build. Write down each result. A check that fails needs a change of the code,
not a test that pretends.

### The build and the connection

1. Do the checks of `modules/efr-device-key/README.md`, section "Checks on an EAS build".
2. The app sends everything over the WebSocket and calls no `fetch()` to efrd. Check
   once that `ws://` to the 100.x address works with only `NSAllowsLocalNetworking`.
3. Test with the real efrd: set `[remote] enabled = true`, restart efrd, pair the phone
   with `efr device add`, send a prompt, answer an approval, and revoke the device with
   `efr device revoke`.

### Pairing

4. The scanner (`CameraView.launchScanner`) reads the code of `efr device add`.
5. "Open Tailscale" opens the Tailscale app.
6. The terminal shows the model name of the iPhone (expo-device), such as "iPhone 17
   Pro".
7. Re-pair one machine from Settings, and check that the other machines stay paired.

### The transcript

8. The list: compare the inverted FlashList 2.3.2 with Legend List 3.6.0 on a recorded
   fast stream, with the keyboard open, scrolled up, and 500 rows of history. Measure
   the largest drift and the slowest commit. Check the VoiceOver order in an inverted
   list. Only `src/features/conversation/components/transcript-list.tsx` knows the list
   library. Legend List is not installed yet: add it before you try it.
9. Stream a long reply (about 10,000 tokens). Look at the memory and the lag of
   react-native-enriched-markdown 1.1.1, the frames of the code blocks, and the fade of
   the streaming tail.
10. Markdown with enriched 1.1.1: code blocks use Menlo, the underline of a link is cyan
    (not purple), strong text is bold (not semibold), and a table has one border width.
    Check that these look right.
11. Recycled rows: scroll fast up and down a long conversation with many replies. Each
    row must show its own text, and a "copied" or a cursor of one reply must never show
    on another one. The jest tests mock the list, so only the phone shows this.
12. Older history: open a conversation that is longer than its snapshot and scroll to
    the top. The older turns come in above, the place of the reader stays, and "Jump to
    latest" does not show for them. Kill the app, open it again without a network, and
    check that the older turns are still there.
13. A reply with an image (`![x](https://example.com/x.png)`) and an HTML block
    (`<video src="https://example.com/v.mp4">`): the image shows as "!" and the link
    "x", the video line shows as text, and nothing loads (watch the log of a proxy, or
    of the web server).

### Speed

14. zod on Hermes: run the bench route of a preview build, which is a release build. Open
    it right after a launch, to get a cold first parse. It parses each frame of the
    recordings through the schemas, in three modes. The draft frames must stay under
    0.5 ms at p95. If they do not, use jitless mode or smaller draft schemas.
15. The React Compiler: the app must behave the same with it on. Count the renders of a
    streaming conversation with the render counter of a `development` build.

### The header and the navigation

16. The title menu (`Stack.Title asChild` with `MenuView`): its layout, the yolk dot
    and the size of its tap target.
17. The menu glyphs: `imageColor` must give yolk glyphs, not the colour of the label.
    The braille glyph of a running conversation: a menu takes only SF Symbols.
18. The anchor of a menu that opens from a view (`findNodeHandle`).
19. The `Stack.Toolbar` glass group on iOS 26, the titles of the back history menu
    (long press on back), and the strip under the transparent bar with the scroll edge
    effect.
20. The first send on the blank root opens the new conversation with no animation, and
    the id of the conversation then changes in place (`replaceParams`). The new params
    have no `enter`, so back and the edge swipe from that conversation have the normal
    animation.
21. The ninth conversation in the stack drops the oldest with no flash. `dismissTo`
    goes back to a conversation in the stack. Start after a pairing, and Forget of the
    last machine, go to the root with a short cross-fade (`animation: "fade"` on the
    screen that goes, set just before the reset), and with none when Reduce Motion is
    on.
22. The All conversations sheet: its two sizes, with the search field.

### The composer

23. The composer docks to the keyboard (`KeyboardStickyView`) and the list keeps its
    inset.
24. The input grows to 6 lines.
25. The Steer and Queue menu opens with a long press on the capsule.

### Look and motion

26. The BrailleCell and the cursor animate with the native driver. With Reduce Motion
    on, they show a still frame.
27. `fontFamily: "ui-monospace"` gives SF Mono and follows Dynamic Type. If not, the
    app uses Menlo.
28. The app icon at 60 pt and at 29 pt. Make an Icon Composer `.icon` bundle from the
    layers of `assets/icon/` (this needs a Mac).

## Rules of the app

- No image, no video and no raw HTML in Markdown, and no WebView with JavaScript. The app
  gives enriched only inert Markdown: an image becomes a link, and no line can begin an
  HTML block. So a reply cannot make the phone load a URL. The app asks before it opens
  a link, and then opens it in SFSafariViewController.
- The device key never leaves the Secure Enclave. The app keeps the device id and the
  endpoint of each machine in expo-secure-store.
- The app sends no `input.respond`, no attachments and no "always allow". efrd refuses
  them from a phone anyway.
- A question that only the terminal can answer (a command that needs a password or
  runs as another user, a background job, a delete that full mode guards) shows why,
  and no answer buttons. A question that offers an "always" rule keeps Allow and Deny,
  and tells that the rule is set in the terminal.
- Offsets of text count bytes of UTF-8, never UTF-16 units (`src/lib/stream/utf8.ts`).
