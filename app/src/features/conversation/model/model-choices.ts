// The model, the account and the effort that the user chose in the model sheet for the
// next messages of a composer (S-ModelSheet): one choice per draft key, so a choice
// sticks to its conversation (or the blank root of a machine) until the user changes it.
// No choice: efrd runs the model of the conversation, else the machine's default.
//
// The store also keeps:
// - the draft keys where the user said "Don't ask again here": a switch that compacts
//   asks no more in that conversation;
// - the draft keys whose next messages run in auto (S-ComposerR7 8): the mode stays with
//   the conversation on this phone, and a key that is not in the set is cautious;
// - the favourite model of each account of a machine (S-ModelSheet r7 5): one for each
//   account, so a star on another model of the account moves it;
// - the last plan of each machine: the account of the last choice or the last message
//   with an account. A new thread starts on it, with its favourite.
//
// - The store changes at once; everything goes to the key-value store as one JSON value
//   right after each change (a choice changes seldom).
// - The store reads the saved choices once, when it is made.
// - A forgotten machine loses its choices, its modes, its favourites and its plan.
//
// Pure TypeScript on TanStack Store.

import { createStore, type Readable } from "@tanstack/store";
import type { DaemonId } from "@/lib/protocol/generated";
import { readSaved, type KeyValueStorage } from "@/lib/store/key-value";
import type { DraftKey } from "./drafts";
import { savedModelChoicesSchema } from "./model-choices-schemas";

/** The key-value key of the choices. */
export const MODEL_CHOICES_KEY = "efr.model_choices";

/** A model, the account that runs it, and an effort when the model has efforts. */
export interface ModelChoice {
  readonly model: string;
  /** The provider id of the account, such as `openai-subscription.b`. */
  readonly provider?: string;
  readonly effort?: string;
}

/** The mode of the next messages of a composer. The phone never sends full. */
export type PhoneMode = "cautious" | "auto";

/**
 * The mode that the next message of a composer runs with: auto only when the composer is
 * in auto AND the Face ID unlock of the app is on (round 9).
 */
export function effectiveMode(stored: PhoneMode, lockOn: boolean): PhoneMode {
  return stored === "auto" && lockOn ? "auto" : "cautious";
}

/** The choices by draft key. */
export interface ModelChoicesState {
  readonly choices: ReadonlyMap<string, ModelChoice>;
  /** The draft keys where a costly switch asks no more. */
  readonly quiet: ReadonlySet<string>;
  /** The draft keys whose next messages run in auto. */
  readonly auto: ReadonlySet<string>;
  /** The favourite model by `favouriteKey`. */
  readonly favourites: ReadonlyMap<string, string>;
  /** The last plan (a provider id) by machine. */
  readonly plans: ReadonlyMap<string, string>;
}

/** The key of the favourite of `provider` on `machine`; "" for an old daemon's one list. */
export function favouriteKey(machine: DaemonId, provider: string | undefined): string {
  return `${machine}:${provider ?? ""}`;
}

/** The choice of `key`, for `useSelector`. */
export function selectModelChoice(
  key: DraftKey,
): (state: ModelChoicesState) => ModelChoice | undefined {
  return (state) => state.choices.get(key);
}

/** True when a costly switch of `key` asks no more, for `useSelector`. */
export function selectQuiet(key: DraftKey): (state: ModelChoicesState) => boolean {
  return (state) => state.quiet.has(key);
}

/** The mode of the next messages of `key`, for `useSelector`. */
export function selectMode(key: DraftKey): (state: ModelChoicesState) => PhoneMode {
  return (state) => (state.auto.has(key) ? "auto" : "cautious");
}

/** The favourites of every machine, for `useSelector`; read one with `favouriteOf`. */
export function selectFavourites(state: ModelChoicesState): ReadonlyMap<string, string> {
  return state.favourites;
}

/** The last plan of `machine`, for `useSelector`. */
export function selectPlan(machine: DaemonId): (state: ModelChoicesState) => string | undefined {
  return (state) => state.plans.get(machine);
}

/** The favourite of `provider` on `machine` in `favourites`. */
export function favouriteOf(
  favourites: ReadonlyMap<string, string>,
  machine: DaemonId,
  provider: string | undefined,
): string | undefined {
  return favourites.get(favouriteKey(machine, provider));
}

/** True when `a` and `b` name the same model, account and effort. */
export function sameChoice(a: ModelChoice | undefined, b: ModelChoice | undefined): boolean {
  return a?.model === b?.model && a?.provider === b?.provider && a?.effort === b?.effort;
}

