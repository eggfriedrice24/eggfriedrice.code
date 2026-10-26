// The time now, to the minute, for the words that count time: "seen 13:58" and "2m", and
// a flag for a time that comes. It is the one minute clock of the app: the header, the
// menus and Settings read it. A component that shows a time reads it here, never
// `Date.now()` in render, so a render stays pure and the words change once a minute.

import { useSyncExternalStore } from "react";

/** How often the minute is checked. */
const TICK_MS = 15_000;

const MINUTE_MS = 60_000;

function minuteNow(): number {
  return Math.floor(Date.now() / MINUTE_MS) * MINUTE_MS;
}

function subscribe(onChange: () => void): () => void {
  const timer = setInterval(onChange, TICK_MS);
  return () => clearInterval(timer);
}

/** The time now in ms since the epoch, rounded down to the minute. */
export function useNow(): number {
  return useSyncExternalStore(subscribe, minuteNow);
}

/**
 * True once the time `at` (ms since the epoch) is reached; always true for undefined. The
 * component renders again when the time comes.
 */
export function useReached(at: number | undefined): boolean {
  return useSyncExternalStore(
    (onChange) => {
      if (at === undefined) {
        return () => undefined;
      }
      const timer = setTimeout(onChange, Math.max(0, at - Date.now()));
      return () => clearTimeout(timer);
    },
    () => at === undefined || Date.now() >= at,
  );
}
