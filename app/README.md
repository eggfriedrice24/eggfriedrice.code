# efr phone app

The iPhone app of efr. It connects to efrd over Tailscale, shows your conversations
live, sends prompts and answers the questions of the agent. It is a React Native and Expo
app. It lives outside the Cargo workspace, and every JS config file is in this directory.

The user guide is `docs/use/phone.md`. The protocol is `docs/protocol.md`, section
"Remote devices". The decision record is `docs/adr/0022-remote-listener-and-device-keys.md`.
The rules for coding agents are in `AGENTS.md` in this directory.

## Status

The app is a preview. It is not in the App Store. It ran as a dev build on an iPhone
with iOS 26: pairing, chat, approvals, steer, queue and the conversations sheet work.

- The screens follow the design canvas of the app: a blank conversation as the root,
  the conversation with its transcript and composer, the All conversations sheet,
  Settings, and the pairing screens with each error. Each screen and each state of the
  canvas is in the code.
- Approvals follow round 8 "minimal" (boards C-Waiting, S-Approvals, AP-Sheets,
  R-Approval): the waiting tool card in the transcript, a one-line strip in the
  composer that answers, and the approval sheet (the route `/approval`, a form sheet
  that fits its content) with the full command or diff. The strip or the card opens the
  sheet; nothing opens it by itself.
- Auto calls follow AU-Auto and errata 1, 6, 7 and 9 of round 6: the tool card shows
  "reviewing", the verdict of the reviewer, of an always rule or of efr's own floor in
  the words of the terminal (`src/features/conversation/model/exit-words.ts`), and what
  the call got on the same line. The pause of the reviewer is one quiet line, and asks
  nothing.
- Quarantine with the reviewer on (Q-Quarantine 7): one quiet line per changed file
  that runs code on the card of its call, "moved to quarantine" or "left in place".
- The question of the reviewer off (`surface_question_requested`, Q-Quarantine 1 to 6):
  the waiting card ("git settings changed"), the strip ("keep .git/config
  (core.fsmonitor)?") and the sheet with the changes that went to quarantine, then
  "kept", "left in quarantine" or "not answered · left in quarantine". The phone
  answers Leave (Q-Quarantine 8, 9); efrd refuses a Keep from a phone
  (`RemoteSurfaceAnswer`), so the strip and the sheet show "Keep on <machine>" where the
  Keep button was, and the status line says "waiting for you".
- The Face ID unlock (S-Lock, round 9): a switch in Settings, off by default. With it
  on, the app locks at a cold start and after 5 minutes in the background
  (features/navigation/model/app-lock.ts), the lock screen and the cover lie over the
  whole window, and links wait for the unlock. Auto from the phone needs it.
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
- A step of web search, which the provider of the model runs, shows as a tool block
  with the verb `web`: a search with its query, an opened page as `open <page>`. The
  status says `✓ 10 results · 6s` (the count only when the provider gives it), `✓ 5s`,
  or why the step failed, such as `too many requests · 1s`. A find in a page has no
  block. The citation marks of an answer (`[1]`) show as links through the Markdown of
  the reply. From `turn_web_report`, the end of the turn lists the sources that the
  answer cites (`[1] protondb.com/app/2384710  ROB RUN · ProtonDB`, a tap asks and then
  opens the page), counts the searches on the end line (`· 5 searches`, `, stored copy`)
  and, under the first turn with a stored copy, says that it can be some days old and
  what gives the live web (`stored_copy_reason`, the words of the terminal).
- The app hears of the foreground from AppState, and of a change of the network path
  (such as from Wi-Fi to cellular) from NetInfo. Then each machine connects again at
  once.
- A dev build and a preview build have the dev tools: three demo machines (desktop,
  laptop and homelab), which reach no computer, and a gallery of every board of the
  design canvas, plus three boards of round 6: S-ModelSheet (the model sheet in its
  states), AU-Auto (auto calls, quarantine cards, a switch of account and an account
  that is out, in one transcript) and Q-Quarantine (the strip and the sheet). The demo
  machines answer `models.list` with the accounts of the boards, so the model sheet
  and Settings › Accounts show them. A long press on the version in Settings, section
  About, opens the gallery. The gallery also opens the bench of the frame decoder. The
  render counter is only in a dev build, because React counts renders only there.

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
- Expo SDK 58 (expo ~58.0.25), React Native 0.88.1, React 19.3.0.
- TypeScript 7.0.2. Expo pins an older version, so `expo.install.exclude` keeps it out of
  `expo install`.
- oxlint 1.89.0 with oxlint-tsgolint 7.0.2005 (type-aware, warnings fail), oxfmt 0.74.0,
  vitest 5.0.5, fast-check 4.10.2, json-schema-to-typescript 16.0.0.
- jest 30.5.2 with jest-expo 58.0.14, @testing-library/react-native 14.1.0 and
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
   app, with the React Compiler, then `scripts/check-bundle.mjs`: the bundle holds no
   dev tools)

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
`/`, and its bundle does not hold the dev tools. The dev routes and the root layout
require the gallery, the bench and the demo machines only under the full test of
`src/lib/dev/dev-tools.ts`. Metro folds that test in such a build and drops the require.
`pnpm run bundle:check` fails when the bundle holds a line of a recording or a title of
the gallery (`scripts/check-bundle.mjs`).

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
expo-clipboard, expo-web-browser, NetInfo, expo-device, react-native-enriched-markdown,
expo-local-authentication (the Face ID unlock, with the Face ID text of `app.json`), the
effort slider (`@react-native-community/slider`) and the module of the device key. The
pnpm patch of the slider changes its native code (`RNCSliderComponentView.mm`, the move
from stop to stop), so a change of that patch needs a new build too. A build without
the slider still runs the JavaScript of a newer app: the model sheet then draws the
efforts as buttons. One dev build takes the slider, expo 58.0.25 with the SDK packages
of `expo install --fix` (React Native 0.88.1) and enriched 1.2.0 together.

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

