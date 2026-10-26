// The yolk block cursor (design 3.2, 3.18 and 3.20). It marks where text goes or still
// arrives, and it is never a spinner.
//
// - `blink`: the cursor of the blank root. It blinks on a 1.06 s cycle, off for the
//   second half.
// - `idle`: the cursor of the no-machine root, at 35% and never blinking.
// - `solid`: always on.
// - `streaming`: the end of a streamed reply or code block. It is solid while tokens
//   arrive and blinks down to 25% after 600 ms of silence. The caller changes
//   `activity` on each new token; before the first token it blinks.
//
// With Reduce Motion the cursor does not blink: a blinking cursor is solid.

import { useEffect, useState } from "react";
import { Animated } from "react-native";

import { CURSOR } from "@/theme/metrics";
import { OPACITY } from "@/theme/materials";
import { roles } from "@/theme/roles";
import { useReduceMotion } from "@/lib/native/reduce-motion";

import { blinkTable } from "./loops";
import { useLoopClock } from "./use-loop-clock";

/** The size: shell (9 x 19, design item 8.1), prose (7 x 14) or code (6 x 13). */
export type CursorSize = keyof typeof CURSOR;

/** How the cursor moves. */
export type CursorMode = "blink" | "idle" | "solid" | "streaming";

/** The silence after the last token before a streaming cursor blinks. */
export const STREAM_SILENCE_MS = 600;

const BLINK = { ...blinkTable(0), extrapolate: "clamp" as const };
const STREAM_BLINK = { ...blinkTable(OPACITY.cursorBlinkLow), extrapolate: "clamp" as const };

export interface BlockCursorProps {
  readonly size: CursorSize;
  readonly mode: CursorMode;
  /** For `streaming`: a value that changes on each new token, such as the text length. */
  readonly activity?: number | string;
}

/** The block cursor. VoiceOver skips it. */
export function BlockCursor(props: BlockCursorProps) {
  const reduceMotion = useReduceMotion();
  const moves = (props.mode === "blink" || props.mode === "streaming") && !reduceMotion;
  const clock = useLoopClock("blink", moves);
  // 1 while tokens arrive, 0 after the silence: the cursor shows the blink only at 0.
  const [quiet] = useState(() => new Animated.Value(1));
  const streaming = props.mode === "streaming" && moves;
  const activity = props.activity;
  useEffect(() => {
    if (!streaming) {
      return undefined;
    }
    if (activity === undefined) {
      // No token yet: the cursor waits, so it blinks at once.
      quiet.setValue(0);
      return undefined;
    }
    quiet.setValue(1);
    const wait = Animated.timing(quiet, {
      toValue: 0,
      duration: 0,
      delay: STREAM_SILENCE_MS,
      useNativeDriver: true,
      isInteraction: false,
    });
    wait.start();
    return () => wait.stop();
  }, [streaming, activity, quiet]);
  const size = CURSOR[props.size];
  return (
    <Animated.View
      testID="block-cursor"
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
      style={{
        width: size.width,
        height: size.height,
        backgroundColor: roles.accent_primary,
        opacity: cursorOpacity(props.mode, moves, clock, quiet),
      }}
    />
  );
}

function cursorOpacity(
  mode: CursorMode,
  moves: boolean,
  clock: Animated.Value,
  quiet: Animated.Value,
): Animated.AnimatedNode | number {
  if (mode === "idle") {
    return OPACITY.idleCursor;
  }
  if (!moves || mode === "solid") {
    return 1;
  }
  if (mode === "blink") {
    return clock.interpolate(BLINK);
  }
  // On while tokens arrive (quiet 1), the blink after the silence (quiet 0):
  // quiet + (1 - quiet) * blink.
  const blink = clock.interpolate(STREAM_BLINK);
  return Animated.add(quiet, Animated.multiply(Animated.subtract(1, quiet), blink));
}
