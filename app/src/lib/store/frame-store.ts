// The frame store: how a stream commits at most once per display frame. efrd sends
// drafts at about 60 Hz and the network delivers them in bursts, so the state of a
// stream can change many times in one frame. The owner sets each new state here; the
// frame store keeps the newest one in a plain field and writes it to its TanStack Store
// on the next frame, once. So a row that reads the store with a selector renders at
// most once per frame, and a row whose part did not change does not render at all.
//
// A store update renders synchronously and cannot run in a transition, so the stream
// coalesces at the source (plan section 2.4). TanStack Pacer has no frame timer, so this
// is our own code.
//
// Pure TypeScript: the owner passes the frame scheduler (`animationFrames` in the app,
// a manual one in the tests).

import { createStore, type Readable } from "@tanstack/store";

/** Runs `callback` on the next display frame; the result cancels it. */
export type FrameScheduler = (callback: () => void) => () => void;

/** The frames of the display, through `requestAnimationFrame`. */
export const animationFrames: FrameScheduler = (callback) => {
  const id = requestAnimationFrame(() => {
    callback();
  });
  return () => {
    cancelAnimationFrame(id);
  };
};

/** A value that the owner sets often, and that its readers see once per frame. */
export interface FrameStore<T> {
  /** The value of the last frame, for `useSelector`. */
  readonly store: Readable<T>;
  /** The newest value, which the store shows from the next frame. */
  latest(): T;
  /** Holds `value` and asks for a frame, unless one is asked for already. */
  set(value: T): void;
  /** Writes the newest value to the store now, without a frame. */
  flush(): void;
  /** Cancels the frame that is asked for. Later calls of `set` hold the value only. */
  dispose(): void;
}

/** A frame store that starts with `initial`. Setting the same object asks for no frame. */
export function createFrameStore<T>(initial: T, schedule: FrameScheduler): FrameStore<T> {
  const store = createStore(initial);
  let latest = initial;
  let cancel: (() => void) | undefined;
  let disposed = false;

  const flush = (): void => {
    cancel?.();
    cancel = undefined;
    if (!Object.is(store.state, latest)) {
      const next = latest;
      store.setState(() => next);
    }
  };

  return {
    store,
    latest: () => latest,
    set: (value) => {
      if (Object.is(value, latest)) {
        return;
      }
      latest = value;
      if (cancel === undefined && !disposed) {
        // A scheduler may run the callback at once, as the one of the tests does. Then
        // no frame waits, and the next `set` must ask for a frame again.
        let ran = false;
        const stop = schedule(() => {
          ran = true;
          cancel = undefined;
          flush();
        });
        if (!ran) {
          cancel = stop;
        }
      }
    },
    flush,
    dispose: () => {
      disposed = true;
      cancel?.();
      cancel = undefined;
    },
  };
}

/**
 * A memo of `select` over the values of a store: it returns its last result while the
 * input is the same object, and the last result again when `equal` says the new result
 * matches it. So a selector that builds a list gives the same list until the list
 * changes, and `useSelector` renders nothing.
 */
export function memoSelector<T, S>(
  select: (value: T) => S,
  equal: (a: S, b: S) => boolean,
): (value: T) => S {
  let last: { readonly input: T; readonly output: S } | undefined;
  return (input) => {
    if (last !== undefined && Object.is(last.input, input)) {
      return last.output;
    }
    const output = select(input);
    if (last !== undefined && equal(last.output, output)) {
      last = { input, output: last.output };
      return last.output;
    }
    last = { input, output };
    return output;
  };
}