8. The list: FlashList 2.3.2 in chat mode (not inverted, `startRenderingFromBottom`).
   A conversation opens at its end, a new row scrolls into view while you are within
   200 pt of the end, and the keyboard lifts the content only then. Open a tool block
   with "show all N lines" and close it again: the list must not move to the end, and
   the block stays where it is on the screen (unless the list was at its very end and
   gets shorter than the screen). A conversation with one prompt does not move when you
   drag it, and does not bounce. A tap on its empty space closes the keyboard, and with
   the keyboard up a drag down closes it too. Compare it with Legend List 3.6.0
   on a recorded fast stream, with the keyboard open, scrolled up, and 500 rows of
   history. Measure the largest drift and the slowest commit. Only `src/features/conversation/components/transcript-list.tsx` knows the list
   library. Legend List is not installed yet: add it before you try it.
9. Stream a long reply (about 10,000 tokens). Look at the memory and the lag of
   react-native-enriched-markdown 1.2.0, the frames of the code blocks, and the fade of
   the streaming tail. A fence body of more than one line draws its text in the
   sideways scroll of its frame: 1.2.0 has the fix of upstream PR #1017, so the app
   has no patch of enriched any more.
10. Markdown with enriched 1.2.0: code blocks use Menlo, the underline of a link is cyan
    (not purple), strong text is bold (not semibold), and a table has one border width.
    Check that these look right, and that the limits in `src/theme/markdown.ts` still
    hold.
11. Recycled rows: scroll fast up and down a long conversation with many replies. Each
    row must show its own text, and a "copied" or a cursor of one reply must never show
    on another one. The jest tests mock the list, so only the phone shows this.
12. Older history: open a conversation that is longer than its snapshot and scroll to
    the top. The older turns come in above, the place of the reader stays, and "Jump to
    latest" does not show for them. Kill the app, open it again without a network, and
    check that the older turns are still there.
13. A turn that searched the web: each search and each opened page is one tool block
    (`web` and the query, or `open` and the page), in the order of their starts, with
    its count of results and its time; a failed one names its reason. The marks `[1]`
    of an answer of a Claude model are links that open their page. The sources over the
    end line fit on one line each, and a tap on one asks, then opens its page.
14. A reply with an image (`![x](https://example.com/x.png)`) and an HTML block
    (`<video src="https://example.com/v.mp4">`): the image shows as "!" and the link
    "x", the video line shows as text, and nothing loads (watch the log of a proxy, or
    of the web server).