/** What the choices need from the app. */
export interface ModelChoicesDeps {
  readonly storage: KeyValueStorage;
  /** Called when the read or a save fails; the choices go on in memory. */
  readonly onStorageError?: (error: unknown) => void;
}

/** The model choices of all composers. */
export interface ModelChoices {
  readonly store: Readable<ModelChoicesState>;
  /** The choice of `key` now. */
  choice(key: DraftKey): ModelChoice | undefined;
  /** Sets the choice of `key`; undefined goes back to what efrd runs. */
  set(key: DraftKey, choice: ModelChoice | undefined): void;
  /** True when a costly switch of `key` asks no more. */
  quiet(key: DraftKey): boolean;
  /** Sets "Don't ask again here" for `key`. */
  setQuiet(key: DraftKey, quiet: boolean): void;
  /** The mode of the next messages of `key`. */
  mode(key: DraftKey): PhoneMode;
  /** Sets the mode of the next messages of `key`. */
  setMode(key: DraftKey, mode: PhoneMode): void;
  /** Sets every composer back to cautious: the Face ID unlock turned off (D3). */
  clearAuto(): void;
  /** Makes `model` the favourite of `provider` on `machine`; undefined removes it. */
  setFavourite(machine: DaemonId, provider: string | undefined, model: string | undefined): void;
  /** Keeps `provider` as the last plan of `machine`. */
  setPlan(machine: DaemonId, provider: string): void;
  /** Removes every choice of a forgotten machine. */
  removeMachine(machine: DaemonId): void;
  /** Resolves when every save so far is done. */
  settled(): Promise<void>;
}

const EMPTY: ModelChoicesState = {
  choices: new Map(),
  quiet: new Set(),
  auto: new Set(),
  favourites: new Map(),
  plans: new Map(),
};

/** A copy of `choice` without the fields that it does not have. */
export function cleanChoice(choice: ModelChoice): ModelChoice {
  const clean: { model: string; provider?: string; effort?: string } = { model: choice.model };
  if (choice.provider !== undefined) {
    clean.provider = choice.provider;
  }
  if (choice.effort !== undefined) {
    clean.effort = choice.effort;
  }
  return clean;
}

/** Reads the saved choices, or none when there are none or they are broken. */
function load(deps: ModelChoicesDeps): ModelChoicesState {
  const saved = readSaved(
    deps.storage,
    MODEL_CHOICES_KEY,
    savedModelChoicesSchema,
    deps.onStorageError,
  );
  if (saved === undefined) {
    return EMPTY;
  }
  const choices = new Map<string, ModelChoice>();
  for (const [key, choice] of Object.entries(saved.choices)) {
    choices.set(key, cleanChoice(choice));
  }
  return {
    choices,
    quiet: new Set(saved.quiet),
    auto: new Set(saved.auto),
    favourites: new Map(Object.entries(saved.favourites)),
    plans: new Map(Object.entries(saved.plans)),
  };
}

/** The parts of the state that a change edits. */
interface Draft {
  readonly choices: Map<string, ModelChoice>;
  readonly quiet: Set<string>;
  readonly auto: Set<string>;
  readonly favourites: Map<string, string>;
  readonly plans: Map<string, string>;
}

/** True when the state holds nothing to save. */
function isEmpty(state: ModelChoicesState): boolean {
  return (
    state.choices.size === 0 &&
    state.quiet.size === 0 &&
    state.auto.size === 0 &&
    state.favourites.size === 0 &&
    state.plans.size === 0
  );
}

/** Removes each key of `keys` that starts with `prefix`; true when one went. */
function removePrefixed(keys: Map<string, unknown> | Set<string>, prefix: string): boolean {
  let removed = false;
  // Deleting the entry of the iteration is safe for a Map and a Set.
  for (const key of keys.keys()) {
    if (key.startsWith(prefix)) {
      keys.delete(key);
      removed = true;
    }
  }
  return removed;
}

/** Keeps `provider` as the plan of `machine` in `draft`; true when it changed. */
function keepPlan(draft: Draft, machine: string, provider: string): boolean {
  if (draft.plans.get(machine) === provider) {
    return false;
  }
  draft.plans.set(machine, provider);
  return true;
}

