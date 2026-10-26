// The clock of the connection. The connection reads the time and sets timers only
// through this interface, so tests drive it with a fake clock (src/net/testing.ts) and
// the app gives it the real one (src/net/native/clock.ts).

/** A timer that the clock set. */
export interface Timer {
  /** Stops the timer. It does nothing when the timer already ran or was cancelled. */
  cancel(): void;
}

/** Time in milliseconds and one-shot timers. */
export interface Clock {
  /** The time now, in milliseconds. Only differences of two values have a meaning. */
  now(): number;
  /** Runs `run` once, `delayMs` from now. */
  setTimer(delayMs: number, run: () => void): Timer;
}