### Speed

15. zod on Hermes: run the bench route of a preview build, which is a release build. Open
    it right after a launch, to get a cold first parse. It parses each frame of the
    recordings through the schemas, in three modes. The draft frames must stay under
    0.5 ms at p95. If they do not, use jitless mode or smaller draft schemas.
16. The React Compiler: the app must behave the same with it on. Count the renders of a
    streaming conversation with the render counter of a `development` build.

### The header and the navigation

17. The title (`Stack.Title asChild` with our button): its layout, the yolk dot, the
    size of its tap target, and that a tap opens the conversations sheet.
18. The menu glyphs: every glyph shows (Settings row menu, Steer/Queue, the copy
    menus), with no gap before a title and no title on top of the row menu. Before,
    Fabric gave each glyph a clear colour.
19. The anchor of a menu that opens from a view (`findNodeHandle`).
20. The `Stack.Toolbar` glass group on iOS 26, the titles of the back history menu
    (long press on back), and the bar on iOS 26: no fill and no blur of its own, the
    soft scroll edge only at the top of the transcript, no blur over old turns or over
    the composer, and the glass capsule of the strip 8 pt under the bar. Before iOS 26
    the bar has the chrome blur. "Jump to latest" shows and hides its glass with the
    animation of the system, with no flash.
21. The first send on the blank root opens the new conversation with no animation, and
    the id of the conversation then changes in place (`replaceParams`). The new params
    have no `enter`, so back and the edge swipe from that conversation have the normal
    animation.
22. The ninth conversation in the stack drops the oldest with no flash. `dismissTo`
    goes back to a conversation in the stack. Start after a pairing, and Forget of the
    last machine, go to the root with a short cross-fade (`animation: "fade"` on the
    screen that goes, set just before the reset), and with none when Reduce Motion is
    on.
23. The conversations sheet: its two sizes, the native search bar and the compose
    button (New conversation) in the bottom toolbar of iOS 26, that the list ends above
    the toolbar, and that a tap on a row closes the sheet and opens the conversation.
    The rows follow S-Rows: at least 58 pt high, the icon column 24 pt wide (the yolk
    "!" circle, the yolk braille loader that runs and is still with Reduce Motion, the
    quiet bubble), the separators 52 pt in, under the text, and the time at the top
    right of every row. The section headers are plain and stick at the top while their
    rows scroll, with no band of their own at rest and no rows that show through them.
    The words of a search are bold in the titles, and a long press shows Open and Copy
    ID. VoiceOver reads a row as one element, with the hint "Swipe up or down for
    actions."

### The composer

24. The composer docks to the keyboard (`KeyboardStickyView`) and the list keeps its
    inset.
25. At rest the composer is a one-line pill (field and action, 48 pt). A tap focuses
    the input: the status line shows and the input grows to 6 lines, with no jump of
    the transcript. A tap on the transcript or a closed keyboard makes it a pill again,
    with the draft kept. The model button opens the sheet from the full composer. The
    focused field is at least 72 pt and takes the full width under the status line: a
    pasted paragraph and the placeholder start at the top left, in line with the status
    line, and the focus stays when the composer changes size. The placeholder is smaller
    than the text, and the field does not jump at the first letter. At rest one row:
    plus, the field, microphone, Send or Stop. Focused, under the field: plus at the
    left, microphone and Send or Stop at the right. Plus and microphone are grey, take no
    tap, and VoiceOver says "Not available yet.".
26. The Steer and Queue menu opens with a long press on the capsule, and a tap on the
    capsule sends. This needs the patch of react-native-menu in a new build: without it,
    the button of a long press menu takes the taps of the views inside it. Check also
    that a tap on a tool block and on "show all N lines" opens it, and that a tap on a
    row of the All conversations sheet opens the conversation.

### Look and motion

27. The BrailleCell and the cursor animate with the native driver. With Reduce Motion
    on, they show a still frame.
28. `fontFamily: "ui-monospace"` gives SF Mono and follows Dynamic Type. If not, the
    app uses Menlo.
29. The app icon at 60 pt and at 29 pt. Make an Icon Composer `.icon` bundle from the
    layers of `assets/icon/` (this needs a Mac).

