// BrailleCell: the one loader of the app (design 3.20). An 8-dot 2 x 4 grid of Views
// that sits in a mono line like a glyph. It is never a spinner inside prose or code; text
// that still arrives gets the block cursor.
//
// The tint tells who works: yolk when efr works (a turn, a tool, live thinking, the rows
// of the sheet), text_muted when the app waits on the network (reconnecting, connecting,
// loading, waiting on the terminal in pairing), text_on_accent inside the Send and Allow
// buttons. With Reduce Motion it shows one still frame.

import { Animated, View } from "react-native";

import { BRAILLE } from "@/theme/metrics";
import { roles } from "@/theme/roles";
import { useReduceMotion } from "@/lib/native/reduce-motion";

import { cn } from "./cn";
import { BRAILLE_STILL, brailleDots, brailleTable } from "./loops";
import { useLoopClock } from "./use-loop-clock";

/** The size: inline (7 x 13), row (8 x 15, Send and sheet rows), button (9 x 17, Allow). */
export type BrailleSize = keyof typeof BRAILLE;

/** Who works: efr (yolk), the network (text_muted), or a yolk button (text_on_accent). */
export type BrailleTint = "work" | "wait" | "on_accent";

const TINT = {
  work: roles.accent_primary,
  wait: roles.text_muted,
  on_accent: roles.text_on_accent,
} as const satisfies Record<BrailleTint, string>;

const SIZE_CLASS = {
  inline: "w-7 h-13",
  row: "w-8 h-15",
  button: "w-9 h-17",
} as const satisfies Record<BrailleSize, string>;

/** The dot layout of each size, computed once. */
const DOTS = {
  inline: brailleDots(BRAILLE.inline.width, BRAILLE.inline.height),
  row: brailleDots(BRAILLE.row.width, BRAILLE.row.height),
  button: brailleDots(BRAILLE.button.width, BRAILLE.button.height),
} as const;

/** The opacity table of each dot, computed once. */
const TABLES = BRAILLE_STILL.map((_, dot) => ({
  ...brailleTable(dot),
  extrapolate: "clamp" as const,
}));

function opacityOf(
  clock: Animated.Value,
  dot: number,
): Animated.AnimatedInterpolation<number> | number {
  const table = TABLES[dot];
  return table === undefined ? 0 : clock.interpolate(table);
}

export interface BrailleCellProps {
  readonly size?: BrailleSize;
  readonly tint?: BrailleTint;
  /**
   * The VoiceOver label, such as "Running" or "Loading". Without it the cell is
   * decoration (inside a button that has its own label, or next to words that say it).
   */
  readonly label?: string;
  /** True for a still frame even without Reduce Motion. */
  readonly still?: boolean;
}

/** The braille loader. */
export function BrailleCell(props: BrailleCellProps) {
  const size = props.size ?? "inline";
  const color = TINT[props.tint ?? "work"];
  const reduceMotion = useReduceMotion();
  const still = props.still === true || reduceMotion;
  const clock = useLoopClock("braille", !still);
  const label = props.label;
  return (
    <View
      className={cn("relative", SIZE_CLASS[size])}
      accessible={label !== undefined}
      accessibilityRole={label === undefined ? undefined : "image"}
      accessibilityLabel={label}
      accessibilityElementsHidden={label === undefined}
      testID="braille-cell"
    >
      {DOTS[size].map((dot) => (
        <Animated.View
          key={dot.dot}
          testID="braille-dot"
          style={{
            position: "absolute",
            left: dot.left,
            top: dot.top,
            width: dot.size,
            height: dot.size,
            borderRadius: dot.size / 2,
            backgroundColor: color,
            opacity: still ? (BRAILLE_STILL[dot.dot] ?? 0) : opacityOf(clock, dot.dot),
          }}
        />
      ))}
    </View>
  );
}
