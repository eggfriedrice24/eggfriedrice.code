// The wall clock of the elapsed times in the transcript (`running · 1m 12s`, `12s` in a
// tool header, `thinking · 12s`). It is one external store that ticks once a second, and
// only while a row that shows a time reads it: the shared timer of `lib/store/ticker.ts`
// runs only while a reader listens. So the screen renders no row on a tick that no row
// shows, and a row needs no effect for its timer.
//
// The value is the time in ms, cut to the second. It moves only on a tick, so all the
// rows of one render read the same time.

import { useSyncExternalStore } from "react";

import { createTicker } from "@/lib/store/ticker";

const TICK_MS = 1000;

const ticker = createTicker(TICK_MS);
let current = second(Date.now());

function second(ms: number): number {
  return Math.floor(ms / TICK_MS) * TICK_MS;
}

function subscribe(listener: () => void): () => void {
  return ticker.subscribe(() => {
    current = second(Date.now());
    listener();
  });
}

function snapshot(): number {
  // Without a reader the timer is off, so the value can be old: read the clock again.
  if (!ticker.running()) {
    current = second(Date.now());
  }
  return current;
}

function idle(): () => void {
  return () => undefined;
}

/**
 * The time now in ms, cut to the second. With `ticking` true the component renders again
 * each second; with false it reads the time once and does not keep the timer on.
 */
export function useNow(ticking: boolean): number {
  return useSyncExternalStore(ticking ? subscribe : idle, snapshot);
}