### The approvals

30. The strip is the first row of the composer pill, 44 pt high. A tap on Deny or Allow
    answers, a tap on the rest of the row opens the sheet, and the touch areas of the
    two buttons do not overlap. With the keyboard up, the strip moves with the
    composer and still takes taps.
31. The approval sheet (`sheetAllowedDetents: "fitToContents"`) fits a short command,
    and a long diff makes it grow to the full height and scroll, with the answer row
    pinned at the bottom. Swipe down and Close in the header (check 45) close it. The
    conversation under it keeps its stream, and the strip shows the next question
    after an answer in the sheet.
32. The waiting tool card shows the pressed fill (selection_bg) and opens the sheet.
    VoiceOver reads "waiting for approval" and "Opens the approval." on the card, and
    reaches Deny and Allow of the strip as their own buttons.

### The model sheet

33. The model sheet (detents 0.75 and full) shows the native header with "Model" and
    "Use" at its right. "Use" is off until something changed, reads "Switch" for a
    switch that compacts, and is off while the sheet asks efrd what the switch costs.
    The native search bar and the filter menu sit in the bottom toolbar of iOS 26.
    While the search has the focus or text, the sheet holds the full height. The
    slider and the quiet line stay pinned under the list.
34. The effort slider is the native UISlider (`@react-native-community/slider` with the
    pnpm patch, a new dev build): the thumb moves from stop to stop while the finger
    moves, with one haptic per stop, each label stands centred under its stop, a tap of
    a label moves the thumb there, the panel keeps one height while the effort changes,
    and VoiceOver reads the efforts. A drag that starts low on the thumb moves it. At the
    largest text size no label is cut and the default dot stays above the quiet line. A build from before the slider draws the steps as
    buttons and still works.
35. The Claude spark keeps its own colour, the OpenAI mark is text_primary, and both
    are sharp at 3x. A signed-out account shows its logos at 40 %.
36. The model button at the end of the status line gives way to the path first, then
    to ctx, then cuts its own end, on the smallest iPhone. The path and ctx each go
    whole with their "·": no lone "·" and no half-cut `ctx` stays. Look with a long
    path, with the limit of an account (`5-hour 92%`), and with "waiting for you".
    - Round 7 (S-ModelSheet r7): the list shows no scroll indicator, and a hairline
      tops the pinned panel only while rows lie under it. The native search bar filters
      as you type and the sheet holds the large detent while it has the focus or text.
      Type a search, then tap a row while the keyboard is up: the keyboard goes, the
      search text stays in the bar, and the slider and the quiet line show again.
      The filter button of the bottom toolbar opens a native menu (All, Claude, OpenAI, then Legacy models); its
      glyph fills while a filter is on. A tap on a star moves the favourite of its
      account and never selects the row; the favourite goes first the next time the
      sheet opens. VoiceOver reaches the star as an action of the row, and after the
      action says the result, such as "GPT-6 Astra is the favourite of the ChatGPT
      plan" or "The ChatGPT plan has no favourite". VoiceOver reads the filter button
      of the native toolbar with its hint "Opens the filters.".
    - S-ComposerR7: the provider mark stands before the model name at 11 pt (the
      Claude spark orange, the OpenAI mark in the colour of the button). A new thread
      starts with the favourite of the last plan. The mode control (cautious, or 🤖 auto)
      left of the model button opens a menu with Cautious and Auto. With the Face ID
      unlock off, Auto is greyed and "Turn on Face ID unlock" opens Settings. With two
      or more paired machines, so that the row is below the fold, Settings opens
      scrolled to Privacy and marks the "Face ID unlock" row for about 1 s.
    - Face ID unlock (S-Lock, Settings round 9; a new dev build with
      expo-local-authentication):
      1. Unlock on, kill the app, open it: the lock screen and the prompt of iOS at once.
      2. Home for less than 5 minutes and back: no prompt. For 5 minutes or more: the
         prompt.
      3. Cancel the prompt: the line and Unlock, and no second prompt by itself.
      4. Fail Face ID until iOS asks for the passcode: the passcode opens the app.
      5. The app switcher shows the cover (the mark), never the transcript, also with the
         model sheet or the approval sheet open, and after a fast swipe home.
      6. The lock screen lies over an open model sheet, approval sheet and All
         conversations sheet on iOS 26, and over an open menu: the mode menu, the
         filter menu of the model sheet and the Steer and Queue menu. VoiceOver reaches
         nothing under it.
      7. The keyboard was up when the app left: it is down on the lock screen.
      8. Control Center, the notification shade, a paste question of iOS and the camera
         question of pairing: the cover shows only while iOS is in front.
      9. The switch asks before it turns on and before it turns off, and does not move
         before the pass. During that prompt Settings stays visible (no cover).
      10. After the switch is off, each conversation shows cautious, and a prompt that
          waited for an offline machine runs as cautious on the desktop.
      11. Remove the passcode of the iPhone while the unlock is on, then open efr inside
          5 minutes: the app opens, the switch is off, Settings says why.
      12. An iPhone with Face ID not set up: the row says "Passcode unlock".
      13. Unlock on, app killed: open `efr://conversations` from Safari. The lock screen
          shows with no sheet over it; after the unlock the sheet opens.
      14. Unlock on, app away for more than 5 minutes: open `efr://pair` from the Camera.
          Pairing opens only after the unlock.
      15. Unlock on and auto: a message runs in auto on the desktop with no extra prompt.
      16. Cold start with the unlock on: after the unlock the first screen comes with no
          visible blank time.
      17. Unlock on: tap a link in a transcript, so the action sheet "Open <host>?"
          shows. Go home: the app switcher shows only the cover, not the address. Come
          back after 5 minutes: the lock screen shows, and the sheet is gone.
      18. Unlock on: open Pair (a full-screen view), swipe home fast, and come back
          after 5 minutes. The lock screen covers Pair.
      19. The mark stands at the same place on the cover and on the lock screen, with
          and without the line. At the largest accessibility text size, Unlock grows
          with its label and the line wraps under the mark.
      20. Unlock on: open the model sheet and tap the search, so its keyboard is up. Go
          home for 5 minutes and come back. The keyboard is down on the lock screen, and
          what you type reaches nothing.

