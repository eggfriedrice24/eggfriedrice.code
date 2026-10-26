// The React hooks of the screens: reading a store through a selector, the frame gate of
// a transcript, the frame clock, Reduce Motion and a clock that ticks only when a card
// needs it.

import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { AccessibilityInfo } from "react-native";

import {
  type ExternalStore,
  type FrameScheduler,
  frameGated,
  type GatedStore,
} from "@/lib/store/frame-store";

/**
 * The part of `store` that `select` picks. The component renders again only when the
 * result changes (by `Object.is`), so `select` must return a part of the value, or a
 * memo of one, never a new object on each call. Keep `select` stable with useCallback.
 */
export function useStoreSelector<T, S>(store: ExternalStore<T>, select: (value: T) => S): S {
  const getSnapshot = useCallback(() => select(store.getSnapshot()), [store, select]);
  return useSyncExternalStore(store.subscribe, getSnapshot, getSnapshot);
}

/** The whole value of `store`. */
export function useStore<T>(store: ExternalStore<T>): T {
  return useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
}

/** Runs a callback on the next display frame. */
export const nextFrame: FrameScheduler = (callback) => {
  const handle = requestAnimationFrame(() => callback());
  return () => cancelAnimationFrame(handle);
};

/** A view of `store` that changes at most once per display frame, for one screen. */
export function useFrameGated<T>(store: ExternalStore<T>): GatedStore<T> {
  return useMemo(() => frameGated(store, nextFrame), [store]);
}

/** The time of the frame clock, in ms. */
export function frameNow(): number {
  return performance.now();
}

/** True while the user has Reduce Motion on. */
export function useReduceMotion(): boolean {
  const [reduce, setReduce] = useState(false);
  useEffect(() => {
    let live = true;
    AccessibilityInfo.isReduceMotionEnabled()
      .then((value) => {
        if (live) {
          setReduce(value);
        }
        return value;
      })
      .catch(() => {});
    const subscription = AccessibilityInfo.addEventListener("reduceMotionChanged", setReduce);
    return () => {
      live = false;
      subscription.remove();
    };
  }, []);
  return reduce;
}

/**
 * The wall clock time (ms since the epoch), that changes at `next(now)`: a countdown asks
 * for its next second, and no timer runs while `next` gives `undefined`.
 */
export function useWallClock(next: (now: number) => number | undefined): number {
  const [now, setNow] = useState(() => Date.now());
  const at = next(now);
  useEffect(() => {
    if (at === undefined) {
      return undefined;
    }
    const timer = setTimeout(() => setNow(Date.now()), Math.max(0, at - Date.now()));
    return () => clearTimeout(timer);
  }, [at]);
  return now;
}
