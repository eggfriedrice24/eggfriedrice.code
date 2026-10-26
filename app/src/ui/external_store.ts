// The stores that the screens read: the shape that React's useSyncExternalStore takes.
// The store of the app (src/store) gives the screens its state through this interface,
// so the screens do not depend on how it is made.
//
// `frameGated` is how the transcript commits at most once per display frame: efrd sends
// drafts at about 60 Hz and the network delivers them in bursts, so a stream store can
// change many times in one frame. The gate holds the value of the last frame and tells
// its listeners once per frame, so a row that reads it renders at most once per frame,
// and a row whose part did not change does not render at all.
//
// Pure TypeScript: the caller passes the frame scheduler (requestAnimationFrame in the
// app, a manual one in the tests).

/** A value that changes, with a way to hear of each change. */
export interface ExternalStore<T> {
  /** The value now. The same object until the value changes. */
  readonly getSnapshot: () => T;
  /** Calls `listener` after each change; the result removes it. */
  readonly subscribe: (listener: () => void) => () => void;
}

/** A store that the owner sets. */
export interface ValueStore<T> extends ExternalStore<T> {
  readonly set: (value: T) => void;
  readonly update: (change: (value: T) => T) => void;
}

/** A store that holds `initial` until it is set. Setting the same object tells nobody. */
export function createValueStore<T>(initial: T): ValueStore<T> {
  let value = initial;
  const listeners = new Set<() => void>();
  const set = (next: T): void => {
    if (Object.is(next, value)) {
      return;
    }
    value = next;
    for (const listener of listeners) {
      listener();
    }
  };
  return {
    getSnapshot: () => value,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    set,
    update: (change) => set(change(value)),
  };
}

/** Runs `callback` on the next display frame; the result cancels it. */
export type FrameScheduler = (callback: () => void) => () => void;

/** A store that follows another one and changes at most once per frame. */
export interface GatedStore<T> extends ExternalStore<T> {
  /** Takes the value of the source now, without a frame, and tells the listeners. */
  readonly flush: () => void;
}

/**
 * A view of `source` that takes its value at most once per frame. It listens to the
 * source only while it has listeners itself, and then follows each change on the next
 * frame. Without listeners it reads the source on each call.
 */
export function frameGated<T>(source: ExternalStore<T>, schedule: FrameScheduler): GatedStore<T> {
  let value = source.getSnapshot();
  const listeners = new Set<() => void>();
  let unsubscribe: (() => void) | undefined;
  let cancel: (() => void) | undefined;

  const flush = (): void => {
    cancel?.();
    cancel = undefined;
    const next = source.getSnapshot();
    if (Object.is(next, value)) {
      return;
    }
    value = next;
    for (const listener of listeners) {
      listener();
    }
  };
  const onChange = (): void => {
    if (cancel === undefined) {
      cancel = schedule(() => {
        cancel = undefined;
        flush();
      });
    }
  };

  return {
    getSnapshot: () => {
      if (unsubscribe === undefined) {
        value = source.getSnapshot();
      }
      return value;
    },
    subscribe: (listener) => {
      listeners.add(listener);
      if (unsubscribe === undefined) {
        value = source.getSnapshot();
        unsubscribe = source.subscribe(onChange);
      }
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0) {
          unsubscribe?.();
          unsubscribe = undefined;
          cancel?.();
          cancel = undefined;
        }
      };
    },
    flush,
  };
}

/**
 * A memo of `select` over the values of a store: it returns its last result while the
 * input is the same object, and the last result again when `equal` says the new result
 * matches it. So a selector that builds a list gives the same list until the list
 * changes, and useSyncExternalStore renders nothing.
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