### Accounts, limits, auto and quarantine

Each check names the gallery board that shows it (a long press on the version in
Settings, section About, of a dev or preview build).
Uniwind draws some classes only on the iPhone: `shrink-[1000000]`, `line-through`,
`text-row-strong` and `text-caption-strong`. Look at them in these checks.

37. AC-Out (gallery "AU-Auto"): the end of the stopped turn says why in the words of
    the terminal, such as "the ChatGPT plan reached its usage limit · resets 14:20",
    with one plain "switch to ChatGPT plan · b" per account that serves the model, and
    "b runs again 14:20" for a full account whose reset comes first. A
    signed-out account gets one quiet line with its login command and no action. With
    the real efrd: a tap opens the model sheet on that account, and after the confirm
    the switch actions become "retry", and the retry runs on the new account. A stop of
    "too many requests" names no reset, and its offer stays.
38. AC-Limits (gallery "S-ModelSheet", Settings and the composer): the model sheet
    shows the warning of a plan near its limit and of a full account with its reset.
    From 90 % the status line shows the limit (`5-hour 92%`) with the warning glyph
    while a turn runs, and the path gives way first (`shrink-[1000000]`). Settings ›
    Accounts on <machine> has one row per account, and its screen shows each window
    with "limit reached" in state_error. The strong texts (`text-row-strong`,
    `text-caption-strong`) are heavier than the plain ones.
39. MS-Switched (gallery "AU-Auto"): one quiet line in text_subtle above the first
    answer on a new model or account, which names both accounts of a switch on the
    same model.
40. The gone model (gallery "S-ModelSheet", "a model that is gone"): its name is struck
    through (`line-through`) in text_subtle.
41. AU-Auto (gallery "AU-Auto"): under a tool card, "reviewing" while the reviewer
    works, then the verdict on one line, such as `reviewer allowed: <line> for this call
    (low risk; <reason>): github.com:443 reachable`, with `host network` in the
    warning colour. A refusal reads `reviewer refused: <line>: <reason>` in text_muted,
    and a refusal of efr's floor `efr: <line>: a secret (floor)`. The pause of the
    reviewer is one quiet mono line in its turn. Long lines wrap and stay readable.
