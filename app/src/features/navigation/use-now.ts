// The time now, to the minute, for the words that count time: "seen 13:58" and "2m", and
// a flag for a time that comes. It is the one minute clock of the app: the header, the
// menus, Settings and the conversations sheet read it. A component that shows a time
// reads it here, never `Date.now()` in render, so a render stays pure and the words
// change once a minute.
//
// All readers share one timer: the first reader starts it and the last one stops it, as
// the second clock of the transcript does (`features/conversation/components/use-now.ts`).
// So a screen needs no effect for its times.

import { useSyncExternalStore } from "react";

/** How often the minute is checked. */
export const TICK_MS = 15_000;

const MINUTE_MS = 60_000;

const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | undefined;

/** `ms` rounded down to the minute. */
export function minuteOf(ms: number): number {
  return Math.floor(ms / MINUTE_MS) * MINUTE_MS;
}

function tick(): void {
  for (const listener of listeners) {
    listener();
  }
}

/**
 * Adds a reader of the minute clock. Each reader hears every tick and reads its own
 * clock again; React renders it only when its minute changed.
 */
export function subscribeMinute(listener: () => void): () => void {
  listeners.add(listener);
  if (timer === undefined) {
    timer = setInterval(tick, TICK_MS);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== undefined) {
      clearInterval(timer);
      timer = undefined;
    }
  };
}

/**
 * The time now in ms since the epoch, rounded down to the minute. `clock` gives the time;
 * a test gives a fake one.
 */
export function useNow(clock: () => number = Date.now): number {
  return useSyncExternalStore(subscribeMinute, () => minuteOf(clock()));
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
