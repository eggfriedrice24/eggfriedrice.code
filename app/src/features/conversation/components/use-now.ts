// The wall clock of the elapsed times in the transcript (`running · 1m 12s`, `12s` in a
// tool header, `thinking · 12s`). It is one external store that ticks once a second, and
// only while a row that shows a time reads it: the first reader starts the timer and the
// last one stops it. So the screen renders no row on a tick that no row shows, and a
// row needs no effect for its timer.
//
// The value is the time in ms, cut to the second, so two reads in one second agree.

import { useSyncExternalStore } from "react";

const TICK_MS = 1000;

const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | undefined;
let current = second(Date.now());

function second(ms: number): number {
  return Math.floor(ms / TICK_MS) * TICK_MS;
}

function tick(): void {
  const next = second(Date.now());
  if (next === current) {
    return;
  }
  current = next;
  for (const listener of listeners) {
    listener();
  }
}

function subscribe(listener: () => void): () => void {
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

function snapshot(): number {
  // Without a reader the timer is off, so the value can be old: read the clock again.
  if (timer === undefined) {
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
