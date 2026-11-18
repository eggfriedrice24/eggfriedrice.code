// One shared timer for a clock of the screens, such as the second clock of the transcript
// and the minute clock of the header. The timer runs only while a reader listens: the
// first reader starts it and the last one stops it. So a component that shows a time
// needs no effect, and no timer runs while nothing shows a time.

/** One shared timer. */
export interface Ticker {
  /** Adds `listener`, which runs on each tick. The result removes it. */
  readonly subscribe: (listener: () => void) => () => void;
  /** True while a reader listens, so the timer runs. */
  readonly running: () => boolean;
}

/** A ticker that calls each of its readers every `ms`. */
export function createTicker(ms: number): Ticker {
  const listeners = new Set<() => void>();
  let timer: ReturnType<typeof setInterval> | undefined;
  const tick = (): void => {
    for (const listener of listeners) {
      listener();
    }
  };
  return {
    subscribe: (listener) => {
      listeners.add(listener);
      timer ??= setInterval(tick, ms);
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0 && timer !== undefined) {
          clearInterval(timer);
          timer = undefined;
        }
      };
    },
    running: () => timer !== undefined,
  };
}
