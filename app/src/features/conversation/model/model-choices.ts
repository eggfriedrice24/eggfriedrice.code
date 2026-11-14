// The model, the account and the effort that the user chose in the model sheet for the
// next messages of a composer (S-ModelSheet): one choice per draft key, so a choice
// sticks to its conversation (or the blank root of a machine) until the user changes it.
// No choice: efrd runs the model of the conversation, else the machine's default.
//
// The store also keeps the draft keys where the user said "Don't ask again here": a
// switch that compacts asks no more in that conversation.
//
// - The store changes at once; the choices go to the key-value store as one JSON value
//   right after each change (a choice changes seldom).
// - The store reads the saved choices once, when it is made.
// - A forgotten machine loses its choices.
//
// Pure TypeScript on TanStack Store.

import { createStore, type Readable } from "@tanstack/store";
import type { DaemonId } from "@/lib/protocol/generated";
import type { KeyValueStorage } from "@/lib/store/key-value";
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

/** The choices by draft key. */
export interface ModelChoicesState {
  readonly choices: ReadonlyMap<string, ModelChoice>;
  /** The draft keys where a costly switch asks no more. */
  readonly quiet: ReadonlySet<string>;
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
  /** Removes every choice of a forgotten machine. */
  removeMachine(machine: DaemonId): void;
  /** Resolves when every save so far is done. */
  settled(): Promise<void>;
}

const EMPTY: ModelChoicesState = { choices: new Map(), quiet: new Set() };

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
  let json: string | null;
  try {
    json = deps.storage.getItemSync(MODEL_CHOICES_KEY);
  } catch (error) {
    deps.onStorageError?.(error);
    return EMPTY;
  }
  if (json === null) {
    return EMPTY;
  }
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    return EMPTY;
  }
  const parsed = savedModelChoicesSchema.safeParse(value);
  if (!parsed.success) {
    return EMPTY;
  }
  const choices = new Map<string, ModelChoice>();
  for (const [key, choice] of Object.entries(parsed.data.choices)) {
    choices.set(key, cleanChoice(choice));
  }
  return { choices, quiet: new Set(parsed.data.quiet) };
}

/** Makes the choices and reads the saved ones. */
export function createModelChoices(deps: ModelChoicesDeps): ModelChoices {
  const store = createStore<ModelChoicesState>(load(deps));
  let saves: Promise<void> = Promise.resolve();

  const save = (): void => {
    const { choices, quiet } = store.state;
    const json = JSON.stringify({ choices: Object.fromEntries(choices), quiet: [...quiet] });
    const write =
      choices.size === 0 && quiet.size === 0
        ? () => deps.storage.removeItem(MODEL_CHOICES_KEY)
        : () => deps.storage.setItem(MODEL_CHOICES_KEY, json);
    saves = saves.then(write).catch((error: unknown) => deps.onStorageError?.(error));
  };

  const update = (
    change: (choices: Map<string, ModelChoice>, quiet: Set<string>) => boolean,
  ): void => {
    const choices = new Map(store.state.choices);
    const quiet = new Set(store.state.quiet);
    if (change(choices, quiet)) {
      store.setState(() => ({ choices, quiet }));
      save();
    }
  };

  return {
    store,
    choice(key: DraftKey): ModelChoice | undefined {
      return store.state.choices.get(key);
    },
    set(key: DraftKey, choice: ModelChoice | undefined): void {
      update((choices) => {
        if (choice === undefined) {
          return choices.delete(key);
        }
        if (sameChoice(choices.get(key), choice)) {
          return false;
        }
        choices.set(key, cleanChoice(choice));
        return true;
      });
    },
    quiet(key: DraftKey): boolean {
      return store.state.quiet.has(key);
    },
    setQuiet(key: DraftKey, on: boolean): void {
      update((_choices, quiet) => {
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
    removeMachine(machine: DaemonId): void {
      const prefix = `${machine}:`;
      update((choices, quiet) => {
        let removed = false;
        // Deleting the entry of the iteration is safe for a Map and a Set.
        for (const key of choices.keys()) {
          if (key.startsWith(prefix)) {
            choices.delete(key);
            removed = true;
          }
        }
        for (const key of quiet) {
          if (key.startsWith(prefix)) {
            quiet.delete(key);
            removed = true;
          }
        }
        return removed;
      });
    },
    settled(): Promise<void> {
      return saves;
    },
  };
}
