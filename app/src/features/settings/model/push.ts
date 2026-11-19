// Push notices of efrd on the phone (spec 7.2, ADR 0028). efrd sends a notice through the
// Expo push service when something needs you and you are not at the terminal. The phone
// gives each machine its Expo push token with `device.push_set`; the lease of the client
// (lib/net/client.ts) tells efrd when the app is on screen.
//
// - The switch "Notifications" of Settings is off by default. On: the app asks iOS for
//   alert and sound. The switch moves only after iOS allowed it, so it never jumps back.
//   Then the app gets its Expo push token and gives it to each machine.
// - At each connection to an efrd that names the capability `push`, the app sends what
//   that efrd must hold: the token while the switch is on and iOS allows the notices, else
//   no token. So a machine that was offline when the switch went off, or when iOS took
//   the permission away, stops its pushes at its next connection.
// - A token that the app could not get (no network: the call goes to Expo) comes at the
//   next connection. A new APNs token gives a new Expo token, and each machine gets it.
// - A machine whose efrd names no `push` (before 0.0.13, or `presence.push` off) shows
//   "Not available on <machine>" in Settings, from the hello of its last connection in
//   this run.
// - Before the app forgets a machine, it sends no token, so that machine stops at once.
//
// The token stays in this closure: it is a secret (whoever has it can send a notice to the
// phone), so it is in no store, no log and no error. Pure: the effects come in as
// parameters.

import { createStore, type Readable } from "@tanstack/store";

import type { MethodParams } from "@/lib/protocol/generated";
import type { MachineId } from "@/features/machines/model/machine-ref";
import type { Runtime } from "@/features/machines/model/runtime";
import type { PreferencesStore } from "./preferences";

/** What iOS allows the app: the answer of the permission question, or no question yet. */
export type PushPermission = "granted" | "denied" | "undetermined";

/** The native side of push: expo-notifications, or a fake in the tests. */
export interface PushNative {
  permission(): Promise<PushPermission>;
  /** Asks iOS for alert and sound. iOS shows its question only the first time. */
  requestPermission(): Promise<PushPermission>;
  /** The Expo push token of the app. It is a request to Expo, so it needs the network. */
  expoToken(): Promise<string>;
  /**
   * Calls `listener` when iOS gives the app an APNs token other than the one of the last
   * Expo token. Returns its stop.
   */
  followTokens(listener: () => void): () => void;
  /** Removes the delivered notices of conversation `id`. */
  dismiss(conversation: string): Promise<void>;
  /** Opens the iOS settings of the app, where the notices are allowed. */
  openSettings(): void;
}

/** What the row "Notifications" shows. */
export interface PushState {
  /** The switch, as it is saved. */
  readonly on: boolean;
  /** What iOS allows, or undefined until the app read it. */
  readonly permission: PushPermission | undefined;
  /** iOS asks the question: the row takes no tap. */
  readonly asking: boolean;
  /** The machines whose efrd takes no push token, in the order of their connection. */
  readonly unavailable: readonly MachineId[];
}

/** The push service of the app. */
export interface Push {
  readonly store: Readable<PushState>;
  /** The user moved the switch to `on`. */
  setOn(on: boolean): void;
  /** Sends no token to `machine` now, before the app forgets it. */
  release(machine: MachineId): void;
  /**
   * Conversation `id` is on screen: its delivered notices go now, and again each time
   * the app comes back to the foreground on it.
   */
  shown(conversation: string): void;
  /** Conversation `id` left the screen. */
  left(conversation: string): void;
  openSettings(): void;
  /** Stops every listener. */
  dispose(): void;
}

/** What the push service needs. */
export interface PushDeps {
  readonly native: PushNative;
  readonly runtime: Pick<Runtime, "registry" | "machine" | "call" | "onReady">;
  readonly preferences: PreferencesStore;
  /** Calls `listener` with true when the app comes to the foreground. */
  readonly followAppState: (listener: (active: boolean) => void) => () => void;
  readonly onError: (error: unknown) => void;
}

/**
 * True when the ready connection of `machine` takes a push token, false when it does not,
 * and undefined while the machine is not connected.
 */
