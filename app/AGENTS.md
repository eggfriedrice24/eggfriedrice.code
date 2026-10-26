# Agent instructions for the phone app

Rules for every coding agent that works in `app/`. They add to the root `AGENTS.md`,
`ARCHITECTURE.md` and `CONVENTIONS.md`, and do not repeat them. `README.md` in this
directory tells what the app is, how to build it and how to run the gates. Read it first.

The tests and the lint rules enforce most of these rules. When a rule and the code do
not agree, the code is wrong or the rule is old: tell the user, and do not change both
in silence.

## Safety

- The rules of the root `AGENTS.md` apply here too. Never reach the user's real efrd,
  never run `efr`, `efrd` or `efr-sbx`, and never talk to the real tailscaled.
- A test never reaches the network or a real host. It uses the fake sockets, the fake
  clocks and the fake stores of the `testing.ts` files.
- Run every `pnpm`, `node` and `expo` command at `nice -n 19 ionice -c 3`.
- Add a package only from the approved list of the user. Ask before any other package.
  Only the packages in `allowBuilds` of `pnpm-workspace.yaml` can run an install script.

## Layout

All paths are under `app/src/`.

| Path               | What it holds                                                                                |
| ------------------ | -------------------------------------------------------------------------------------------- |
| `app/`             | The routes of expo-router. Each one is thin: it reads its params and renders one screen.     |
| `features/<name>/` | One feature: `machines`, `pairing`, `conversation`, `conversations`, `navigation`, and more. |
| `components/ui/`   | The parts of the design that every feature uses: text, buttons, menus, materials, loaders.   |
| `lib/<name>/`      | Code that knows no feature: `protocol`, `stream`, `net`, `store`, `query`, `native`.         |
| `theme/`           | The palette, the colour roles, the type scale, the metrics, the materials and the theme CSS. |

The shape of a feature:

```text
features/<feature>/
  screens/<name>-screen.tsx   one screen per route; the route file only renders it
  components/<name>.tsx       used only by this feature
  model/<name>.ts             pure logic: reducers, stores, formatters; the test is beside it
  model/<name>-schemas.ts     the zod schemas of the feature
  native/<name>.ts            the platform adapters of the feature
  use-<name>.ts               hooks
  <feature>-queries.ts        the key factory and the query and mutation option factories
```

There is no `api/` folder in a feature. The `*-queries.ts` file holds the key factory and
the option factories.

## Layers and imports

`test/dependency-graph.test.ts` checks these rules over every file of `src/`.

- `lib/` and `theme/` import nothing from `features/`, `components/` or `app/`.
- `components/` imports only `lib/`, `theme/`, other components and packages.
- A feature imports `lib/`, `theme/`, `components/` and, from another feature, only its
  `model/**`, its root `*-queries.ts` or its root `use-*.ts`. It never imports the
  `components/` or the `screens/` of another feature.
- A route imports only `features/*/screens/**` and `features/*/model/**`. The root
  `_layout.tsx` can import more, because it puts the providers together.
- Use `@/` for an import into another unit. Use a relative path inside a unit. A unit is
  `features/<name>`, `lib/<name>`, `components/<name>`, `theme` or `app`.
- No barrel files: no `index.ts` outside `src/app/`.
- No import cycle.
- Write `import type` for a type (`verbatimModuleSyntax` is on).

Pure code runs in Node under Vitest. So a `.ts` file outside `use-*.ts`, `native/**`,
the tests and `testing.ts` must not import `react-native`, `react-native-*`, `expo`,
`expo-*`, `@expo/*`, `@react-native-*/**`, `@shopify/**` or `@legendapp/**`. A type
import is allowed. oxlint enforces it.

## Files and names

- File names are kebab-case (`unicorn/filename-case`). Use these suffixes: `-screen.tsx`,
  `-queries.ts`, `-schemas.ts`, `-format.ts`, `-store.ts`, `use-*.ts`, `*.test.ts` and
  `*.test.tsx`. Route params are kebab-case too.
- Functions, factories and schemas are camelCase (`recentConversationsQueryOptions`,
  `conversationKeys`, `storedRecordsSchema`). Components and types are PascalCase.
  Constants are SCREAMING_CASE.
- A function named `use*` must call a hook. If it calls none, give it another name.

## TypeScript

- `exactOptionalPropertyTypes` is on. Build an object with a local variable and `if`
  statements. Do not use a conditional spread such as `...(x ? { a } : {})`.
- Fire and forget with `void promise.catch(() => undefined)`, or report the error.
- `lib` is `ES2022`, and oxlint stops `toSorted`, `toReversed`, `toSpliced` and
  `Intl.Segmenter`, because Hermes can lack them. Sort or reverse a copy.
