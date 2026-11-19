// The preferences of the app, kept in the key-value store:
// - `haptics`: the Haptics switch of Settings. Every haptic follows it (plan 5.2 item
//   8.16); src/lib/native/haptics.ts reads it.
// - `sendMode`: the last choice of the Steer/Queue menu of the composer.
// - `lastMachine`: the daemon id of the machine of the blank root, so the app opens on
//   the machine that the user used last.
// - `askCostlySwitch`: the switch "Ask before a costly model switch" of Settings (on by
//   default, for every machine). On: a pick in the model sheet waits for "Use" in its
//   header ("Switch" for a switch that compacts). Off: a tap on a row of the sheet
//   switches at once.
// - `showLegacyModels`: the switch "Legacy models" of the filter menu of the model sheet
//   (off by default, for every machine). On: the sheet shows the models that efrd marks
//   legacy, in a group at the end of each account.
// - `faceIdUnlock`: the switch "Face ID unlock" of Settings (off by default). The app lock
//   (features/navigation/model/app-lock.ts) reads it at the start and saves it; only it
//   calls `setFaceIdUnlock`. `faceIdUnlockNotice` is "no_passcode" after efr turned the
//   switch off because the iPhone has no passcode (D5).
// - `notifications`: the switch "Notifications" of Settings (off by default). Only the
//   push service (model/push.ts) calls `setNotifications`, after iOS allowed the notices.
//
// The store reads the saved value once, when it is made, so the first frame shows it.
// Each change writes the whole value at once; the writes run in order.
//
// Pure TypeScript on TanStack Store.

import { createStore, type Readable } from "@tanstack/store";

import type { DaemonId } from "@/lib/protocol/generated";
import { readSaved, type KeyValueStorage } from "@/lib/store/key-value";
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
  readonly askCostlySwitch: boolean;
  readonly showLegacyModels: boolean;
  readonly faceIdUnlock: boolean;
  readonly faceIdUnlockNotice: FaceIdUnlockNotice;
  readonly notifications: boolean;
}

/** Why efr turned the Face ID unlock off, or none. */
export type FaceIdUnlockNotice = "no_passcode" | null;

/** The preferences of a new install. */
export const DEFAULT_PREFERENCES: Preferences = {
  haptics: true,
  sendMode: "steer",
  lastMachine: null,
  askCostlySwitch: true,
  showLegacyModels: false,
  faceIdUnlock: false,
  faceIdUnlockNotice: null,
  notifications: false,
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
  setAskCostlySwitch(on: boolean): void;
  setShowLegacyModels(on: boolean): void;
  /** Saves the switch of the Face ID unlock and its notice. Only the app lock calls it. */
  setFaceIdUnlock(on: boolean, notice: FaceIdUnlockNotice): void;
  /** Saves the switch "Notifications". Only the push service calls it. */
  setNotifications(on: boolean): void;
  /** The user forgot a machine: it is no longer the last one. */
  forgetMachine(machine: DaemonId): void;
  /** Resolves when every write so far is done. */
  settled(): Promise<void>;
}

/**
 * Makes the preferences store and reads the saved value, or the defaults when there is
 * none or it is broken.
 */
export function createPreferences(deps: PreferencesDeps): PreferencesStore {
  const saved: Preferences | undefined = readSaved(
    deps.storage,
    PREFERENCES_KEY,
    savedPreferencesSchema,
    deps.onStorageError,
  );
  const store = createStore<Preferences>(saved ?? DEFAULT_PREFERENCES);
  let writes: Promise<void> = Promise.resolve();

  const update = (next: Preferences): void => {
    const now = store.state;
    if (
      next.haptics === now.haptics &&
      next.sendMode === now.sendMode &&
      next.lastMachine === now.lastMachine &&
      next.askCostlySwitch === now.askCostlySwitch &&
      next.showLegacyModels === now.showLegacyModels &&
      next.faceIdUnlock === now.faceIdUnlock &&
      next.faceIdUnlockNotice === now.faceIdUnlockNotice &&
      next.notifications === now.notifications
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
    setAskCostlySwitch(on: boolean): void {
      update({ ...store.state, askCostlySwitch: on });
    },
    setShowLegacyModels(on: boolean): void {
      update({ ...store.state, showLegacyModels: on });
    },
    setFaceIdUnlock(on: boolean, notice: FaceIdUnlockNotice): void {
      update({ ...store.state, faceIdUnlock: on, faceIdUnlockNotice: notice });
    },
    setNotifications(on: boolean): void {
      update({ ...store.state, notifications: on });
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
