// The times of the transcript (design 3.7 to 3.10 and 3.20): the clock time of a turn
// end (`14:32`), a duration (`0.2s`, `38s`, `3m 08s`, `1h 02m`) and the elapsed time of
// something that runs now. An elapsed time shows only from 2 s on, so a short wait shows
// no number that changes at once.

/** An elapsed time shows from this many ms on (design 3.20: "Elapsed time appears after 2 s"). */
export const ELAPSED_FROM_MS = 2000;

/** A duration in ms as the design writes it: `0.2s`, `9s`, `1m 02s`, `1h 02m`. */
export function duration(ms: number): string {
  const total = Math.max(0, ms);
  if (total < 1000) {
    return `${String(Math.floor(total / 100) / 10)}s`;
  }
  const seconds = Math.floor(total / 1000);
  if (seconds < 60) {
    return `${String(seconds)}s`;
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${String(minutes)}m ${pad(seconds % 60)}s`;
  }
  return `${String(Math.floor(minutes / 60))}h ${pad(minutes % 60)}m`;
}

/** The time between `from` and `to`, or undefined when one of them is not known. */
export function between(from: number | undefined, to: number | undefined): string | undefined {
  return from === undefined || to === undefined ? undefined : duration(to - from);
}

/**
 * The elapsed time of something that started at `since`, at `now`, in whole seconds
 * (`12s`, `1m 12s`). It is undefined under 2 s and when the start is not known.
 */
export function elapsed(since: number | undefined, now: number): string | undefined {
  if (since === undefined || now - since < ELAPSED_FROM_MS) {
    return undefined;
  }
  return duration(Math.floor((now - since) / 1000) * 1000);
}

/** The local clock time of `ms` (ms since the epoch), as `14:32`. */
export function clockTime(ms: number): string {
  const date = new Date(ms);
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function pad(value: number): string {
  return value < 10 ? `0${String(value)}` : String(value);
}
