// The time now, to the minute, for the words that count time: "seen 13:58" and "2m", and
// a flag for a time that comes. It is the one minute clock of the app: the header, the
// menus, Settings and the conversations sheet read it. A component that shows a time
// reads it here, never `Date.now()` in render, so a render stays pure and the words
// change once a minute.
//
// All readers share one timer (`lib/store/ticker.ts`), as the second clock of the
// transcript does. So a screen needs no effect for its times.

import { useSyncExternalStore } from "react";

import { createTicker } from "@/lib/store/ticker";

/** How often the minute is checked. */
const TICK_MS = 15_000;

const MINUTE_MS = 60_000;

/**
 * The timer of the minute clock. Each reader hears every tick and reads its own clock
 * again; React renders it only when its minute changed.
 */
const minuteTicker = createTicker(TICK_MS);

/** `ms` rounded down to the minute. */
export function minuteOf(ms: number): number {
  return Math.floor(ms / MINUTE_MS) * MINUTE_MS;
}

/**
 * The time now in ms since the epoch, rounded down to the minute. `clock` gives the time;
 * a test gives a fake one.
 */
export function useNow(clock: () => number = Date.now): number {
  return useSyncExternalStore(minuteTicker.subscribe, () => minuteOf(clock()));
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
      const wait = setTimeout(onChange, Math.max(0, at - Date.now()));
      return () => clearTimeout(wait);
    },
    () => at === undefined || Date.now() >= at,
  );
}
