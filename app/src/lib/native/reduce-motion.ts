// The Reduce Motion setting of iOS, as one small external store. A component reads it
// with `useReduceMotion()` (useSyncExternalStore, so no effect and no state of its own);
// code outside React reads `reduceMotion()`.
//
// With Reduce Motion on (design note 12): the cursor does not blink, the loaders show
// one still frame, streamed text and output lines append without a fade, and scrolls and
// stack resets do not animate.
//
// The store listens to AccessibilityInfo only while a component reads it. iOS gives the
// first value with a promise, so the value is false until that promise resolves.

import { useSyncExternalStore } from "react";
import { AccessibilityInfo } from "react-native";

let current = false;
const listeners = new Set<() => void>();
let stop: (() => void) | undefined;

function set(value: boolean): void {
  if (value === current) {
    return;
  }
  current = value;
  for (const listener of listeners) {
    listener();
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (stop === undefined) {
    const subscription = AccessibilityInfo.addEventListener("reduceMotionChanged", set);
    stop = () => subscription.remove();
    AccessibilityInfo.isReduceMotionEnabled().then(set, () => undefined);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && stop !== undefined) {
      stop();
      stop = undefined;
    }
  };
}

function snapshot(): boolean {
  return current;
}

/** True when Reduce Motion is on, as the app last heard it. */
export function reduceMotion(): boolean {
  return current;
}

/** True when Reduce Motion is on. The component renders again when it changes. */
export function useReduceMotion(): boolean {
  return useSyncExternalStore(subscribe, snapshot);
}
