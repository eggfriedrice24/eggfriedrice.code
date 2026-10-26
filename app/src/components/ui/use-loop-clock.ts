// The shared clocks of the loops: one RN Animated value for all braille cells and one for
// all blinking cursors. Each runs a native-driver loop (Animated.loop), so no JavaScript
// runs on a frame and every loader on the screen shows the same step.
//
// A clock is an external store. A view subscribes through useSyncExternalStore, which
// subscribes after the commit and unsubscribes when the view goes. The first subscriber
// starts the loop and the last one stops it, so a screen without a loader runs no
// animation, and no view needs an effect.

import { useSyncExternalStore } from "react";
import { Animated } from "react-native";

import { BLINK_LOOP, BRAILLE_LOOP, type Loop } from "./loops";

/** A clock of one loop. */
interface Clock {
  readonly value: Animated.Value;
  readonly subscribe: (listener: () => void) => () => void;
}

function clock(loop: Loop): Clock {
  const value = new Animated.Value(0);
  let views = 0;
  let running: Animated.CompositeAnimation | undefined;
  const subscribe = (): (() => void) => {
    views += 1;
    if (running === undefined) {
      value.setValue(0);
      running = Animated.loop(
        Animated.timing(value, {
          toValue: loop.steps,
          duration: loop.cycleMs,
          // Linear: the step tables of loops.ts make the steps.
          easing: (progress) => progress,
          useNativeDriver: true,
          isInteraction: false,
        }),
      );
      running.start();
    }
    return () => {
      views -= 1;
      if (views === 0 && running !== undefined) {
        running.stop();
        running = undefined;
      }
    };
  };
  return { value, subscribe };
}

const CLOCKS = {
  braille: clock(BRAILLE_LOOP),
  blink: clock(BLINK_LOOP),
} as const;

/** The name of a shared loop. */
export type LoopName = keyof typeof CLOCKS;

function stillSubscribe(): () => void {
  return () => undefined;
}

/**
 * The clock value of the loop `name`, from 0 to its number of steps. With `running`
 * false the view does not subscribe, so the loop does not run for it; the view then
 * shows its still frame (Reduce Motion).
 */
export function useLoopClock(name: LoopName, running: boolean): Animated.Value {
  const loop = CLOCKS[name];
  return useSyncExternalStore(running ? loop.subscribe : stillSubscribe, () => loop.value);
}
