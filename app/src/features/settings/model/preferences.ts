// The preferences of the app, kept in the key-value store:
// - `haptics`: the Haptics switch of Settings. Every haptic follows it (plan 5.2 item
//   8.16); src/lib/native/haptics.ts reads it.
// - `sendMode`: the last choice of the Steer/Queue menu of the composer.
// - `lastMachine`: the daemon id of the machine of the blank root, so the app opens on
//   the machine that the user used last.
//
// The store reads the saved value once, when it is made, so the first frame shows it.
// Each change writes the whole value at once; the writes run in order.
//
// Pure TypeScript on TanStack Store.

import { createStore, type Readable } from "@tanstack/store";

import type { DaemonId } from "@/lib/protocol/generated";
import type { KeyValueStorage } from "@/lib/store/key-value";
import { savedPreferencesSchema } from "./preferences-schemas";

/** The key-value key of the preferences. */
export const PREFERENCES_KEY = "efr.preferences";

/** What the Send button of a running turn does: steer the turn, or queue a prompt. */
export type SendMode = "steer" | "queue";

/** The preferences. */
export interface Preferences {
  readonly haptics: boolean;
  readonly sendMode: SendMode;
  readonly lastMachine: DaemonId | null;
}

/** The preferences of a new install. */
export const DEFAULT_PREFERENCES: Preferences = {
  haptics: true,
  sendMode: "steer",
  lastMachine: null,
};

/** What the preferences need from the app. */
export interface PreferencesDeps {
  readonly storage: KeyValueStorage;
  /** Called when the read or a write fails; the preferences go on in memory. */
  readonly onStorageError?: (error: unknown) => void;
}

/** The preferences store. */
export interface PreferencesStore {
  readonly store: Readable<Preferences>;
  setHaptics(on: boolean): void;
  setSendMode(mode: SendMode): void;
  setLastMachine(machine: DaemonId | null): void;
  /** The user forgot a machine: it is no longer the last one. */
  forgetMachine(machine: DaemonId): void;
  /** Resolves when every write so far is done. */
  settled(): Promise<void>;
}

/** The saved preferences, or the defaults when there are none or they are broken. */
export function parsePreferences(json: string | null): Preferences {
  if (json === null) {
    return DEFAULT_PREFERENCES;
  }
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    return DEFAULT_PREFERENCES;
  }
  const parsed = savedPreferencesSchema.safeParse(value);
  if (!parsed.success) {
    return DEFAULT_PREFERENCES;
  }
  const preferences: Preferences = parsed.data;
  return preferences;
}

/** Makes the preferences store and reads the saved value. */
export function createPreferences(deps: PreferencesDeps): PreferencesStore {
  let initial = DEFAULT_PREFERENCES;
  try {
    initial = parsePreferences(deps.storage.getItemSync(PREFERENCES_KEY));
  } catch (error) {
    deps.onStorageError?.(error);
  }
  const store = createStore<Preferences>(initial);
  let writes: Promise<void> = Promise.resolve();

  const update = (next: Preferences): void => {
    const now = store.state;
    if (
      next.haptics === now.haptics &&
      next.sendMode === now.sendMode &&
      next.lastMachine === now.lastMachine
    ) {
      return;
    }
    store.setState(() => next);
    const json = JSON.stringify(next);
    writes = writes
      .then(() => deps.storage.setItem(PREFERENCES_KEY, json))
      .catch((error: unknown) => deps.onStorageError?.(error));
  };

  return {
    store,
    setHaptics(on: boolean): void {
      update({ ...store.state, haptics: on });
    },
    setSendMode(mode: SendMode): void {
      update({ ...store.state, sendMode: mode });
    },
    setLastMachine(machine: DaemonId | null): void {
      update({ ...store.state, lastMachine: machine });
    },
    forgetMachine(machine: DaemonId): void {
      if (store.state.lastMachine === machine) {
        update({ ...store.state, lastMachine: null });
      }
    },
    settled(): Promise<void> {
      return writes;
    },
  };
}
