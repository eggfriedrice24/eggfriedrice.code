// The time now, to the minute, for the words "seen 13:58" and "seen 2 days ago" of an
// offline machine. A render must not read the clock (the React Compiler rule of purity),
// so the time is a small external store: `useSyncExternalStore` reads it, and a timer
// tells the component when the minute can have changed. The timer runs only while a
// component reads the store.

import { useSyncExternalStore } from "react";

/** How often the store looks at the clock while a component reads it. */
export const MINUTE_CHECK_MS = 15_000;

const MINUTE_MS = 60_000;

function subscribe(listener: () => void): () => void {
  const timer = setInterval(listener, MINUTE_CHECK_MS);
  return () => clearInterval(timer);
}

/** The time now, rounded down to the minute: the same value for the whole minute. */
function snapshot(): number {
  return Math.floor(Date.now() / MINUTE_MS) * MINUTE_MS;
}

/** The time now in milliseconds, rounded down to the minute. */
export function useMinute(): number {
  return useSyncExternalStore(subscribe, snapshot);
}