- Do not use `performance` in app code. Use the time of the animation frame or the
  `Clock` of `lib/net/clock.ts`.

## React

- The React Compiler is on (`experiments.reactCompiler` in `app.json`). Do not memoize
  by hand: oxlint stops `memo`, `useMemo` and `useCallback`.
- `test/react-compiler-coverage.test.ts` compiles each `.tsx` file and each `use-*.ts`
  hook. A file that makes the compiler skip a function fails the test. `KNOWN_BAILOUTS`
  is empty, and it must stay empty. Do not add a file to it: change the code.
- The oxlint rules of the compiler are errors: `purity`, `immutability`,
  `set-state-in-render`, `set-state-in-effect`, `static-components`, `unsupported-syntax`,
  `incompatible-library` and `no-deriving-state-in-effects`.
- Effects budget: `test/effect-budget.test.ts` counts `useEffect` and `useLayoutEffect`
  per file against `scripts/effect-budget.json`. A file that is not in the budget may
  have none. A budget only goes down. Put lifecycle work (sockets, AppState, NetInfo,
  the outbox, persistence) in the runtime outside React.
- Keep state where it belongs:
  - server data (lists, machine status) only in TanStack Query;
  - stream data (the transcript) only in the stream store of the conversation;
  - drafts and row UI state (a thought or a tool block that is open) in their stores,
    never in `useState`, because the list recycles row instances;
  - values that you can derive, in render.
- Use `useState` only for short local UI state that no other component reads.
- Read a store with `useSelector(store, select, { compare })` of
  `@tanstack/react-store`, with a narrow selector. Do not use `useStore`.
- Use the modern hooks where they fit: `use()` for a context and the root promise,
  `useEffectEvent` for a callback that a hook gives to the runtime, `useTransition` and
  `useDeferredValue` for slow updates, and `Activity` to keep the state of a hidden pane.
  Never put `Activity` around a stack screen.
- Each route file exports `ErrorBoundary`.
- No Reanimated loops: oxlint stops `withRepeat`. A loop (the cursor blink, the
  BrailleCell) uses RN `Animated.loop` with `useNativeDriver: true`, so no JS runs on each
  frame. Reanimated is only for the keyboard and the fade of the streaming tail.
- Each animation has a still frame for Reduce Motion (`lib/native/reduce-motion.ts`).

## Components and styles

- A component is a named `export function X(props: XProps)`. `interface XProps` has
  `readonly` fields. A route file is the only default export.
- Style with uniwind `className` and the classes of the theme. Join classes with `cn()`
  of `components/ui/cn.ts`. A variant is a string union mapped to a class string. Use
  `StyleSheet` only where `className` cannot apply, such as an `Animated` value.
- The theme classes come from `theme/theme.css`, which `pnpm gen:theme` writes from
  `roles.ts`, `type.ts` and `metrics.ts`. Do not edit `theme.css` by hand.
  - Colour: `bg-<role>`, `text-<role>` and `border-<role>`, with the role in kebab-case
    (`text-text-muted`). The default colours of Tailwind do not exist.
  - Text: one `text-<style>` class per style of `TYPE`, such as `text-body` or
    `text-mono-xs`. Never use `font-mono`.
  - Space: the unit is 1 pt (`p-12` is 12 pt), plus named distances such as `gutter`,
    `item`, `inset`, `hit` and `strip`.
  - Corners: `rounded-box`, `rounded-inner`, `rounded-control`, `rounded-field`,
    `rounded-group` and `rounded-full`. Hairlines: `border-hairline`,
    `border-t-hairline` and `border-b-hairline`.
- A prop that cannot take a class reads `roles` (`theme/roles.ts`), `TYPE`,
  `MONO_FONT` or `SEMIBOLD` (`theme/type.ts`), the values of `theme/metrics.ts`, or
  `MATERIALS` and `OPACITY` (`theme/materials.ts`).
- Never write a raw colour value in a component. Never invent a colour or a role. Each
  role names an entry of `theme/palette.ts`, which is a copy of the palette of the
  eggfriedrice design system.
- A screen has its data hooks at the top, then an early return for a pending state (the
  BrailleCell) and for an error, then the values that it derives, then JSX from
  `components/ui`.
- Use `components/ui` for the parts of the design: `MonoText`, `BodyText`,
  `BrailleCell`, `BlockCursor`, `Material`, `GlassButton`, `Hairline`, `SymbolIcon`,
  `MenuView`, `PrimaryButton`, `BorderedButton`, `PlainButton`, `CopyButton`,
  `KeyValueBox`, `InsetGroup`, `SectionHeader` and `Dot`.
