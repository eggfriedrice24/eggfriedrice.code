// Tells the client when the app goes to the background and comes back, from React
// Native's AppState. The client then opens a new socket after 10 s or more away.
//
// No package of the app reports network path changes (such as Wi-Fi to cellular); a
// later one can call `client.networkChanged()`.

import { AppState, type AppStateStatus } from "react-native";

import type { Client } from "../client";

/** Follows AppState for `client`. Returns the function that stops it. */
export function followAppState(client: Client): () => void {
  const changed = (status: AppStateStatus): void => {
    if (status === "active") {
      client.setForeground(true);
    } else if (status === "background") {
      client.setForeground(false);
    }
  };
  const subscription = AppState.addEventListener("change", changed);
  return () => subscription.remove();
}
