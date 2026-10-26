// Tells Query when the app is in the foreground and when the phone has a network
// (plan section 2.2). The platform adapters (AppState and NetInfo, in src/lib/native)
// are passed in, so this file stays pure and runs in Node.
//
// - Focus: back in the foreground, Query fetches the stale queries that have observers.
// - Online: NetInfo tells whether the phone has a network, not whether efrd answers.
//   With `networkMode: "always"` nothing pauses on it; a network that comes back only
//   makes Query fetch again. The reachability of each machine stays in its supervisor.

import { focusManager, onlineManager } from "@tanstack/react-query";

/** Calls `listener` on each change. Returns the function that stops it. */
export type Subscribe<T> = (listener: (value: T) => void) => () => void;

/** The platform sources of focus and network. */
export interface QueryManagerSources {
  /** True when the app becomes active, false when it goes to the background. */
  readonly appActive: Subscribe<boolean>;
  /** True when the phone has a network path, false when it has none. */
  readonly networkOnline: Subscribe<boolean>;
}

/** The parts of the two managers of Query that this file uses. */
export interface QueryManagers {
  readonly focus: Pick<typeof focusManager, "setEventListener">;
  readonly online: Pick<typeof onlineManager, "setEventListener">;
}

const QUERY_MANAGERS: QueryManagers = { focus: focusManager, online: onlineManager };

/**
 * Feeds the managers of Query from `sources`. Returns the function that stops it. The
 * managers call the stop functions of the sources when they get a new listener.
 */
export function wireQueryManagers(
  sources: QueryManagerSources,
  managers: QueryManagers = QUERY_MANAGERS,
): () => void {
  managers.focus.setEventListener((setFocused) =>
    sources.appActive((active) => {
      setFocused(active);
    }),
  );
  managers.online.setEventListener((setOnline) =>
    sources.networkOnline((online) => {
      setOnline(online);
    }),
  );
  return () => {
    managers.focus.setEventListener(() => undefined);
    managers.online.setEventListener(() => undefined);
  };
}

/**
 * The focus of an AppState status: `active` is true, `background` is false. Another
 * status (`inactive`, such as while Control Center is open) changes nothing: undefined.
 */
export function focusOfAppState(status: string): boolean | undefined {
  if (status === "active") {
    return true;
  }
  if (status === "background") {
    return false;
  }
  return undefined;
}

/**
 * The online state of a NetInfo state. `isConnected` is null before NetInfo knows; the
 * app then counts the phone as online, so that nothing waits for the first report.
 */
export function onlineOfNetInfo(state: { readonly isConnected: boolean | null }): boolean {
  return state.isConnected !== false;
}