- `MenuView` never gets an empty image: react-native-menu 2.0.0 then logs a fault.
- efr is not a TUI. The app uses native iOS parts: the native stack, UIMenu, action
  sheets and form sheets.

## Data

- One key factory per feature, keyed by machine, in `<feature>-queries.ts`. The shared
  keys are in `lib/query/keys.ts`.
- Write `queryOptions`, `infiniteQueryOptions` and `mutationOptions` factories that take
  the runtime (a `MachineCaller`). A screen spreads a factory and adds its callbacks:
  `useMutation({ ...answerApprovalMutationOptions(runtime, ref), onSuccess })`.
- Each mutation on a conversation has the scope of `conversationScope(ref)`
  (`lib/query/keys.ts`), so its calls reach efrd in the order of the taps. Stop
  (`turn.interrupt`) has no scope, so it never waits behind another call.
- Persist a query only with `meta: { persist: true }`.
- A prompt goes through the outbox (`features/conversation/model/outbox.ts`). The
  pending row shows at once, and the drain of the machine sends it when the machine is
  ready. `prompt.send` is idempotent by `command_id`.
- Get the services from `@/features/navigation/use-app-services`: `useAppServices`,
  `useRuntime`, `useOutbox`, `useDrafts` and `usePreferences`.
- Open a conversation only with `useOpenConversation()`. It runs the history helper
  (`features/navigation/model/history.ts`): at most 8 conversations above the root, and
  a conversation that is in the stack opens where it is. Do not dismiss a sheet before
  you call it.

## zod

- Only `src/lib/protocol/generated-zod.ts` and `**/model/*-schemas.ts` call `z.`. The
  dependency graph test fails on a call in any other file.
- `generated.ts` and `generated-zod.ts` come from `docs/protocol.schema.json`. Never edit
  them by hand. Run `pnpm gen:protocol`.
- Use `.exactOptional()` for an optional field, never `.optional()`. Use `z.guid()` for an
  id and `z.iso.datetime({ offset: true })` for a time.
- An unknown kind, or a known kind that does not parse, becomes `other` and the cursor
  goes on. Do not make the stream retry for it.

## Platform

- Every haptic goes through `lib/native/haptics.ts`. It reads the Haptics switch.
- Every copy goes through `lib/native/clipboard.ts` (expo-clipboard).
- A link opens only through `lib/native/links.ts`: the app asks first, then opens it in
  SFSafariViewController.
- Offsets of text count bytes of UTF-8, never UTF-16 units (`lib/stream/utf8.ts`).

## Tests

- Vitest runs the `*.test.ts` files in Node: the pure code, the stores, the schemas over
  every fixture of efr-protocol, the replay of the recordings and the rules of this
  file.
- jest-expo runs the `*.test.tsx` files: the components, the screens and the routes.
  - Import `describe`, `it`, `expect` and `jest` from `@jest/globals`.
  - `await` each `render` and `fireEvent` of React Native Testing Library.
  - Put each router call in `await act(async () => { ... })`.
  - A test that renders `AppProviders` mocks `react-native-keyboard-controller` with its
    `/jest` mock. A test that renders transcript rows mocks `./fade-in`. A test that
    renders the list also mocks `@shopify/flash-list`.
  - A test with a Query mutation sets `gcTime: Infinity`, and a direct notify scheduler
    where it waits on Query.
  - Keep `jest-matchers.d.ts`: it gives the matchers of both testing libraries their
    types.
- Use the fakes of the `testing.ts` files. `@/features/navigation/model/testing` has
  `testServices(records)`, which makes the real services of the app on fake sockets, a
  fake clock and memory stores.
- Tests are deterministic. A fake clock moves only when the test moves it. Do not write
  a test that waits on real time, and do not stress the machine with busy loops.
- A fact that only an iPhone can show goes on the device checklist of `README.md`. Do
  not write a test that pretends to check it.
- After you add a fixture, delete `node_modules/.vitest-cache`. Vitest caches the
  result of `import.meta.glob`, so a new fixture can stay hidden.

## Gates

Run `just app-check` from the repository root before each commit. It must pass with no
warning. `README.md`, section "Work on Linux", lists its steps.

When you change efr-protocol, run `cargo xtask protocol-schema`, then
`pnpm gen:protocol`, and commit the schema and the generated files together.

## Writing

- Comments, docs and the texts of the app follow ASD-STE100: short sentences, active
  voice, simple words. A comment says why, not what.
- The texts of the screens come from the design. Write a new text in the same voice.
- Never use an em dash or an en dash. Use "-".
- Add each change that a user can see to `[Unreleased]` in `CHANGELOG.md`.
