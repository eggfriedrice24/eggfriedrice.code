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
  "kept", "left in quarantine" or "not answered · left in quarantine". efrd takes the
  answer only from the terminal (`RemoteSurfaceAnswer`), so the strip and the sheet say
  "answer on <machine>" in place of Keep and Leave, and the status line says "waiting
  in the terminal".
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
the effort slider (`@react-native-community/slider`) and the module of the device key.
A build without the slider still runs the JavaScript of a newer app: the model sheet then
draws the efforts as buttons.

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
23. The conversations sheet: its two sizes, the search field under "New conversation"
    and not under the bar, the status glyphs, and that a tap on a row closes the sheet
    and opens the conversation.

### The composer

24. The composer docks to the keyboard (`KeyboardStickyView`) and the list keeps its
    inset.
25. The input grows to 6 lines.
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
    pinned at the bottom. Swipe down closes it. The conversation under it keeps its
    stream, and the strip shows the next question after an answer in the sheet.
32. The waiting tool card shows the pressed fill (selection_bg) and opens the sheet.
    VoiceOver reads "waiting for approval" and "Opens the approval." on the card, and
    reaches Deny and Allow of the strip as their own buttons.

### The model sheet

33. The model sheet (`sheetAllowedDetents: "fitToContents"`) fits two accounts, and four
    accounts make it grow to the full height and scroll, with the slider, the quiet line
    and the confirm pinned at the bottom.
34. The effort slider is the native UISlider (`@react-native-community/slider`, a new
    dev build): it snaps to each step, the thumb moves on a tap of a step label, and
    VoiceOver reads the efforts. A build from before the slider draws the steps as
    buttons and still works.
35. The Claude spark keeps its own colour, the OpenAI mark is text_primary, and both
    are sharp at 3x. A signed-out account shows its logos at 40 %.
36. The model button at the end of the status line gives way to the path first, then
    cuts its own end, on the smallest iPhone.

### Accounts, limits, auto and quarantine

Each check names the gallery board that shows it (a long press on the version in
Settings, section About, of a dev or preview build).
Uniwind draws some classes only on the iPhone: `shrink-[1000]`, `line-through`,
`text-row-strong` and `text-caption-strong`. Look at them in these checks.

37. AC-Out (gallery "AU-Auto"): the end of the stopped turn says why in the words of
    the terminal, such as "the ChatGPT plan reached its usage limit · resets 14:20",
    with one plain "switch to ChatGPT plan · b" per account that serves the model. A
    signed-out account gets one quiet line with its login command and no action. With
    the real efrd: a tap opens the model sheet on that account, and after the confirm
    the switch actions become "retry", and the retry runs on the new account. A stop of
    "too many requests" names no reset, and its offer stays.
38. AC-Limits (gallery "S-ModelSheet", Settings and the composer): the model sheet
    shows the warning of a plan near its limit and of a full account with its reset.
    From 90 % the status line shows the limit (`5-hour 92%`) with the warning glyph
    while a turn runs, and the path gives way first (`shrink-[1000]`). Settings ›
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
43. Q-Quarantine (gallery "Q-Quarantine" and "AU-Auto"): with the reviewer off, the
    waiting tool card, the strip and the sheet show the question and say "answer on
    <machine>" in place of the buttons. The sheet fits its rows. After the answer in
    the terminal, the card says "kept" or "left in quarantine", and the sheet adds where
    it came from.
44. The web end line (no board; a real turn with web search on the desktop): the end
    line counts the searches (`2 searches`), the sources over it fit on one line each,
    and with `web.search = "cached"` the end line says `, stored copy` and one quiet
    line says what gives the live web.

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