42. Quarantine on the card (gallery "AU-Auto"): each changed git setting or other file
    that runs programs is one muted row under the tool card, with `moved to
    quarantine` or `left in place`.
43. Q-Quarantine 8, 9 (gallery "Q-Quarantine" and "AU-Auto"): with the reviewer off,
    the strip has two rows: the question, then "Keep on <machine>" in text_subtle and
    the capsule Leave. The sheet has the same answer row. A tap on Leave sends the
    answer and the card says "left in quarantine". The status line says "waiting for
    you". VoiceOver reads "Leave in quarantine" and "Keep is possible only on
    <machine>". After an answer in the terminal, the card says "kept" or "left in
    quarantine", and the sheet adds where it came from.
44. The web end line (no board; a real turn with web search on the desktop): the end
    line counts the searches (`2 searches`), the sources over it fit on one line each,
    and with `web.search = "cached"` the end line says `, stored copy` and one quiet
    line says what gives the live web.
45. The header of the approval sheet (no board yet; AP-Sheets has no header): the sheet
    shows the native header with no title, on surface_1 with no hairline, and the
    native xmark Close at its right. With the header, `fitToContents` still fits a
    short command, and a long diff still grows to the full height. Close goes back to
    the conversation. After a question closes in the sheet while more wait, the native
    "Next" stands left of Close and shows the next question; with none waiting, only
    Close shows. Deny and Allow stay the 28 pt decision buttons in the sheet. VoiceOver reads
    "Close" and "Next question".
46. The header of the quarantine sheet and of a question that is not open now (from
    the strip or the waiting card): the same header with Close, and Next after the
    question closed while another question (an approval or a quarantine question)
    waits. Leave stays the 28 pt decision button in the sheet, and after an answer the
    sheet dims with no button.
47. "Use default" of the model sheet: while the quiet line says "chosen here", the
    native header shows "Use default" at its left, and a tap selects the default model
    of the machine ("desktop default" in the quiet line) and takes the item away. It
    never shows next to "desktop default".

### The model's question, the row states and the offline banner (0.0.12)

`src/lib/protocol/status.ts` (`rowState`, `waitsForUser`) is the one rule of the row
state, in the order of `row_state` in efr-cli. `src/features/navigation/use-now.ts` is
one shared minute timer for every reader. These checks need the iPhone:

48. SH-Offline: with one machine offline, the banner is the header of the sheet's
    native list, with `wifi.slash` in text_subtle and one line in text_muted, such as
    `homelab offline · rows from 2h ago`. With two machines, it names both; with
    three, it counts them. The footer "1 machine did not answer" is gone, also from
    the search results. VoiceOver reads the line in full words ("2 hours ago").
49. S-Rows: the rows "needs an answer" and "answer in the terminal" have the yolk "!"
    mark and stand in Needs you; "✗ failed" is in state_error with no mark and stays
    in its day. Leave the sheet open over a minute: the times move from "now" to "1m"
    with no tap.
50. S-Ask 1 and 2: a question of the model shows in the strip with the yolk dot, the
    question and "Answer". A tap opens the native action sheet, anchored to the strip
    on iOS 26, with the question as the title, the `label: description` lines as the
    message, one button per option, "Type an answer", "Skip" and Cancel. Check a
    question of 300 characters and labels of 60 at the largest Dynamic Type size (AX5).
    With an approval and a question, the older goes first and "1 of 2" counts both.
51. S-Ask 3: "Type an answer" opens the keyboard at once. The answer row (`Answer ·
    <question> · xmark`) and the hint `Do not type a password or a token here.` stand
    above the keyboard, the field says "Your answer", and VoiceOver reads "Send the
    answer". Past 1024 bytes, Send dims. The xmark leaves and keeps the text as a draft.
    An answer from the terminal ends the answer state by itself.
52. S-Ask 4: the row of the call says "waiting for you" with the yolk dot, and a tap
    opens the sheet; after the answer it says "answered · <label>", or "answered on
    <machine> · ..." for an answer from the terminal.
53. The 0.0.11 app from TestFlight against an efrd of 0.0.12: the lists load, and a
    conversation with a waiting question of the model opens with no error, and its
    row says "running".

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
