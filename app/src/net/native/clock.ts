// The real clock for the connection and the client (src/net/clock.ts).
//
// iOS stops the JS timers while the app is suspended. A timer that was due runs late,
// after the app is active again; the liveness rule then sees the whole silence and
// closes the old socket, and the client also replaces it on the foreground.

import type { Clock } from "../clock";

/** `Date.now` and `setTimeout`. */
export const systemClock: Clock = {
  now: () => Date.now(),
  setTimer: (delayMs, run) => {
    const handle = setTimeout(run, delayMs);
    return { cancel: () => clearTimeout(handle) };
  },
};