/** Makes the choices and reads the saved ones. */
export function createModelChoices(deps: ModelChoicesDeps): ModelChoices {
  const store = createStore<ModelChoicesState>(load(deps));
  let saves: Promise<void> = Promise.resolve();

  const save = (): void => {
    const state = store.state;
    const json = JSON.stringify({
      choices: Object.fromEntries(state.choices),
      quiet: [...state.quiet],
      auto: [...state.auto],
      favourites: Object.fromEntries(state.favourites),
      plans: Object.fromEntries(state.plans),
    });
    const write = isEmpty(state)
      ? () => deps.storage.removeItem(MODEL_CHOICES_KEY)
      : () => deps.storage.setItem(MODEL_CHOICES_KEY, json);
    saves = saves.then(write).catch((error: unknown) => deps.onStorageError?.(error));
  };

  // NOTE: a part is copied when the change first reads it. The parts that a change does
  // not read keep their identity, so a selector of one of them (`selectFavourites`) sees
  // no change and its rows do not render again.
  const update = (change: (draft: Draft) => boolean): void => {
    const state = store.state;
    const copies: {
      choices?: Map<string, ModelChoice>;
      quiet?: Set<string>;
      auto?: Set<string>;
      favourites?: Map<string, string>;
      plans?: Map<string, string>;
    } = {};
    const draft: Draft = {
      get choices() {
        return (copies.choices ??= new Map(state.choices));
      },
      get quiet() {
        return (copies.quiet ??= new Set(state.quiet));
      },
      get auto() {
        return (copies.auto ??= new Set(state.auto));
      },
      get favourites() {
        return (copies.favourites ??= new Map(state.favourites));
      },
      get plans() {
        return (copies.plans ??= new Map(state.plans));
      },
    };
    if (change(draft)) {
      store.setState(() => ({
        choices: copies.choices ?? state.choices,
        quiet: copies.quiet ?? state.quiet,
        auto: copies.auto ?? state.auto,
        favourites: copies.favourites ?? state.favourites,
        plans: copies.plans ?? state.plans,
      }));
      save();
    }
  };

  return {
    store,
    choice(key: DraftKey): ModelChoice | undefined {
      return store.state.choices.get(key);
    },
    set(key: DraftKey, choice: ModelChoice | undefined): void {
      update((draft) => {
        if (choice === undefined) {
          return draft.choices.delete(key);
        }
        // NOTE: a choice of an account makes it the plan of the next new thread.
        const machine = key.slice(0, key.indexOf(":"));
        const plan = choice.provider !== undefined && keepPlan(draft, machine, choice.provider);
        if (sameChoice(draft.choices.get(key), choice)) {
          return plan;
        }
        draft.choices.set(key, cleanChoice(choice));
        return true;
      });
    },
    quiet(key: DraftKey): boolean {
      return store.state.quiet.has(key);
    },
    setQuiet(key: DraftKey, on: boolean): void {
      update(({ quiet }) => {
        if (quiet.has(key) === on) {
          return false;
        }
        if (on) {
          quiet.add(key);
        } else {
          quiet.delete(key);
        }
        return true;
      });
    },
    mode(key: DraftKey): PhoneMode {
      return store.state.auto.has(key) ? "auto" : "cautious";
    },
    setMode(key: DraftKey, mode: PhoneMode): void {
      update(({ auto }) => {
        const on = mode === "auto";
        if (auto.has(key) === on) {
          return false;
        }
        if (on) {
          auto.add(key);
        } else {
          auto.delete(key);
        }
        return true;
      });
    },
    clearAuto(): void {
      update(({ auto }) => {
        if (auto.size === 0) {
          return false;
        }
        auto.clear();
        return true;
      });
    },
    setFavourite(machine: DaemonId, provider: string | undefined, model: string | undefined): void {
      const key = favouriteKey(machine, provider);
      update(({ favourites }) => {
        if (model === undefined) {
          return favourites.delete(key);
        }
        if (favourites.get(key) === model) {
          return false;
        }
        favourites.set(key, model);
        return true;
      });
    },
    setPlan(machine: DaemonId, provider: string): void {
      update((draft) => keepPlan(draft, machine, provider));
    },
    removeMachine(machine: DaemonId): void {
      const prefix = `${machine}:`;
      update((draft) => {
        const removed = [
          removePrefixed(draft.choices, prefix),
          removePrefixed(draft.quiet, prefix),
          removePrefixed(draft.auto, prefix),
          removePrefixed(draft.favourites, prefix),
          draft.plans.delete(machine),
        ];
        return removed.includes(true);
      });
    },
    settled(): Promise<void> {
      return saves;
    },
  };
}
