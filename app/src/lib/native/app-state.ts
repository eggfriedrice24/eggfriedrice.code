// Tells the app when it goes to the background and comes back, from React Native's
// AppState. The runtime of the machines hears it once and tells each machine's client,
// which opens a new socket after 10 s or more away. The focus manager of the queries can
// use it too.
//
// iOS also reports "inactive", for example while the Control Center is open. The app stays
// on screen then, so this adapter reports nothing for it.

import { AppState, type AppStateStatus } from "react-native";

/**
 * Calls `listener` with true when the app comes to the foreground and with false when it
 * goes to the background. Returns the function that stops it.
 */
export function followAppState(listener: (active: boolean) => void): () => void {
  const changed = (status: AppStateStatus): void => {
    if (status === "active") {
      listener(true);
    } else if (status === "background") {
      listener(false);
    }
  };
  const subscription = AppState.addEventListener("change", changed);
  return () => subscription.remove();
}

/** The phase of the app, for the Face ID unlock: it needs "inactive" too (the cover). */
export type AppPhase = "active" | "inactive" | "background";

/** `status` as a phase; "unknown" and an empty status read as "inactive". */
function phaseOf(status: string | null | undefined): AppPhase {
  return status === "active" || status === "background" ? status : "inactive";
}

/** The phase of the app now. */
export function appPhase(): AppPhase {
  return phaseOf(AppState.currentState);
}

/**
 * Calls `listener` with each phase of the app, "inactive" too (the app switcher, the
 * Control Center, a prompt of iOS). Only the Face ID unlock listens to it: the machines,
 * the queries and the drafts follow `followAppState`. Returns the function that stops it.
 */
export function followAppPhase(listener: (phase: AppPhase) => void): () => void {
  const subscription = AppState.addEventListener("change", (status) => listener(phaseOf(status)));
  return () => subscription.remove();
}