function takesToken(runtime: PushDeps["runtime"], machine: MachineId): boolean | undefined {
  const client = runtime.machine(machine)?.connection.get().client;
  if (client?.state !== "ready") {
    return undefined;
  }
  return client.hello.capabilities?.push === true;
}

/** Makes the push service and starts it. */
export function createPush(deps: PushDeps): Push {
  const { native, runtime, preferences, onError } = deps;
  const store = createStore<PushState>({
    on: preferences.store.get().notifications,
    permission: undefined,
    asking: false,
    unavailable: [],
  });
  let token: string | undefined;
  let fetching = false;
  let disposed = false;
  /** The conversation on screen, whose notices go at each return to the foreground. */
  let onScreen: string | undefined;

  const update = (change: Partial<PushState>): void => {
    store.setState((state) => ({ ...state, ...change }));
  };

  /** True while the machines must hold the token. */
  const wanted = (): boolean => {
    const { on, permission } = store.state;
    return on && permission === "granted";
  };

  const send = (machine: MachineId, params: MethodParams["device.push_set"]): void => {
    void runtime.call(machine, "device.push_set", params).catch(onError);
  };

  /**
   * Gives `machine` what it must hold, when its efrd takes a token. Before the app knows
   * what iOS allows, it sends nothing: the read of the permission syncs every machine.
   */
  const sync = (machine: MachineId): void => {
    if (store.state.permission === undefined || takesToken(runtime, machine) !== true) {
      return;
    }
    if (!wanted()) {
      send(machine, {});
    } else if (token !== undefined) {
      send(machine, { token });
    } else {
      fetchToken();
    }
  };

  const syncAll = (): void => {
    for (const machine of runtime.registry.get().machines) {
      sync(machine.id);
    }
  };

  /** Gets the Expo token once at a time, then gives it to each machine. */
  const fetchToken = (): void => {
    if (fetching) {
      return;
    }
    fetching = true;
    void (async () => {
      try {
        token = await native.expoToken();
        if (!disposed) {
          syncAll();
        }
      } catch (error) {
        onError(error);
      } finally {
        fetching = false;
      }
    })();
  };

  /** Lets iOS ask, then moves the switch to its answer. */
  const ask = async (): Promise<void> => {
    update({ asking: true });
    try {
      const permission = await native.requestPermission();
      const allowed = permission === "granted";
      preferences.setNotifications(allowed);
      update({ asking: false, permission, on: allowed });
      syncAll();
    } catch (error) {
      update({ asking: false });
      onError(error);
    }
  };

  /** Reads what iOS allows; a change gives each machine what it must hold now. */
  const readPermission = async (): Promise<void> => {
    const permission = await native.permission();
    if (permission !== store.state.permission) {
      update({ permission });
      syncAll();
    }
  };

  const stops = [
    runtime.onReady((machine) => {
      const takes = takesToken(runtime, machine) === true;
      const { unavailable } = store.state;
      if (takes === unavailable.includes(machine)) {
        update({
          unavailable: takes
            ? unavailable.filter((id) => id !== machine)
            : [...unavailable, machine],
        });
      }
      sync(machine);
    }),
    native.followTokens(() => {
      token = undefined;
      if (wanted()) {
        fetchToken();
      }
    }),
    deps.followAppState((active) => {
      if (active) {
        readPermission().catch(onError);
        if (onScreen !== undefined) {
          native.dismiss(onScreen).catch(onError);
        }
      }
    }),
  ];
  readPermission().catch(onError);

  return {
    store,
    setOn(on: boolean): void {
      if (!on) {
        preferences.setNotifications(false);
        update({ on: false });
        syncAll();
        return;
      }
      if (!store.state.asking) {
        void ask();
      }
    },
    release(machine: MachineId): void {
      if (takesToken(runtime, machine) === true) {
        send(machine, {});
      }
    },
    shown(conversation: string): void {
      onScreen = conversation;
      native.dismiss(conversation).catch(onError);
    },
    left(conversation: string): void {
      if (onScreen === conversation) {
        onScreen = undefined;
      }
    },
    openSettings(): void {
      native.openSettings();
    },
    dispose(): void {
      disposed = true;
      for (const stop of stops.splice(0)) {
        stop();
      }
    },
  };
}
