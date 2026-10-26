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
